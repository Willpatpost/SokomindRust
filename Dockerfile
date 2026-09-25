FROM rust:1.98.1-bookworm AS rust-build
WORKDIR /app
# Manifests first: the toolchain install survives crate and web edits.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN rustup target add wasm32-unknown-unknown && cargo install wasm-bindgen-cli --version 0.2.128 --locked
# Dependencies alone, against stub sources (and no build.rs), so crate edits
# reuse this layer. The stubs' own outputs are deleted so nothing stale remains.
COPY crates/core/Cargo.toml crates/core/
COPY crates/search/Cargo.toml crates/search/
COPY crates/wasm/Cargo.toml crates/wasm/
COPY crates/server/Cargo.toml crates/server/
RUN for crate in core search wasm; do mkdir -p crates/$crate/src && touch crates/$crate/src/lib.rs; done \
    && mkdir -p crates/server/src && echo 'fn main() {}' > crates/server/src/main.rs \
    && cargo build --locked --release -p sokomind-server \
    && cargo build --locked -p sokomind-wasm --target wasm32-unknown-unknown --profile wasm-release \
    && find target -name '*sokomind*' -prune -exec rm -rf {} +
COPY crates crates
COPY data data
COPY migrations migrations
# COPY keeps the build context's mtimes, which can predate the stub build.
RUN find crates data migrations -type f -exec touch {} + && cargo build --locked --release -p sokomind-server
RUN cargo build --locked -p sokomind-wasm --target wasm32-unknown-unknown --profile wasm-release && wasm-bindgen --target web --out-dir web/wasm --out-name sokomind target/wasm32-unknown-unknown/wasm-release/sokomind_wasm.wasm

FROM debian:bookworm-slim AS server
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=rust-build /app/target/release/sokomind-server /usr/local/bin/sokomind-server
USER 65532:65532
ENV BIND_ADDR=0.0.0.0:3000
EXPOSE 3000
ENTRYPOINT ["sokomind-server"]

FROM node:24-bookworm-slim AS web-build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY web web
COPY data data
COPY --from=rust-build /app/web/wasm web/wasm
RUN npm run check && npx vite build --config web/vite.config.ts

FROM nginx:stable-alpine AS web
COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=web-build /app/web/dist /usr/share/nginx/html
EXPOSE 80
