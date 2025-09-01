<div align="center">
    <img align="center" src="./assets/logo.png" width="130" />
</div>

<p align="center">
  <h1 align="center">InterShare SDK</h1>
</p>

[![CI](https://github.com/InterShare/InterShareSDK/actions/workflows/ci.yml/badge.svg)](https://github.com/InterShare/InterShareSDK/actions/workflows/ci.yml)
![](https://www.repostatus.org/badges/latest/wip.svg)

This is the internal SDK used by the InterShare clients.

## Encryption

The network stream is encrypted using the `XChaCha20` algorithm ([using this crate](https://crates.io/crates/chacha20)).
