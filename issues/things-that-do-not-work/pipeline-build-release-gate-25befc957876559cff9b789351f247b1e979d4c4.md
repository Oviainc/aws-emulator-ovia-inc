---
Issue-Source: pipeline-build
Issue-Commit: 25befc957876559cff9b789351f247b1e979d4c4
Issue-Severity: HIGH
---

# Pipeline ReleaseGate stage failed

The tenant CodePipeline stage failed for pipeline-infrastructure reasons; this is
not a release-gate refusal (the gate files its own issues when it refuses).

- Stage: `ReleaseGate`
- Execution: `85604402-c066-425c-9f7e-eab9f5e2e24a`
- Commit: `25befc957876559cff9b789351f247b1e979d4c4`

## CodePipeline error details

The action failed before its build produced a log stream.

````text
Build terminated with state: FAILED. Phase: BUILD, Code: COMMAND_EXECUTION_ERROR, Message: Error while executing command: set -euo pipefail
if [ -n "${RELEASE_VERSION_MODE:-}" ]; then
  # tee keeps the script's whole output in the build log; its last line is the version.
  RELEASE_VERSION="$(python3 /tmp/release_gate.py | tee /dev/stderr | tail -n 1)"
  export RELEASE_VERSION
else
  python3 /tmp/release_gate.py
fi
. Reason: exit status 1
````
