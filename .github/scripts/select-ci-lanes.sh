#!/usr/bin/env bash
set -euo pipefail

contract=false
coordinator=false

if [[ "${1:-}" == "--uncertain" ]]; then
  printf 'contract=true\ncoordinator=true\n'
  exit 0
fi

for changed_path in "$@"; do
  case "$changed_path" in
    Cargo.toml|Cargo.lock|rust-toolchain.toml|src/lib.rs|.github/*)
      contract=true
      coordinator=true
      ;;
    src/contracts/*|tests/contracts.rs|tests/protected_effect_contracts.rs|schemas/*|examples/contracts/*|examples/generate-contract-artifacts/*|docs/contracts/*|docs/adr/0003-*|docs/adr/0004-*)
      contract=true
      ;;
    src/main.rs|tests/cli.rs|evidence/*|docs/adr/0001-*|docs/adr/0002-*)
      coordinator=true
      ;;
    README.md|CONTRIBUTING.md|LICENSE*|docs/*)
      ;;
    *)
      contract=true
      coordinator=true
      ;;
  esac
done

printf 'contract=%s\ncoordinator=%s\n' "$contract" "$coordinator"
