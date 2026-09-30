# The two images share only rust-base and stubs. Past them, target server
# (compose's api) runs server-build, and target web runs wasm-bindgen-version,
# wasm-tools, wasm-build and web-build, so a server-only edit runs no WASM
# step, and a WASM-only edit no server step.

# The web-build stage's Node. It must equal .node-version, which CI's
# setup-node reads; the deploy job in ci.yml fails when the two differ.
ARG NODE_VERSION=24.14.0

# The toolchain rust-toolchain.toml pins. The file comes first, so every cargo
# and rustup call below uses it. The deploy job in ci.yml fails when this tag,
# or Cargo.toml's rust-version, differs from it.
FROM rust:1.98.1-bookworm AS rust-base
WORKDIR /app
COPY rust-toolchain.toml ./

# The workspace with stub sources (and no build.rs): Cargo needs every member's
# manifest and a target for each before it builds any of them. Each build stage
# compiles its dependencies against these stubs first, so that layer survives
# crate edits, then deletes the stubs' outputs and fingerprints, which makes its
# real build recompile the workspace crates whatever the copied files' mtimes.
FROM rust-base AS stubs
COPY Cargo.toml Cargo.lock ./
COPY crates/core/Cargo.toml crates/core/
COPY crates/search/Cargo.toml crates/search/
COPY crates/wasm/Cargo.toml crates/wasm/
COPY crates/server/Cargo.toml crates/server/
RUN for crate in core search wasm; do mkdir -p crates/$crate/src && touch crates/$crate/src/lib.rs; done \
    && mkdir -p crates/server/src && echo 'fn main() {}' > crates/server/src/main.rs

# Copies only what the server compiles (the wasm crate keeps its stub), so a
# WASM-only edit leaves this stage cached.
FROM stubs AS server-build
RUN cargo build --locked --release -p sokomind-server \
    && find target -name '*sokomind*' -prune -exec rm -rf {} +
COPY crates/core crates/core
COPY crates/search crates/search
COPY crates/server crates/server
COPY data data
COPY migrations migrations
RUN cargo build --locked --release -p sokomind-server

FROM debian:bookworm-slim AS server
COPY --from=server-build /app/target/release/sokomind-server /usr/local/bin/sokomind-server
USER 65532:65532
ENV BIND_ADDR=0.0.0.0:3000
EXPOSE 3000
ENTRYPOINT ["sokomind-server"]

# wasm-bindgen-cli must equal wasm-bindgen's version in Cargo.lock. wasm-build
# runs wasm-bindgen without scripts/build-wasm.mjs, which checks that, so this
# stage reads the version from Cargo.lock (tr drops CRLF endings). ci.yml and
# scripts/build-wasm.mjs parse the lock the same way; keep the three in step.
FROM rust-base AS wasm-bindgen-version
COPY Cargo.lock ./
RUN version=$(tr -d '\r' < Cargo.lock | grep -m1 -A1 -x 'name = "wasm-bindgen"' | sed -n 's/^version = "\(.*\)"$/\1/p') \
    && if [ -z "$version" ]; then echo 'Cargo.lock does not list wasm-bindgen' >&2; exit 1; fi \
    && printf '%s' "$version" > /wasm-bindgen.version

# The install, a from-source compile of several minutes, copies only the
# version file, whose content keys its cache: a Cargo.lock edit that keeps
# wasm-bindgen's version keeps the installed CLI.
FROM rust-base AS wasm-tools
RUN rustup target add wasm32-unknown-unknown
COPY --from=wasm-bindgen-version /wasm-bindgen.version /tmp/
RUN cargo install wasm-bindgen-cli --version "$(cat /tmp/wasm-bindgen.version)" --locked

# Copies only what the WASM compiles (the server crate keeps its stub), so a
# server-only edit leaves this stage cached. data/ is left out because only
# test code in core and search includes it; a non-test include_str! of data/
# in core, search or wasm must add COPY data data here.
FROM wasm-tools AS wasm-build
COPY --from=stubs /app ./
RUN cargo build --locked -p sokomind-wasm --target wasm32-unknown-unknown --profile wasm-release \
    && find target -name '*sokomind*' -prune -exec rm -rf {} +
COPY crates/core crates/core
COPY crates/search crates/search
COPY crates/wasm crates/wasm
# Mirrors the cargo build and wasm-bindgen commands in scripts/build-wasm.mjs;
# change both together.
RUN cargo build --locked -p sokomind-wasm --target wasm32-unknown-unknown --profile wasm-release \
    && wasm-bindgen --target web --out-dir web/wasm --out-name sokomind target/wasm32-unknown-unknown/wasm-release/sokomind_wasm.wasm

FROM node:${NODE_VERSION}-bookworm-slim AS web-build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY web web
COPY data data
COPY --from=wasm-build /app/web/wasm web/wasm
RUN npm run build:web

# deploy/nginx.conf's upstream needs 1.27.3+ ("resolve"). 1.30 is the
# maintained stable branch; 1.28 has been legacy, without fixes, since 1.30.0.
FROM nginx:1.30-alpine AS web
COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=web-build /app/web/dist /usr/share/nginx/html
EXPOSE 80
