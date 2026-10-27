Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.presigned-put-requires-every-signed-header-and-refuses-an-over-week-expiry s3.sigv4skew-presigned-get-altered-expires-param-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.presigned-put-requires-every-signed-header-and-refuses-an-over-week-expiry|run#28|AssertionError|assert status == 400, (
Issue-Key: s3.sigv4skew-presigned-get-altered-expires-param-rejected|run#23|KeyError|assert f"X-Amz-Expires={widened}" in err["CanonicalRequest"], (
Issue-Commit: adc01e2549f42bfb6ad800e861ab78e027b599f0

# s3: 2 net-new root causes in SigV4 query (presigned) authentication — the one-week X-Amz-Expires ceiling is not enforced as AuthorizationQueryParametersError/400, and the SignatureDoesNotMatch body omits its canonical-request diagnostics

> _Note: this issue was identified against the code base at commit `adc01e2549f42bfb6ad800e861ab78e027b599f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Two distinct net-new root causes in the S3 front door's SigV4 query-string (presigned URL) authentication path. Both are numbered below with their own failing check ids; BOTH must be closed before this issue is submitted.

Note on scope vs. already-open work: the open issue 20261025T032458Z-s3-request-auth-and-eventing-plane-10-root.md (Cause 3) covers presigned/virtual-hosted signature *verification* failing outright. That part now works — in the replay, the presigned PUT legs of s3.presigned-put-requires-every-signed-header-and-refuses-an-over-week-expiry all pass (the signed-header-bearing PUT is 200, the header-less PUT is refused 403 SignatureDoesNotMatch with the exact AWS message, the object is absent, and resending the same URL with the header restored succeeds). What fails now is the two causes below, which that issue does not describe. Neither check is marked OWNED or ALREADY FILED in the replay report.

Cause 1 of 2: the SigV4 query-authentication one-week ceiling on X-Amz-Expires is not enforced; the refusal is the wrong status, the wrong error code and a non-AWS message.
AWS: SigV4 query-string authentication caps the validity window at 604800 seconds (one week). A presigned URL carrying X-Amz-Expires greater than 604800 is refused with HTTP 400 and error code AuthorizationQueryParametersError, message "X-Amz-Expires must be less than a week (in seconds); that is, the given X-Amz-Expires must be less than 604800 seconds". Every part of that message is fixed text, including the 604800 constant — nothing in it comes from the request; the request's own over-limit value is not echoed. Smithy: s3#GetObject models only NoSuchKey (404) and InvalidObjectState (403); SigV4 query-auth errors are protocol-level and are not in the model, so this is AWS's documented SigV4 query-string-auth behavior plus recorded behavior (reference used by the check: https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-query-string-auth.html).
Emulator: a presigned GET with X-Amz-Expires=604801 is answered HTTP 403 with code AccessDenied and the emulator-invented message "Sign the request with the emulator credentials" (an in-window presigned GET for the same key on the same signer answers 200, so the refusal is reached only on the over-limit value). The status class, the error code and the message all differ from AWS.
Why: a client that distinguishes a malformed query-authentication request (400, non-retryable, fix the URL) from an authorization failure (403, re-sign or re-authorize) takes the wrong branch, and any code asserting on AuthorizationQueryParametersError never sees it. The invented message also cannot be matched by callers that key off AWS's fixed wording.
Fix: the S3 front door's SigV4 query-string authentication layer — validate X-Amz-Expires against the 604800 s ceiling before credential/signature evaluation and emit AuthorizationQueryParametersError/400 with AWS's fixed message; also stop emitting the non-AWS "Sign the request with the emulator credentials" text on this path.
Traces: s3.presigned-put-requires-every-signed-header-and-refuses-an-over-week-expiry (fails at `assert status == 400`).

Cause 2 of 2: the SignatureDoesNotMatch error body omits the SigV4 diagnostic elements (CanonicalRequest, SignatureProvided, and the rest of that element set).
AWS: S3's SignatureDoesNotMatch refusal is an <Error> document that carries, in addition to Code and Message, the signing diagnostics AWSAccessKeyId, StringToSign, StringToSignBytes, CanonicalRequest, CanonicalRequestBytes and SignatureProvided. CanonicalRequest is the canonical request S3 itself built, so for a tampered presigned URL it contains the tampered query parameter as received (e.g. the widened X-Amz-Expires value), and SignatureProvided is the X-Amz-Signature value the URL still carries unchanged. The element names and their meaning are fixed; their contents are derived from the request as received. Smithy: these are protocol-level S3 REST error elements, not modeled shapes — recorded behavior only.
Emulator: for a presigned GET whose signed X-Amz-Expires was widened while every other query pair was left untouched, the emulator answers correctly with HTTP 403, Code SignatureDoesNotMatch and the exact AWS message "The request signature we calculated does not match the signature you provided. Check your key and signing method.", and serves no object bytes — but the <Error> document contains no CanonicalRequest element at all, so reading it raises `KeyError: 'CanonicalRequest'`; SignatureProvided is likewise not available to be compared against the URL's X-Amz-Signature.
Why: those elements are the only way a client (or a human debugging an SDK/presigner) can tell WHICH canonical request the service signed and which signature it compared against. Without them, every signing mismatch is indistinguishable from a wrong secret key, and tooling that parses S3's canonical-request echo to diagnose signing bugs gets nothing.
Fix: the S3 SigV4 verifier's SignatureDoesNotMatch error serializer — populate AWSAccessKeyId, StringToSign/StringToSignBytes, CanonicalRequest/CanonicalRequestBytes and SignatureProvided from the request as received.
Traces: s3.sigv4skew-presigned-get-altered-expires-param-rejected (fails at `assert f"X-Amz-Expires={widened}" in err["CanonicalRequest"]`).

## Evidence
Cause 1 — s3.presigned-put-requires-every-signed-header-and-refuses-an-over-week-expiry, [mismatch]:
`AssertionError: a presigned GET with X-Amz-Expires=604801 returned 403, expected 400 — SigV4 query authentication caps the window at 604800s, and a target that ignores the cap issues effectively permanent URLs: b'<?xml version="1.0"?><Error><Code>AccessDenied</Code><Message>Sign the request with the emulator credentials</Message><RequestId>…</RequestId><HostId>…'`
Expected: HTTP 400, Code AuthorizationQueryParametersError, Message "X-Amz-Expires must be less than a week (in seconds); that is, the given X-Amz-Expires must be less than 604800 seconds" (all fixed text). Actual: HTTP 403, Code AccessDenied, Message "Sign the request with the emulator credentials" (not an AWS message). The request id / host id in the body are per-run values and are not part of the requirement.
Preceding legs of the same check PASSED, which localises the defect: the presigned PUT resending every signed header returned 200 and stored AES256; the presigned PUT omitting the signed x-amz-server-side-encryption header returned 403 SignatureDoesNotMatch with AWS's exact message and created nothing; the same URL with the header restored returned 200; and the in-window presigned GET returned 200 with the object's bytes.

Cause 2 — s3.sigv4skew-presigned-get-altered-expires-param-rejected, [error]:
`KeyError: 'CanonicalRequest'` at `assert f"X-Amz-Expires={widened}" in err["CanonicalRequest"]`.
Before that point the check asserted and PASSED: untampered presigned GET → 200 with the payload; presigned GET with only X-Amz-Expires widened (300 → 600) → HTTP 403, Code SignatureDoesNotMatch, Message "The request signature we calculated does not match the signature you provided. Check your key and signing method.", no object bytes in the body. Expected additionally: an <Error> document carrying CanonicalRequest (containing `X-Amz-Expires=600`, i.e. the tampered value as received) and SignatureProvided (equal to the URL's unchanged X-Amz-Signature). Actual: those elements are absent from the body.

## Steps to Reproduce
Boot the emulator and replay the two check ids in trace_ids.

Manual repro, Cause 1: create a bucket, PUT an object, then build a presigned GET with botocore (`Config(signature_version="s3v4")`, `generate_presigned_url("get_object", ..., ExpiresIn=604801)`) and fetch the URL with a raw HTTP client. Expect HTTP 400 with <Code>AuthorizationQueryParametersError</Code> and the fixed message "X-Amz-Expires must be less than a week (in seconds); that is, the given X-Amz-Expires must be less than 604800 seconds"; observe HTTP 403 <Code>AccessDenied</Code> <Message>Sign the request with the emulator credentials</Message>. A presigned GET for the same key with ExpiresIn=900 returns 200, confirming the signer and the object are fine.

Manual repro, Cause 2: presign a GET with ExpiresIn=300, confirm it returns 200, then re-encode the SAME query with only the X-Amz-Expires pair changed to 600 (leave X-Amz-Signature and every other pair byte-identical) and fetch it. The 403 SignatureDoesNotMatch is correct; parse the <Error> body and look for <CanonicalRequest> (must contain `X-Amz-Expires=600`) and <SignatureProvided> (must equal the URL's X-Amz-Signature) — both elements are missing.
