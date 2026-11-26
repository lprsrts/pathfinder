# The collector image. Build context: the repository root (schema/ and
# collector/), e.g. `git archive HEAD schema collector | docker build -f collector/Dockerfile -`.
# COMMIT names the source it was built from; the service reports it.
FROM rust:1.95-slim-trixie AS build
WORKDIR /src
COPY schema schema
COPY collector collector
RUN cd collector && cargo build --release --locked && strip target/release/collector

FROM debian:trixie-slim
ARG COMMIT=unknown
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/collector/target/release/collector /usr/local/bin/collector
USER 2002:2002
ENV COLLECTOR_COMMIT=$COMMIT \
    COLLECTOR_DB=/data/collector.sqlite \
    COLLECTOR_PUBLIC=/public \
    COLLECTOR_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/collector"]
CMD ["serve"]
