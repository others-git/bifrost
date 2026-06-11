# ── Stage 1: build frontend ────────────────────────────────────────────────
FROM node:20-alpine AS frontend-builder
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json* ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# ── Stage 2: build Rust binary ─────────────────────────────────────────────
FROM rust:alpine AS rust-builder
# musl-dev: C toolchain for musl libc
# sqlite-dev: SQLite headers + static lib
RUN apk add --no-cache musl-dev sqlite-dev pkgconfig
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY migrations/ ./migrations/
# Copy built frontend assets so rust-embed can bake them in.
COPY --from=frontend-builder /app/frontend/dist ./frontend/dist

# Build in release mode against musl — fully static except for sqlite.
RUN cargo build --release

# ── Stage 3: minimal runtime ───────────────────────────────────────────────
FROM alpine:3.20
RUN apk add --no-cache sqlite-libs ca-certificates tzdata
WORKDIR /app
COPY --from=rust-builder /app/target/release/bifrost ./bifrost

# Persist DB and config on a mounted volume.
VOLUME ["/data"]
ENV DATABASE_URL=sqlite:///data/bifrost.db
ENV BIND_ADDR=0.0.0.0:3000

EXPOSE 3000
ENTRYPOINT ["./bifrost"]
