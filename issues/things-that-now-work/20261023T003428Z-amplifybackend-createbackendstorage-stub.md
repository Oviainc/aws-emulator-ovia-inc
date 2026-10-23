Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplifybackend.create-backend-storage-job-lifecycle
Issue-Shape-Id: com.amazonaws.amplifybackend#AmplifyBackend
Issue-Key: amplifybackend|CreateBackendStorage|NotImplemented
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Implement amplifybackend CreateBackendStorage: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackendStorage

## Description
`CreateBackendStorage` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/create_backend_storage.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateBackendStorage` call, so each one unblocks only when `CreateBackendStorage` works: `amplifybackend.create-backend-storage-job-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/create_backend_storage.rs`. Replaying the corpus, the emulator refused the `CreateBackendStorage` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_amplifybackend_create_backend_storage_job_lifecycle.py", line 72, in run
    assert error["Code"] == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException rejecting CreateBackendStorage via SDK/CLI, got NotImplemented: operation CreateBackendStorage is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the amplifybackend emulator
Call CreateBackendStorage and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
