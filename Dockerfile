FROM rust:1.58.1
WORKDIR /usr/src/validator-rust
COPY . .
RUN cargo install --path .
ENTRYPOINT ["validator-rust"]