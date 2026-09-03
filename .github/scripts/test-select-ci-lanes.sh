#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
selector="$script_dir/select-ci-lanes.sh"

assert_lanes() {
  local expected=$1
  shift
  local actual
  actual="$(bash "$selector" "$@")"
  if [[ "$actual" != "$expected" ]]; then
    printf 'classification mismatch for paths:' >&2
    printf ' %q' "$@" >&2
    printf '\nexpected:\n%s\nactual:\n%s\n' "$expected" "$actual" >&2
    return 1
  fi
}

contract_only=$'contract=true\ncoordinator=false\nruntime=false'
contract_runtime=$'contract=true\ncoordinator=false\nruntime=true'
coordinator_only=$'contract=false\ncoordinator=true\nruntime=false'
runtime_only=$'contract=false\ncoordinator=false\nruntime=true'
all=$'contract=true\ncoordinator=true\nruntime=true'
baseline_only=$'contract=false\ncoordinator=false\nruntime=false'

assert_lanes "$contract_runtime" src/contracts/model.rs
assert_lanes "$contract_only" schemas/agent-run/v0.1.0/agent-run-request.schema.json
assert_lanes "$contract_only" examples/generate-contract-artifacts/catalog.rs
assert_lanes "$coordinator_only" src/main.rs
assert_lanes "$coordinator_only" evidence/run-invariant-subject-v0.1.0.json
assert_lanes "$runtime_only" src/runtime.rs
assert_lanes "$runtime_only" tests/runtime.rs
assert_lanes "$runtime_only" docs/adr/0005-bounded-agent-run-engine.md
assert_lanes "$all" src/contracts/model.rs src/main.rs
assert_lanes "$all" .github/workflows/ci.yml
assert_lanes "$all" Cargo.lock
assert_lanes "$all" src/future-runtime.rs
assert_lanes "$all" --uncertain
assert_lanes "$baseline_only" README.md
assert_lanes "$baseline_only" docs/research/new-note.md

printf 'CI lane classification invariants passed\n'
