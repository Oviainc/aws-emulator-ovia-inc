Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__s3.obs-s3-2-lookup-events-records-create-bucket-and-rejected-duplicate
Issue-Key: cloudtrail__s3.obs-s3-2-lookup-events-records-create-bucket-and-rejected-duplicate|run#11|AssertionError|assert message == _DUP_MESSAGE, (
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# A duplicate CreateBucket by the owner is refused with no error Message at all

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucket

## Description
Cause 1 of 1: the S3 XML error envelope for a duplicate CreateBucket by the bucket's own owner carries no Message element, so the SDK surfaces the refusal with no message.

AWS: recorded behavior (the check's measured oracle): re-creating a bucket the caller already owns is refused with the documented fixed text `Your previous request to create the named bucket succeeded and you already own it.` (Smithy: CreateBucket models BucketAlreadyExists and BucketAlreadyOwnedByYou as client errors; S3's restXml error body always carries `<Message>`, and for this code the text is a documented constant — the rule is known, so a Message MUST be present and MUST be that text.)
Emulator: the error code matches (the check's code assertion passed) but `Error.Message` is absent — botocore reports it as `None`.
Why: any client, CLI or log pipeline that surfaces the error message gets nothing for a very common condition (re-running an idempotent provisioning step), and operators see a code with no explanation. It also means the recorded audit/error-reporting path has no message to copy.
Fix: the S3 error serializer that renders the XML `<Error>` envelope — emit `<Message>` for BucketAlreadyOwnedByYou (and the other canned S3 codes) rather than code-only. RELATED open issue, which does NOT name this operation/code: 20261022T210606Z-s3-breaks-cloudformation-provisioned-buckets-and-refuses-two.md, whose Cause 2 is the same serializer omitting `<Message>` for `NoSuchBucket`; if that cause is fixed generically for all canned codes this one may close with it, but it is filed separately because that issue's traces and assertions never exercise CreateBucket.
Traces: cloudtrail__s3.obs-s3-2-lookup-events-records-create-bucket-and-rejected-duplicate.

## Evidence
`AssertionError: a duplicate CreateBucket by the owner: expected message 'Your previous request to create the named bucket succeeded and you already own it.', actual None` (cloudtrail__s3.obs-s3-2-lookup-events-records-create-bucket-and-rejected-duplicate, line 111). The error code assertion that precedes it passed, so only the Message is missing.

## Steps to Reproduce
1. `create_bucket(Bucket='b-1', CreateBucketConfiguration={'LocationConstraint': <region>})` (omit the configuration in us-east-1) -> 200.
2. Call the identical `create_bucket` again as the same owner.
   Expect: HTTP 409 with code `BucketAlreadyOwnedByYou` and `<Message>Your previous request to create the named bucket succeeded and you already own it.</Message>`.
   Observe: the code is returned but `exc.response['Error']['Message']` is None (no Message element in the XML body).
