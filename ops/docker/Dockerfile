# Multi-stage build for the Tempo off-chain services. One image per binary,
# selected by the BIN build-arg:
#   docker build -f ops/docker/Dockerfile --build-arg BIN=tempo-liquidator -t tempo/liquidator .
# Valid BIN values: tempo-keeper | tempo-liquidator | tempo-api | tempo-mm-bot |
#   tempo-sim | tempo-sim-provision | tempo-sim-orchestrator.
FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
ARG BIN=tempo-keeper
RUN cargo build --release --bin ${BIN} \
    && cp target/release/${BIN} /build/service

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home tempo
USER tempo
COPY --from=builder /build/service /usr/local/bin/service
ENTRYPOINT ["/usr/local/bin/service"]
