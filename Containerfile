# Containerfile builds the normal Linux executable for local Podman development.
# The runtime contains only the binary and non-secret configuration.
FROM docker.io/library/rust:1.99.1-bookworm@sha256:32f7944ed00949c44d3944a0a852b3e5ea862a610a38b43ac92a33aa87256b75 AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY config.example.toml ./
COPY deploy/Caddyfile ./deploy/Caddyfile
RUN --mount=type=cache,id=garmin-pace-registry,target=/usr/local/cargo/registry \
    mkdir -p /data /out && chmod 700 /data && cargo build --release --locked --bin pace && cp target/release/pace /out/pace

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9ae6393649d0c55122c66fe6d4803b0d85b2d7b60b2b09b690ce80c086f40a83
WORKDIR /app
COPY --from=build /out/pace /app/pace
COPY config.container.toml /app/config.toml
COPY --from=build --chown=65532:65532 /data /data
USER 65532:65532
VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["/app/pace", "--config", "/app/config.toml"]
CMD ["serve", "--transport", "http"]
