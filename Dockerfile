FROM ubuntu:noble

RUN apt update
RUN apt install -y curl gcc pkg-config libssl-dev gnupg golang less

RUN useradd rust-user -md /gathering -u 1337 -s /bin/bash

USER rust-user

WORKDIR /gathering

ENV PATH="/gathering/.cargo/bin:/gathering/.local/bin:/gathering/.go/bin:$PATH"
ENV GOPATH="/gathering/.go"

RUN go install github.com/charmbracelet/glow@latest

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs > /tmp/rustup-init
RUN sh /tmp/rustup-init -y

COPY --chown=rust-user ./ /tmp/cargo-cult
RUN cargo install --path /tmp/cargo-cult
RUN --mount=type=secret,id=AIRTABLE_KEY,env=AIRTABLE_KEY \
    cargo-cult install-all-packages

RUN mkdir -p /gathering/.local/bin
RUN ln -s /gathering/.cargo/bin/cargo-cult /gathering/.local/bin/readme

RUN rm /gathering/.bashrc

ENTRYPOINT ["cargo-cult", "ssh-entrypoint"]
