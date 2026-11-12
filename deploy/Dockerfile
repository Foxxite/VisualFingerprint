# Build:  docker build -f deploy/Dockerfile -t vfp-server .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p vfp-server

FROM debian:bookworm-slim
# ffmpeg is needed for audio and video
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 10001 vfp && mkdir /data && chown vfp /data
COPY --from=build /src/target/release/vfp-server /usr/local/bin/vfp-server
USER vfp
ENV VFP_DATA=/data VFP_BIND=0.0.0.0:8443
VOLUME /data
EXPOSE 8443
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["vfp-server", "health"]
ENTRYPOINT ["vfp-server"]
CMD ["serve"]
