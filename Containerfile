# Containerfile builds the normal Linux executable for local Podman development and for the
# aarch64/x86_64 installer archives. The runtime contains only the binary and non-secret configuration.
# Base images are pinned to multi-arch manifest indexes so --platform selects the matching layers.
FROM docker.io/library/rust:1.99.1-bookworm@sha256:48b7f435b8fc6378ce43b44f15e3a47fd8919b2825f835c39bd4bd3da50af5b8 AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY config.example.toml ./
COPY deploy/Caddyfile ./deploy/Caddyfile
RUN --mount=type=cache,id=garmin-pace-registry,target=/usr/local/cargo/registry \
    mkdir -p /data /out && chmod 700 /data && cargo build --release --locked --bin pace && cp target/release/pace /out/pace

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
WORKDIR /app
COPY --from=build /out/pace /app/pace
COPY config.container.toml /app/config.toml
COPY --from=build --chown=65532:65532 /data /data
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["/app/pace", "--config", "/app/config.toml"]
CMD ["serve", "--transport", "http"]
