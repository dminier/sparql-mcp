FROM rust:1.78-slim AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --package sparql-mcp

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /build/target/release/sparql-mcp /usr/local/bin/sparql-mcp
COPY sparql-mcp.toml ./
COPY ontology/ ./ontology/
RUN mkdir -p store front/docs
ENTRYPOINT ["sparql-mcp", "serve"]
