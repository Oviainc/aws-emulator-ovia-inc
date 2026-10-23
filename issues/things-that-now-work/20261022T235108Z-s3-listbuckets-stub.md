Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: _smoke.sdk-connects-with-dummy-creds
Issue-Key: _smoke.sdk-connects-with-dummy-creds|run#3|AssertionError|assert not failures, "drop-in SDK failed for:\n " + "\n ".join(failures)
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement s3 ListBuckets: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBuckets

## Description
`ListBuckets` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/list_buckets.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListBuckets` call, so each one unblocks only when `ListBuckets` works: `_smoke.sdk-connects-with-dummy-creds`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/list_buckets.rs`. Replaying the corpus, the emulator refused the `ListBuckets` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_smoke_sdk_connects_with_dummy_creds.py", line 48, in run
    assert not failures, "drop-in SDK failed for:\n  " + "\n  ".join(failures)
           ^^^^^^^^^^^^
AssertionError: drop-in SDK failed for:
  s3.list_buckets: ClientError: An error occurred (NotImplemented) when calling the ListBuckets operation: operation ListBuckets is 
  iam.list_roles: ClientError: An error occurred (NotImplemented) when calling the ListRoles operation: operation ListRoles is scaf

## Steps to Reproduce
Boot the s3 emulator
Call ListBuckets and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
