# syntax=docker/dockerfile:1.7

ARG RUST_IMAGE=rust:1.88-alpine
FROM ${RUST_IMAGE} AS builder

# Package revisions follow the selected Alpine image tag; pin the base image by
# digest after the first CI build to freeze the repository snapshot as well.
# hadolint ignore=DL3018
RUN apk add --no-cache build-base file
SHELL ["/bin/ash", "-eo", "pipefail", "-c"]
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,id=kekulv-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=kekulv-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=kekulv-target,target=/src/target \
    cargo build --release --locked --bin kekulvd \
    && mkdir -p /out \
    && cp /src/target/release/kekulvd /out/kekulvd \
    && chmod 0755 /out/kekulvd \
    && file /out/kekulvd \
    | grep -Eq 'ELF.*(statically linked|static-pie linked)'

FROM alpine:3.22 AS runtime

# See the builder-stage note about pinning the verified base image digest.
# hadolint ignore=DL3018
RUN apk add --no-cache ca-certificates \
    && addgroup -S -g 10001 kekulv \
    && adduser -S -D -H -h /home/kekulv -u 10001 -G kekulv kekulv \
    && mkdir -p /home/kekulv /config /opt/kekulv/web \
    && chown -R 10001:10001 /home/kekulv /config /opt/kekulv \
    && chmod 0700 /home/kekulv /config

COPY --from=builder /out/kekulvd /usr/local/bin/kekulvd
COPY --chown=kekulv:kekulv web/ /opt/kekulv/web/
COPY --chown=kekulv:kekulv config.example.json /opt/kekulv/config.example.json

# Admin defaults (daemon reads these; CLI flags still win if provided in command:).
# `/config/admin-password` is mandatory because CMD sets `--config-dir /config`;
# mount it before starting. The WebUI login and API session still belong behind HTTPS publicly.
ENV HOME=/home/kekulv \
    RUST_LOG=info \
    KEKULV_ADMIN_HOST=127.0.0.1 \
    KEKULV_ADMIN_PORT=57879 \
    KEKULV_WEB_ROOT=/opt/kekulv/web

WORKDIR /opt/kekulv
USER 10001:10001
VOLUME ["/config"]
# Proxy default 57878 (from config.json); Admin default 57879 (overridable).
EXPOSE 57878 57879
STOPSIGNAL SIGTERM

# Healthcheck probes the unauthenticated, content-free listener liveness route.
# Shell form so $${KEKULV_ADMIN_PORT} expands at container runtime ($$ -> $).
# Alpine busybox provides wget.
# hadolint ignore=DL3025
HEALTHCHECK --interval=15s --timeout=4s --start-period=10s --retries=4 \
    CMD wget -q -T 3 -O /dev/null \
      "http://127.0.0.1:$${KEKULV_ADMIN_PORT:-57879}/healthz" \
      || exit 1

ENTRYPOINT ["/usr/local/bin/kekulvd"]
# Fixed paths inside the image; Admin listen comes from env unless you append CLI flags.
CMD ["--config-dir", "/config", "--web-root", "/opt/kekulv/web"]
