Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.multipart-abort-no-such-upload-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.multipart-abort-no-such-upload-rejected|run#10|AssertionError|assert code == "NoSuchUpload", f"expected NoSuchUpload for a fabricated uploadId, got {code}"
Issue-Commit: e7a2f2386a844500e403d56aecba07370542f5da

# S3 AbortMultipartUpload is an unimplemented scaffold stub: every abort answers 501 NotImplemented, so a fabricated uploadId is never refused with 404 NoSuchUpload and a real in-flight upload can never be aborted

> _Note: this issue was identified against the code base at commit `e7a2f2386a844500e403d56aecba07370542f5da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AbortMultipartUpload

## Description
One net-new root cause.

Cause 1 of 1: `AbortMultipartUpload` is still a scaffold stub in the S3 crate, so it answers HTTP 501 `NotImplemented` for EVERY request — both the rejection path (an uploadId that was never issued) and the happy path (aborting an upload the emulator itself handed out via CreateMultipartUpload). Failing check: `s3.multipart-abort-no-such-upload-rejected`.

AWS: `AbortMultipartUpload` is modeled with `http {method: DELETE, uri: /{Bucket}/{Key+}?x-id=AbortMultipartUpload, code: 204}` and a single modeled error, `NoSuchUpload` (`errorKind: client`, `httpError: 404`, doc "The specified multipart upload does not exist."). Required input members are `Bucket` (httpLabel), `Key` (httpLabel, @length min 1) and `UploadId` (httpQuery `uploadId`). So real AWS has exactly two outcomes for a well-formed request: (a) the uploadId names an in-flight multipart upload on that bucket/key — the upload is discarded, its parts' storage is freed, and the response is `204 No Content` with no body (optionally `x-amz-request-charged`); (b) the uploadId was never issued, or was already aborted or completed — the response is `404` with error code `NoSuchUpload`. For the 404 class the recorded response message is the fixed text `The specified upload does not exist. The upload ID may be invalid, or the upload may have been aborted or completed.` — this message is entirely fixed text for this error class, with no request- or state-derived substitutions (the offending uploadId and key are NOT interpolated into it). The same `NoSuchUpload` code/message class covers a subsequent `ListParts` against an id that has been aborted. Per the model, the abort must also be scoped to the named uploadId only: other in-flight uploads on the same bucket stay listed by `ListMultipartUploads`.

Emulator: the handler at services/service-com-amazonaws-s3-amazons3/src/ops/abort_multipart_upload.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("AbortMultipartUpload"))`, so the input is never looked at. The replay shows `AbortMultipartUpload` on a fabricated uploadId answering error code `NotImplemented` with message `operation AbortMultipartUpload is scaffolded but not implemented in this emulator` (HTTP 501) where `NoSuchUpload` (HTTP 404) is required — the check fails at `assert code == "NoSuchUpload" ... got NotImplemented`. Because the refusal is unconditional, the check never reaches its later assertions, so the happy-path abort (204, upload removed from `ListMultipartUploads`) and the post-abort `ListParts` → `NoSuchUpload` contract are both unreachable too.

