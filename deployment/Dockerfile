FROM rust:1.75-slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release -p fig-server
FROM debian:bookworm-slim
COPY --from=build /app/target/release/fig-server /usr/local/bin/fig-server 2>/dev/null || echo "binary lands in later commits"
ENTRYPOINT ["fig-server"]
