ARG BUILDER_IMAGE=rust:1.98.1-slim-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim

FROM scratch AS inputs
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY proto ./proto
COPY client ./client
COPY server ./server
COPY docs ./docs

FROM ${BUILDER_IMAGE} AS build
WORKDIR /src
COPY --from=inputs / /src
RUN cargo build --locked --release -p moenotes-server

FROM ${RUNTIME_IMAGE}
COPY --from=build /src/target/release/moenotes-server /usr/local/bin/moenotes-server
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["moenotes-server"]
CMD ["serve", "/etc/moenotes/config.toml"]
