Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.untag-resource-nonexistent-bucket-rejected
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors.untag-resource-nonexistent-bucket-rejected|run#7|AssertionError|assert err["Code"] == "NotFoundException", f"expected NotFoundException untagging a deleted bucket, got {err!r}"
Issue-Commit: c1cdf4b562f81ba1ac84179127e5df6b90873b05

# Implement s3vectors UntagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c1cdf4b562f81ba1ac84179127e5df6b90873b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
`UntagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/untag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UntagResource` call, so each one unblocks only when `UntagResource` works: `s3vectors.untag-resource-nonexistent-bucket-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/untag_resource.rs`. Replaying the corpus, the emulator refused the `UntagResource` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_s3vectors_untag_resource_nonexistent_bucket_rejected.py", line 27, in run
    assert err["Code"] == "NotFoundException", f"expected NotFoundException untagging a deleted bucket, got {err!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected NotFoundException untagging a deleted bucket, got {'Message': 'operation UntagResource is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the s3vectors emulator
Call UntagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
