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
COPY adapters ./adapters
COPY apps ./apps

RUN --mount=type=cache,id=sumpter-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=sumpter-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=sumpter-target,target=/src/target \
    cargo build --release --locked --bin sumpterd-linux \
    && mkdir -p /out \
    && cp /src/target/release/sumpterd-linux /out/sumpterd \
    && chmod 0755 /out/sumpterd \
    && file /out/sumpterd \
    | grep -Eq 'ELF.*(statically linked|static-pie linked)'

FROM alpine:3.22 AS runtime

# See the builder-stage note about pinning the verified base image digest.
# hadolint ignore=DL3018
RUN apk add --no-cache ca-certificates \
    && mkdir -p /config /opt/sumpter/web \
    && chmod 0700 /config

COPY --from=builder /out/sumpterd /usr/local/bin/sumpterd
COPY web/ /opt/sumpter/web/
COPY config.example.json /opt/sumpter/config.example.json

# Admin defaults (daemon reads these; CLI flags still win if provided in command:).
# `/config/admin-password` is mandatory because CMD sets `--config-dir /config`;
# mount it before starting. The WebUI login and API session still belong behind HTTPS publicly.
ENV RUST_LOG=info \
    SUMPTER_ADMIN_HOST=127.0.0.1 \
    SUMPTER_ADMIN_PORT=57879 \
    SUMPTER_WEB_ROOT=/opt/sumpter/web

WORKDIR /opt/sumpter
VOLUME ["/config"]
# Proxy default 57878 (from config.json); Admin default 57879 (overridable).
EXPOSE 57878 57879
STOPSIGNAL SIGTERM

# Healthcheck probes the unauthenticated, content-free listener liveness route.
# Shell form so $${SUMPTER_ADMIN_PORT} expands at container runtime ($$ -> $).
# Alpine busybox provides wget.
# hadolint ignore=DL3025
HEALTHCHECK --interval=15s --timeout=4s --start-period=10s --retries=4 \
    CMD wget -q -T 3 -O /dev/null \
      "http://127.0.0.1:$${SUMPTER_ADMIN_PORT:-57879}/healthz" \
      || exit 1

ENTRYPOINT ["/usr/local/bin/sumpterd"]
# Fixed paths inside the image; Admin listen comes from env unless you append CLI flags.
CMD ["--config-dir", "/config", "--web-root", "/opt/sumpter/web"]
