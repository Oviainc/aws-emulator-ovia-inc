Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.delete-object-annotation-error-paths
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.delete-object-annotation-error-paths|run#4|AssertionError|assert code_bucket == ("NoSuchBucket", "The specified bucket does not exist"), (
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# Implement s3 DeleteObjectAnnotation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteObjectAnnotation

## Description
`DeleteObjectAnnotation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/delete_object_annotation.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteObjectAnnotation` call, so each one unblocks only when `DeleteObjectAnnotation` works: `s3.delete-object-annotation-error-paths`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/delete_object_annotation.rs`. Replaying the corpus, the emulator refused the `DeleteObjectAnnotation` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_s3_delete_object_annotation_error_paths.py", line 27, in run
    assert code_bucket == ("NoSuchBucket", "The specified bucket does not exist"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected NoSuchBucket with AWS's exact message, got ('NotImplemented', 'operation DeleteObjectAnnotation is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the s3 emulator
Call DeleteObjectAnnotation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
