# Build the diagnostic launcher elsewhere; none of this stage's compiler,
# headers, Cargo cache, or build utilities enter the measured container.
FROM rust:1.85.0-bookworm AS launcher
RUN cargo install just --version 1.40.0 --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install --no-install-recommends -y \
    python3 curl ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=launcher /usr/local/cargo/bin/just /usr/local/bin/just
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN curl --fail --silent --show-error --location \
      https://static.rust-lang.org/rustup/archive/1.28.2/x86_64-unknown-linux-gnu/rustup-init \
      --output /tmp/rustup-init && \
    sha256sum /tmp/rustup-init > /opt/rustup-installer.sha256 && \
    chmod +x /tmp/rustup-init && \
    /tmp/rustup-init -y --no-modify-path --default-toolchain none && \
    python3 -c 'import subprocess,tomllib; t=tomllib.load(open("/tmp/rust-toolchain.toml","rb"))["toolchain"]; subprocess.run(["rustup","toolchain","install",t["channel"],"--profile","minimal","--component",",".join(t["components"])],check=True)' && \
    rm /tmp/rustup-init
WORKDIR /source
ENTRYPOINT ["python3", "/checker/check.py"]
