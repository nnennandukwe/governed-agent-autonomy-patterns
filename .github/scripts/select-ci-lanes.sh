#!/usr/bin/env bash
set -euo pipefail

contract=false
coordinator=false
runtime=false

if [[ "${1:-}" == "--uncertain" ]]; then
  printf 'contract=true\ncoordinator=true\nruntime=true\n'
  exit 0
fi

for changed_path in "$@"; do
  case "$changed_path" in
    Cargo.toml|Cargo.lock|rust-toolchain.toml|src/lib.rs|.github/*)
      contract=true
      coordinator=true
      runtime=true
      ;;
    src/contracts/*|tests/contracts.rs|tests/protected_effect_contracts.rs)
      contract=true
      runtime=true
      ;;
    schemas/*|examples/contracts/*|examples/generate-contract-artifacts/*|docs/contracts/*|docs/adr/0003-*|docs/adr/0004-*)
      contract=true
      ;;
    src/main.rs|tests/cli.rs|evidence/*|docs/adr/0001-*|docs/adr/0002-*)
      coordinator=true
      ;;
    src/runtime.rs|tests/runtime.rs|docs/adr/0005-*)
      runtime=true
      ;;
    README.md|CONTRIBUTING.md|LICENSE*|docs/*)
      ;;
    *)
      contract=true
      coordinator=true
      runtime=true
      ;;
  esac
done

printf 'contract=%s\ncoordinator=%s\nruntime=%s\n' "$contract" "$coordinator" "$runtime"
