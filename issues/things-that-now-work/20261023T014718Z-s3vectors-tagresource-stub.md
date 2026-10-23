Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.list-tags-for-deleted-resource-not-found s3vectors.tag-resource-conflict-under-concurrent-updates s3vectors.tag-resource-during-delete-conflict s3vectors.tag-resource-invalid-key-rejected s3vectors.tag-resource-nonexistent-bucket-rejected s3vectors.tag-resource-nonexistent-resource-not-found s3vectors.tag-resource-then-list-reflects s3vectors.tag-value-empty-string-preserved s3vectors.tag-value-unicode-round-trip s3vectors.untag-resource-missing-resource-not-found s3vectors.untag-resource-removes-tag s3vectors.untag-resource-removes-tag-from-list
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|TagResource|NotImplemented
Issue-Key: s3vectors.tag-resource-during-delete-conflict|run#14|AssertionError|assert tag_result["code"] == "NotFoundException", (
Issue-Key: s3vectors.tag-resource-invalid-key-rejected|run#7|AssertionError|assert err["Code"] == "ValidationException", f"expected ValidationException for reserved key, got {err!r}"
Issue-Key: s3vectors.tag-resource-nonexistent-bucket-rejected|run#7|AssertionError|assert err["Code"] == "NotFoundException", f"expected NotFoundException tagging a deleted bucket, got {err!r}"
Issue-Key: s3vectors.tag-resource-nonexistent-resource-not-found|run#5|AssertionError|assert err["Code"] == "NotFoundException", f"expected NotFoundException tagging a ghost ARN, got {err!r}"
Issue-Commit: c1cdf4b562f81ba1ac84179127e5df6b90873b05

# Implement s3vectors TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c1cdf4b562f81ba1ac84179127e5df6b90873b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `s3vectors.list-tags-for-deleted-resource-not-found`, `s3vectors.tag-resource-conflict-under-concurrent-updates`, `s3vectors.tag-resource-during-delete-conflict`, `s3vectors.tag-resource-invalid-key-rejected`, `s3vectors.tag-resource-nonexistent-bucket-rejected`, `s3vectors.tag-resource-nonexistent-resource-not-found`, `s3vectors.tag-resource-then-list-reflects`, `s3vectors.tag-value-empty-string-preserved`, `s3vectors.tag-value-unicode-round-trip`, `s3vectors.untag-resource-missing-resource-not-found`, `s3vectors.untag-resource-removes-tag`, `s3vectors.untag-resource-removes-tag-from-list`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3vectors emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
