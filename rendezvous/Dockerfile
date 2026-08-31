# The standalone rendezvous relay as a tiny container.
# Build from the repo root:  docker build -f rendezvous/Dockerfile -t aspen-relay .
# Run: docker run -p 7440:7440 aspen-relay \
#        --listen 0.0.0.0:7440 --mesh mymesh --root-pubkey "<base64>"
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p aspen-relay

FROM debian:stable-slim
RUN useradd -m aspen
COPY --from=build /src/target/release/aspen-relay /usr/local/bin/aspen-relay
USER aspen
EXPOSE 7440
ENTRYPOINT ["aspen-relay"]
