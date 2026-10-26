Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.cli-s3-select-vs-athena-query-comparison
Issue-Key: athena__glue__s3.cli-s3-select-vs-athena-query-comparison|run#11|AssertionError|assert select_error_code == "MethodNotAllowed", select_error_code
Issue-Commit: 176bb95e3c08e52684921328b2da3d677bd84f34

# S3 SelectObjectContent answers NotImplemented instead of the recorded MethodNotAllowed/405 refusal (1 cause)

> _Note: this issue was identified against the code base at commit `176bb95e3c08e52684921328b2da3d677bd84f34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SelectObjectContent

## Description
One root cause, grounded in one failing behavioral check of suite simparity-behavioral-ga-prod-v1.

Cause 1 of 1 — SelectObjectContent is neither served nor refused the way AWS refuses it.

AWS: Smithy com.amazonaws.s3#SelectObjectContent — POST /{Bucket}/{Key+}?select&select-type=2, success code 200, output member Payload targets the streaming event stream SelectObjectContentEventStream; the operation models NO errors of its own. Recorded behavior only: AWS discontinued S3 Select for accounts that had not already enabled it before the cutoff, and such an account gets a deterministic refusal with HTTP status 405 and error code MethodNotAllowed for every SelectObjectContent request, independent of encryption mode or object shape. Either outcome is AWS-faithful: a 200 event stream carrying the selected rows, or the 405/MethodNotAllowed refusal.

Emulator: the boto3 call s3.select_object_content(...) is refused with error code NotImplemented and the message 'operation SelectObjectContent is scaffolded but not implemented in this emulator'. That code is not modeled for this operation and is neither of AWS's two outcomes. Everything else on the S3 path answers normally in the same check (CreateBucket, PutBucketEncryption with SSEAlgorithm=AES256, PutObject with ServerSideEncryption=AES256 and the GetObject positive control that reads the same bytes back all succeed), so this is specific to SelectObjectContent and not a broadly broken S3.

Why: a client that probes S3 Select and branches on MethodNotAllowed to fall back to a different read path (GetObject, or a query engine) never sees that code, so the fallback never triggers and the call surfaces as an unexpected, unmodeled exception. The 405 refusal is also the only way a caller can tell 'this account does not have S3 Select' from 'this request was malformed'.

Fix: the s3 SelectObjectContent handler (services/service-com-amazonaws-s3-amazons3/src/ops/select_object_content.rs) — either serve the modeled 200 event-stream response, or answer the recorded refusal with HTTP 405 and error code MethodNotAllowed instead of NotImplemented.

Traces: athena__glue__s3.cli-s3-select-vs-athena-query-comparison (the check asserts the refusal code is MethodNotAllowed and got NotImplemented).

Service-attribution note: the leading token of the check id is the composite 'athena__glue__s3' (the check declares SERVICES = ['athena','glue','s3']), which is not a single service token. The failing call is S3's SelectObjectContent, made through boto3 before any Athena or Glue call in the check, so this issue is filed under service 's3' (bootstrapped). The Athena leg of the same check is separately blocked by Athena not being bootstrapped and is NOT part of this issue.

## Evidence
Expected (AWS): s3.select_object_content(Bucket=b, Key='data/rows.csv', Expression="SELECT s.id, s.name, s.category FROM S3Object s WHERE s.id = '2'", ExpressionType='SQL', InputSerialization={'CSV': {'FileHeaderInfo': 'USE'}}, OutputSerialization={'CSV': {}}) -> either HTTP 200 with a Payload event stream whose Records decode to [['2','bob','silver']], or a refusal with HTTP 405 / Error.Code == 'MethodNotAllowed'.

Actual: ClientError with Error.Code == 'NotImplemented' and message 'operation SelectObjectContent is scaffolded but not implemented in this emulator'. Check assertion: `assert select_error_code == "MethodNotAllowed", select_error_code` -> AssertionError: NotImplemented.

Control in the same check that passed: s3.get_object(Bucket=b, Key='data/rows.csv') returns the CSV bytes containing '2,bob,silver', so the object and S3 access are fine.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=b) (with CreateBucketConfiguration.LocationConstraint outside us-east-1).
3. s3.put_bucket_encryption(Bucket=b, ServerSideEncryptionConfiguration={'Rules': [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}}]}).
4. s3.put_object(Bucket=b, Key='data/rows.csv', Body=b'id,name,category\n1,alice,gold\n2,bob,silver\n3,carol,bronze\n', ServerSideEncryption='AES256').
5. s3.get_object(Bucket=b, Key='data/rows.csv') -> succeeds (control).
6. s3.select_object_content(Bucket=b, Key='data/rows.csv', Expression="SELECT s.id, s.name, s.category FROM S3Object s WHERE s.id = '2'", ExpressionType='SQL', InputSerialization={'CSV': {'FileHeaderInfo': 'USE'}}, OutputSerialization={'CSV': {}}) -> observe Error.Code 'NotImplemented'; expect a 200 event stream or Error.Code 'MethodNotAllowed' with HTTP 405.
7. Re-replay athena__glue__s3.cli-s3-select-vs-athena-query-comparison.
