#!/usr/bin/env bash
# Sourceable helper: ensures both release binaries the integration matrix
# consumes — `daft` and `xtask` — exist under target/release/ before the
# matrix runs. Prints nothing on stdout the caller depends on.
#
# Why xtask needs an explicit build: the integration tasks depend on
# `dev:setup`, which builds release `daft` and the multicall symlinks via
# `cargo build --release`. But this is a *root-package* workspace (the top
# `Cargo.toml` is both `[workspace]` and the `daft` `[package]`) with no
# `default-members`, so `cargo build --release` builds only the root package
# — it does NOT build the `xtask` member. The matrix's `runner-output`
# scenario shells out to `$BINARY_DIR/xtask` (= target/release/xtask), so
# without this build it fails with `No such file or directory`. CI sidesteps
# this by building both binaries explicitly before the matrix
# (.github/workflows/test.yml); this helper is the local mirror.
#
# File is intentionally non-executable so mise hides it from `mise tasks ls`.

ensure_release_binaries() {
  echo "Building release xtask for the integration matrix..."
  cargo build --release --package xtask

  # Fail loudly if either binary is missing rather than silently resurfacing
  # the runner-output "No such file or directory" failure (mirrors the
  # two-binary guard in .github/workflows/test.yml).
  local missing=0 bin
  for bin in daft xtask; do
    if [[ ! -x "target/release/$bin" ]]; then
      echo "ERROR: target/release/$bin missing after build" >&2
      missing=1
    fi
  done
  return "$missing"
}