Why: a client cannot release an abandoned multipart upload at all — SDK/CLI cleanup paths (`aws s3api abort-multipart-upload`, the high-level transfer managers' abort-on-failure, lifecycle tooling that drains in-progress uploads before deleting a bucket) see a 501 instead of a 204 and treat the emulator as broken rather than completing teardown, leaving uploads permanently listed. Error-handling code is misled in the other direction as well: a retry/idempotency wrapper that distinguishes "this uploadId is dead" (`NoSuchUpload`, a terminal, expected 404 it should swallow) from a server fault sees a 501 `NotImplemented` and escalates or retries forever. Any negative test that asserts a fabricated or already-completed uploadId is refused with `NoSuchUpload` cannot pass.

Fix: implement the operation in its own handler, services/service-com-amazonaws-s3-amazons3/src/ops/abort_multipart_upload.rs — resolve `(Bucket, Key, UploadId)` against the multipart-upload records in the crate's state module, remove the upload and its parts and return the modeled 204 output on a hit, and return `Err(AbortMultipartUploadErrorModeled::NoSuchUpload(..).into())` with AWS's fixed `NoSuchUpload` message on a miss (never-issued, already aborted, or already completed). Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line once it answers. The same multipart state lookup should back `ListParts`'s `NoSuchUpload` refusal for a dead uploadId.

NOT covered by the already-open `20261025T160428Z-s3-completemultipartupload-stub.md` (CompleteMultipartUpload) or `20261025T160428Z-s3-multipart-analytics-3-net-new-root-causes.md` (UploadPart checksum members, analytics configurations): neither names `AbortMultipartUpload`, and the replay report did not mark this check as ALREADY FILED or OWNED, so no auto-filed stub issue exists for this operation.

Traces: s3.multipart-abort-no-such-upload-rejected.

## Evidence
Check `s3.multipart-abort-no-such-upload-rejected` (verdict: mismatch).

Expected (real AWS): AbortMultipartUpload(Bucket=<bucket>, Key="never-uploaded.bin", UploadId=<fabricated id>) -> HTTP 404, Error.Code = "NoSuchUpload", Error.Message = "The specified upload does not exist. The upload ID may be invalid, or the upload may have been aborted or completed." (all fixed text for this error class; no request/state values are substituted). Then AbortMultipartUpload against the REAL uploadId returned by CreateMultipartUpload -> HTTP 204, and that upload disappears from ListMultipartUploads while nothing else does; a following ListParts on the aborted id -> 404 NoSuchUpload with the same message.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation AbortMultipartUpload is scaffolded but not implemented in this emulator". Replay assertion: `AssertionError: expected NoSuchUpload for a fabricated uploadId, got NotImplemented` (check_s3_multipart_abort_no_such_upload_rejected.py line 95). The refusal is unconditional, so the 204 happy path and the post-abort ListParts refusal are never exercised.

Source of the defect: services/service-com-amazonaws-s3-amazons3/src/ops/abort_multipart_upload.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:`) and line 217 (`Err(crate::scaffolded::Scaffolded::not_implemented("AbortMultipartUpload"))`).

Model citation: Smithy com.amazonaws.s3#AbortMultipartUpload — http {method DELETE, code 204}; errors [NoSuchUpload (client, httpError 404)]; required members Bucket (httpLabel), Key (httpLabel, @length min 1), UploadId (httpQuery "uploadId"); output member RequestCharged (header x-amz-request-charged). com.amazonaws.s3#NoSuchUpload — structure, errorKind client, httpError 404, doc "The specified multipart upload does not exist."

## Steps to Reproduce
1. Create a general-purpose bucket (with CreateBucketConfiguration.LocationConstraint when the region is not us-east-1).
2. CreateMultipartUpload(Bucket=<bucket>, Key="real-upload.bin") and keep the returned UploadId; confirm ListMultipartUploads(Bucket=<bucket>) lists exactly that upload.
3. Call AbortMultipartUpload(Bucket=<bucket>, Key="never-uploaded.bin", UploadId="sim2real-behav-fake-upload-id-0000000000") — an id the emulator never issued.
   Observed: HTTP 501 with Error.Code "NotImplemented".
   Expected: HTTP 404 with Error.Code "NoSuchUpload" and message "The specified upload does not exist. The upload ID may be invalid, or the upload may have been aborted or completed."
4. Call AbortMultipartUpload(Bucket=<bucket>, Key="real-upload.bin", UploadId=<the real UploadId from step 2>).
   Observed: HTTP 501 "NotImplemented".
   Expected: HTTP 204 with no body, after which ListMultipartUploads(Bucket=<bucket>) returns no uploads.
5. Call ListParts(Bucket=<bucket>, Key="real-upload.bin", UploadId=<the now-aborted UploadId>).
   Expected: HTTP 404 NoSuchUpload with the same fixed message.
