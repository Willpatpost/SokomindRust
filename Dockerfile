FROM rust:1.98.1-bookworm AS rust-build
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY data data
COPY migrations migrations
RUN cargo build --locked --release -p sokomind-server

FROM debian:bookworm-slim AS server
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=rust-build /app/target/release/sokomind-server /usr/local/bin/sokomind-server
USER 65532:65532
ENV BIND_ADDR=0.0.0.0:3000
EXPOSE 3000
ENTRYPOINT ["sokomind-server"]

FROM rust-build AS wasm-build
RUN rustup target add wasm32-unknown-unknown && cargo install wasm-bindgen-cli --version 0.2.128 --locked
RUN cargo build --locked -p sokomind-wasm --target wasm32-unknown-unknown --profile wasm-release && wasm-bindgen --target web --out-dir web/wasm --out-name sokomind target/wasm32-unknown-unknown/wasm-release/sokomind_wasm.wasm

FROM node:24-bookworm-slim AS web-build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY web web
COPY data data
COPY --from=wasm-build /app/web/wasm web/wasm
RUN npm run check && npx vite build --config web/vite.config.ts

FROM nginx:stable-alpine AS web
COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=web-build /app/web/dist /usr/share/nginx/html
EXPOSE 80
