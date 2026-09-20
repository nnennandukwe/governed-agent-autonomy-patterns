# Verify the actual Goose Desktop session

The tested runtime is Goose Desktop 1.50.0 on macOS. A recipe declaring GAAP alone
is not sufficient to remove Desktop built-ins. The tagged upstream ACP
`initial_session_extensions` starts with selected default built-ins, then adds
recipe extensions. Its default selection includes Developer. The CLI recipe
resolution follows a different path. This explains why a CLI rehearsal can pass
while Desktop still exposes `edit`, `write`, and `shell`.

Sources: [ACP session extension initialization](https://github.com/aaif-goose/goose/blob/v1.50.0/crates/goose/src/acp/server.rs)
and [CLI extension resolution](https://github.com/aaif-goose/goose/blob/v1.50.0/crates/goose/src/config/extensions.rs).

## Manual session preparation

After trusting the recipe, before selecting any activity, use the extension
manager beside the chat input. Disable Developer and every extension except
GAAP for this session. Confirm only `gaap__read_project`, `gaap__submit_change`,
and `gaap__run_status` remain. Do not change global defaults. Recheck after
reopening, changing extensions, or upgrading Goose. The recipe declaration is
not evidence of the effective session capability set.

## Reproducible Desktop backend preflight

For exact machine-readable evidence, the included macOS diagnostic uses the same
local ACP API as Desktop. It creates a **new** session from the recipe, removes
Developer, checks the complete runtime tool inventory, challenges the now-absent
`edit` tool, reloads the session and checks again, then optionally sends the task.
This is a Desktop-backend reproduction, not a recording of GUI button clicks.
The `_goose/unstable` API is version-specific. Other runtime versions require a
fresh review. The script reads the chosen backend credential only in memory and
verifies TLS using Goose's local certificate. Do not publish raw session exports.

```sh
python3 -m venv .venv-desktop-check
.venv-desktop-check/bin/pip install websockets==15.0.1
ps -ax -o pid=,command= | grep '[g]oose serve.*platform desktop'
.venv-desktop-check/bin/python examples/goose-mcp/desktop_session_check.py \
  --pid YOUR_DESKTOP_BACKEND_PID \
  --recipe demo-runs/YOUR_FRESH_SESSION/desktop-recipe.json \
  --output demo-runs/YOUR_FRESH_SESSION/desktop-preflight --run
```

If several Desktop windows have backends, choose the intended one. Run this only
on a fresh fixture before sending the task in another session. A failed check
never sends the activity. Inspect the output evidence and the actual workspace,
then open the resulting session in Desktop history. Do not rerun the script to
resume: it deliberately creates another session.

This removes the alternate Developer path from this session; it is not OS
isolation. Another native process, another Goose session with file access, or an
operator can still edit the same local files. The GAAP Inspector cannot inventory
all host tools or prevent those outside writes.
