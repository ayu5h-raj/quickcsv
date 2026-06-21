# Stage 1: Build the WASM web app with Trunk
FROM rust:slim-bookworm AS builder

# Build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    git \
    && rm -rf /var/lib/apt/lists/*

# WASM target + Trunk
RUN rustup target add wasm32-unknown-unknown
RUN cargo install --locked trunk wasm-bindgen-cli

WORKDIR /app

# Manifests first (better layer caching)
COPY Cargo.toml Cargo.lock Trunk.toml ./

# Source and assets
COPY src ./src
COPY icons ./icons
COPY index.html ./

# Build the optimized release WASM bundle.
# Served at the domain root on the VPS, so no --public-url prefix.
RUN trunk build --release

# Stage 2: Serve the static bundle with Nginx
FROM nginx:alpine

COPY --from=builder /app/dist /usr/share/nginx/html

EXPOSE 80

CMD ["nginx", "-g", "daemon off;"]
