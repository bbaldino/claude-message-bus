FROM rust:1-slim AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY schema.sql ./
COPY src ./src
RUN cargo build --release --bin claude-bus

FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/claude-bus /usr/local/bin/claude-bus
VOLUME ["/data"]
EXPOSE 7777
ENTRYPOINT ["claude-bus", "serve", "--port", "7777", "--data", "/data"]
