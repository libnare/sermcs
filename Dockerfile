FROM rust:latest AS base
WORKDIR /app

RUN cargo install --config net.git-fetch-with-cli=true cargo-chef

FROM base AS planner

COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM base AS builder

ARG TARGETPLATFORM
ARG RUSTFLAGS='-C target-feature=+crt-static'

COPY --from=planner /app/recipe.json ./
RUN if [ "$TARGETPLATFORM" = "linux/amd64" ]; then TARGET=x86_64-unknown-linux-gnu; elif [ "$TARGETPLATFORM" = "linux/arm64" ]; then TARGET=aarch64-unknown-linux-gnu; fi \
    && cargo chef cook --release --target $TARGET --recipe-path recipe.json

COPY . .
RUN if [ "$TARGETPLATFORM" = "linux/amd64" ]; then TARGET=x86_64-unknown-linux-gnu; elif [ "$TARGETPLATFORM" = "linux/arm64" ]; then TARGET=aarch64-unknown-linux-gnu; fi \
    && cargo build --release --target $TARGET \
    && cp -r target/$TARGET/release/sermcs target/release/sermcs

RUN if [ "$TARGETPLATFORM" = "linux/amd64" ]; then TARGET=amd64; elif [ "$TARGETPLATFORM" = "linux/arm64" ]; then TARGET=arm64; fi \
    && curl -sS "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-$TARGET-static.tar.xz" -o ffmpeg.tar.xz \
    && tar -xf ffmpeg.tar.xz \
    && mv ffmpeg-*-$TARGET-static ffmpeg-static

FROM alpine:latest AS packer

RUN apk add --no-cache upx

COPY --from=builder /app/target/release/sermcs /app/sermcs
RUN upx --lzma /app/sermcs

COPY --from=builder /app/ffmpeg-static/ffmpeg /app/ffmpeg
RUN upx --lzma /app/ffmpeg

FROM gcr.io/distroless/static-debian12:nonroot AS runtime
WORKDIR /app

COPY --from=packer /app/ffmpeg /bin
COPY --from=packer /app/sermcs ./
CMD ["./sermcs"]