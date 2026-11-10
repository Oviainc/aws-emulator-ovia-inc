Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok
Issue-Key: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok|run#18|AssertionError|assert codes == [403] * _BURST, (
Issue-Commit: 6341625b8db4f92d4da751a42125e115b3e2369c

# cloudfront__cloudwatch__sns__sqs: S3 GetObject answers 404 NoSuchKey to a requester without s3:ListBucket where AWS answers 403 AccessDenied, so edge 4xx traffic never produces the 403 the alarm check drives

> _Note: this issue was identified against the code base at commit `6341625b8db4f92d4da751a42125e115b3e2369c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: S3 selects the key-miss error without evaluating whether the requester holds s3:ListBucket on the bucket, so a missing key is disclosed as 404 NoSuchKey to a principal that was granted s3:GetObject only. Failing check: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok.

AWS: documented S3 authorization behavior for GetObject on a key that does not exist — S3 answers 403 with error code AccessDenied (message 'Access Denied') when the requester does NOT hold s3:ListBucket on the bucket, and answers 404 NoSuchKey ('The specified key does not exist.') only when it DOES hold s3:ListBucket. Smithy: GetObject models only NoSuchKey (httpError 404) and InvalidObjectState (403); AccessDenied is S3's service-common authorization refusal and is not an operation-modeled error, so this is the documented authorization rule plus recorded behavior. The recorded run exercises exactly that rule: the origin bucket policy grants cloudfront.amazonaws.com s3:GetObject and deliberately NOT s3:ListBucket, so every OAC-signed origin fetch for a key that is absent must come back through the distribution as HTTP 403.

Emulator: the same request is answered 404. The check drives five viewer GETs of a key that was never written and asserts each answers 403; the emulator answered [404, 404, 404, 404, 404] ('AssertionError: expected every missing-key request through the edge to answer 403, got [404, 404, 404, 404, 404]'). Fixed text of the correct answer: HTTP 403, Code 'AccessDenied', Message 'Access Denied'; the per-run values (bucket, key, request id, host id) come from the request and the emulator's state and are not part of the requirement. The emulator instead serves the 404 body whose fixed text is Code 'NoSuchKey', Message 'The specified key does not exist.', with Key taken from the request.

Why: a client relying on AWS's behavior gets two things wrong. First, the canonical private-bucket-behind-CloudFront diagnosis is inverted: a caller debugging a broken URL is told the key does not exist, when AWS tells it the principal is not authorized, and object/key existence is disclosed to a principal with no list permission. Second, every downstream contract built on the 4xx class of the origin answer breaks: here the AWS/CloudFront 4xxErrorRate alarm scenario cannot even start, because the burst meant to drive the metric above its threshold never produces the 403 AWS produces.

Fix: the s3 GetObject handler's authorization/error selection — the key-miss path must consult the bucket policy for s3:ListBucket on the bucket and answer 403 AccessDenied when it is not granted, falling back to 404 NoSuchKey only when it is. Related open issue 20261108T031654Z-cloudfront-dynamodb-lambda-s3-stepfuncti-cloudfront-dynamodb-lambda-s3-stepfunctions-getobject-for-a.md describes the same rule from a different check and does not carry this check's replay key.

Traces: cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok.

## Evidence
[mismatch] cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok
Error: File "check_cloudfront__cloudwatch__sns__sqs_obs_cloudfront_2_4xx_error_rate_alarm_notifies_sqs_untouched_control_stays_ok.py", line 343, in run
    assert codes == [403] * _BURST
AssertionError: expected every missing-key request through the edge to answer 403, got [404, 404, 404, 404, 404]

Expected: each viewer GET of a key that does not exist, through a distribution whose OAC-signed origin bucket policy grants only s3:GetObject (no s3:ListBucket), is answered HTTP 403 with <Code>AccessDenied</Code> / <Message>Access Denied</Message>.
Actual: HTTP 404 with S3's NoSuchKey body (<Code>NoSuchKey</Code>, <Message>The specified key does not exist.</Message>) for all five requests.

## Steps to Reproduce
1. Create a bucket, put one object (present.txt), and attach a bucket policy allowing Principal Service cloudfront.amazonaws.com the action s3:GetObject ONLY (no s3:ListBucket), conditioned on the distributions' ARNs.
2. Create a CloudFront distribution with an Origin Access Control over that bucket and the managed CachingDisabled cache policy; wait for Deployed.
3. GET <edge>/present.txt — serves 200.
4. GET <edge>/<a key that was never written> five times. AWS answers 403 AccessDenied each time (the requester lacks s3:ListBucket); the emulator answers 404 NoSuchKey.
5. Re-replay cloudfront__cloudwatch__sns__sqs.obs-cloudfront-2-4xx-error-rate-alarm-notifies-sqs-untouched-control-stays-ok.
