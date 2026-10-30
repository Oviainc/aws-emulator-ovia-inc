Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.create-asset-file-rejected-when-artifact-already-has-its-one-file
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent.create-asset-file-rejected-when-artifact-already-has-its-one-file|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Commit: a12986f4241917f433abd9b7a93c682fd0b73a1b

# Implement devops-agent CreateAssetFile: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a12986f4241917f433abd9b7a93c682fd0b73a1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssetFile

## Description
`CreateAssetFile` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-devopsagent-devopsagent/src/ops/create_asset_file.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateAssetFile` call, so each one unblocks only when `CreateAssetFile` works: `devops-agent.create-asset-file-rejected-when-artifact-already-has-its-one-file`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-devopsagent-devopsagent/src/ops/create_asset_file.rs`. Replaying the corpus, the emulator refused the `CreateAssetFile` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_devops_agent_create_asset_file_rejected_when_artifact_already_has_its_one_file.py", line 56, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got NotImplemented: 'operation CreateAssetFile is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the devops-agent emulator
Call CreateAssetFile and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
