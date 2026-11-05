Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-conditional-precedence-and-lifecycle-headers-get
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-conditional-precedence-and-lifecycle-headers-get|run#18|AssertionError|assert ims_before_alone == (200, None, _BODY), (
Issue-Commit: defa25c70e8324a13a1647a7aa41447c10b54fda

# s3 GetObject with If-Modified-Since is refused 501 NotImplemented instead of being evaluated

> _Note: this issue was identified against the code base at commit `defa25c70e8324a13a1647a7aa41447c10b54fda`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
REOPENS 20261104T021914Z-s3-copyobject-header-validation-and-conditional-getobject-refusals.md: its fix left the call refused.

That issue's cause 7 covered conditional GetObject refusals (an If-Unmodified-Since HTTP-date parsed with an RFC-3339 parser and answered ValidationException). The call is still refused, now with a different refusal: the If-Modified-Since branch of GetObject answers 501 NotImplemented. Two open issues describe neighbouring conditional-read faults — 20261025T032458Z-s3-object-plane-10-root-causes-conditional-requests.md and 20261103T054236Z-s3-object-read-request-validation-diverges-partnumber-sse.md — but the replay reports this check unowned by either, so it is filed here.

Cause 1 of 1: GetObject carrying only an If-Modified-Since header is refused instead of evaluated.

AWS: Smithy: GetObject.IfModifiedSince is httpHeader 'If-Modified-Since' of type timestamp, so the REST binding formats/parses it as HTTP-date (IMF-fixdate); GetObject's only modeled errors are NoSuchKey (404) and InvalidObjectState (403), i.e. the model binds no error to this header at all. Recorded behavior: when the object's Last-Modified is LATER than the supplied instant the precondition holds and S3 serves the object normally — HTTP 200 with the full body and the object's usual response headers (ETag, Last-Modified, Content-Length); when it is not later, S3 answers 304 Not Modified with an empty body. The refusal S3 uses for the opposite family (If-Match / If-Unmodified-Since) is 412 with Code 'PreconditionFailed' and Message 'At least one of the pre-conditions you specified did not hold' — that sentence is fixed text — plus a <Condition> member naming the header that failed. A 501 is never part of this surface.

Emulator: HTTP 501, Error.Code 'NotImplemented', Error.Message 'operation GetObject with If-Modified-Since is scaffolded but not implemented in this emulator'. The fixed text of that message is the emulator's own prose; its variable parts are the operation name and the header name. No body and no conditional evaluation occur, so a plain read of an existing, readable object fails.

Why: every SDK and HTTP cache sends If-Modified-Since for revalidation. A client doing a conditional read cannot tell "not modified" (304, reuse the cached copy) from "modified" (200, take the new bytes) because neither is produced — it gets a server-side 501 that boto3 surfaces as NotImplemented, which clients treat as a broken endpoint rather than a cache-validation result. Conditional reads, cache revalidation and the precedence rules between the conditional headers cannot work at all.

Fix: the conditional-header evaluation in the GetObject handler — decode the httpHeader-bound timestamp with the http-date format and compare it against the stored object's Last-Modified to produce 200-with-body or a bodiless 304, with the If-Match/If-Unmodified-Since family producing the 412 PreconditionFailed envelope with Condition. No path through that handler should emit a NotImplemented scaffold refusal.

Traces: s3.s3-conditional-precedence-and-lifecycle-headers-get — the check asserts If-Modified-Since = LastModified-1s alone returns (200, None, body) and observed (501, 'NotImplemented', 'operation GetObject with If-Modified-Since is scaffolded but not implemented in this emulator').

## Evidence
Behavioral replay, check s3.s3-conditional-precedence-and-lifecycle-headers-get (verdict: mismatch):

  File "check_s3_s3_conditional_precedence_and_lifecycle_headers_get.py", line 101, in run
    assert ims_before_alone == (200, None, _BODY), (
  AssertionError: If-Modified-Since=LastModified-1s alone must return the body (the object WAS modified after that instant); got (501, 'NotImplemented', 'operation GetObject with If-Modified-Since is scaffolded but not implemented in this emulator')

Expected (AWS): 200, no error code, the object's bytes in the body.
Actual (emulator): 501, Error.Code 'NotImplemented', Error.Message 'operation GetObject with If-Modified-Since is scaffolded but not implemented in this emulator'.

The complementary case the same check exercises (If-Modified-Since at or after Last-Modified) must be a bodiless 304; it is unreachable while the header is refused.

## Steps to Reproduce
1. Create a bucket and PutObject a small body at a key.
2. HeadObject the key and read its LastModified.
3. GetObject the key with IfModifiedSince set to LastModified minus one second.
4. AWS returns 200 with the full body. The emulator returns 501 NotImplemented with Message 'operation GetObject with If-Modified-Since is scaffolded but not implemented in this emulator'.
5. Repeat with IfModifiedSince set to LastModified: AWS returns a bodiless 304 Not Modified.
6. Re-replay behavioral check s3.s3-conditional-precedence-and-lifecycle-headers-get to confirm.
