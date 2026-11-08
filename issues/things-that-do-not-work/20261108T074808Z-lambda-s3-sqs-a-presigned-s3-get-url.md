Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative
Issue-Key: lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative|run#12|AssertionError|raise AssertionError("expected fetching the presigned URL to be denied (AccessDenied), but it succeeded")
Issue-Commit: 3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e

# lambda__s3__sqs: a presigned S3 GET URL signed by a role without s3:GetObject is served 200 instead of being refused 403 AccessDenied (1 cause)

> _Note: this issue was identified against the code base at commit `3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: S3 does not authorize a presigned GET URL against the signing identity's permissions. A URL signed inside a Lambda whose execution role holds no S3 statements at all is fetched successfully. Failing check: lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative.

AWS: recorded behavior only for the presigning mechanics — `generate_presigned_url` is a client-side SDK operation and is not a modelled call, so signing always succeeds locally regardless of permissions. The authorization happens when the URL is fetched: S3 evaluates the request as the signing principal, so a URL signed by a role lacking s3:GetObject on the key is refused HTTP 403 with an XML error whose Code is `AccessDenied`. The fixed text is the error Code `AccessDenied` and the 403 status; the bucket, key and principal are values from the request. The control case — the same object and the same code behind a role that does hold s3:GetObject — is served 200.

Emulator: the fetch succeeds. The replay signs the URL inside a function whose role carries only the Lambda basic-execution policy (logs only), fetches it, and the GET returns a 2xx body, so the check raises `AssertionError: expected fetching the presigned URL to be denied (AccessDenied), but it succeeded`.

Why: presigned URLs are the standard way to hand out scoped, time-limited object access, and their whole security property is that the signer cannot grant more than it holds. In the emulator any identity that can sign can read any object, so a least-privilege design is silently ineffective and a client cannot test that a narrowed role actually narrows what its presigned URLs can fetch.

Fix: the S3 GetObject request-authorization path for presigned (query-string-signed) requests — it must resolve the signing principal from the signature and evaluate its identity policy (and the bucket policy) before serving the object, refusing 403 AccessDenied when the action is not allowed.

Traces: lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative.

## Evidence
Expected (AWS): fetching a presigned GET URL signed by a role with no s3:GetObject grant is refused with HTTP 403 and an S3 error body containing Code `AccessDenied`; the identical URL signed by a role WITH s3:GetObject is served 200 with the object bytes.

Actual (emulator): the unauthorized presigned URL is served successfully — `AssertionError: expected fetching the presigned URL to be denied (AccessDenied), but it succeeded`. No 403 and no AccessDenied code is produced.

## Steps to Reproduce
1. Create a bucket and PutObject a key.
2. Create a Lambda whose execution role has only the basic-execution (logs) policy and no S3 statements; its handler returns `s3.generate_presigned_url('get_object', Params={'Bucket':..., 'Key':...}, ExpiresIn=300)`.
3. Invoke it and fetch the returned URL. Expect HTTP 403 with AccessDenied; observe a successful 200 fetch.
4. As a control, repeat with a role that holds s3:GetObject — that fetch must succeed.
5. Replay lambda__s3__sqs.lambda-sdk-s3-presigned-url-missing-permission-negative.
