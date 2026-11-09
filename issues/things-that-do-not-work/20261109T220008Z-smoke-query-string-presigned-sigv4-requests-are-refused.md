Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: _smoke.sigv4-query-presigned-requests-identify-the-account
Issue-Key: _smoke.sigv4-query-presigned-requests-identify-the-account|run#20|AssertionError|assert status == 200, f"presigned S3 GetObject: expected HTTP 200, got {status}: {body[:200]!r}"
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# _smoke: query-string (presigned) SigV4 requests are refused 403 AccessDenied, so a presigned S3 GetObject never serves the object

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One cause.

Cause 1 of 1: a SigV4 request authenticated by query string (a presigned URL) is refused instead of authorized.

AWS: S3 accepts SigV4 in the query string (X-Amz-Algorithm/X-Amz-Credential/X-Amz-Date/X-Amz-Expires/X-Amz-SignedHeaders/X-Amz-Signature) exactly as it accepts the Authorization header; a valid, unexpired presigned GET of an existing object answers HTTP 200 with the object's bytes (https://docs.aws.amazon.com/IAM/latest/UserGuide/create-signed-request.html). Smithy: s3 GetObject is GET /{Bucket}/{Key+} with authSchemes sigv4 and models no separate query-auth form — the signing location is a protocol concern, not a per-operation one. A refusal of a correctly presigned URL is not a modeled outcome of GetObject at all.

Emulator: the presigned S3 GET answers HTTP 403 with <Error><Code>AccessDenied</Code><Message>Sign the request with the emulator credentials</Message>. Fixed text of that message is 'Sign the request with the emulator credentials'; nothing in it is derived from the request, i.e. the emulator never evaluated the query-string signature it was given. The presigned STS GetCallerIdentity GET in the same check does answer 200, so the gap is in the S3 request authenticator, not in presign support generally.

Why: every browser-upload/download pattern breaks. A client that hands a presigned URL to a browser, a CDN origin fetch, or any tool that cannot set headers gets 403 AccessDenied where AWS serves the bytes, and the AccessDenied wording points the user at credentials they are deliberately not sending.

Fix: the S3 request-authentication/signature-verification layer (the SigV4 verifier that today only reads the Authorization header) must also accept and verify the X-Amz-* query-string presentation, including X-Amz-Expires expiry, before the GetObject handler runs.

Traces: _smoke.sigv4-query-presigned-requests-identify-the-account. The same 403 'Sign the request with the emulator credentials' body appears on the s3 TLS GET in s3.ssl-deny-bucket-policy-plain-http-head-refusal-carries-no-body, which is owned by another issue; this issue is grounded only in the _smoke check.

Note: this check is listed under the suite heading '_smoke', which is not an AWS service token; the operations it exercises belong to s3 (and sts).

## Evidence
Expected: presigned S3 GetObject -> HTTP 200, body == the bytes PutObject wrote (1024+ random bytes).
Actual: HTTP 403, body b'<?xml version="1.0"?><Error><Code>AccessDenied</Code><Message>Sign the request with the emulator credentials</Message><RequestId>...</RequestId><HostId>...</HostId></Error>'.
AssertionError: presigned S3 GetObject: expected HTTP 200, got 403.
Control in the same check: presigned STS GetCallerIdentity GET answered 200 and named the caller's account/ARN/UserId correctly.

## Steps to Reproduce
1. sts.get_caller_identity() and a presigned GET of get_caller_identity both succeed (control).
2. s3.create_bucket(Bucket=b) (with CreateBucketConfiguration.LocationConstraint when region != us-east-1).
3. s3.put_object(Bucket=b, Key='presigned/object.bin', Body=<1KiB random>).
4. url = s3.generate_presigned_url('get_object', Params={'Bucket': b, 'Key': 'presigned/object.bin'}, ExpiresIn=300).
5. Plain HTTP GET of url -> observe 403 AccessDenied instead of 200 with the object body.
