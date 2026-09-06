FROM rust:1.88-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY server ./server
RUN cargo build --release --locked

FROM alpine:3.20

RUN apk add --no-cache ca-certificates

WORKDIR /opt/yard

COPY --from=builder /build/target/release/yard-web /usr/local/bin/yard-web
COPY static /opt/yard/static

ENV YARD_WEB_HOST=0.0.0.0 \
    YARD_WEB_PORT=8088 \
    YARD_PROJECTS_DIR=/etc/yard/projects \
    YARD_STATE_DIR=/var/lib/yard \
    YARD_WEB_STATIC=/opt/yard/static

USER 65532:65532

CMD ["yard-web"]
