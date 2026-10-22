Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.capability-crud-with-transformer-reference b2bi.capability-instructions-documents-s3-roundtrip b2bi.create-partnership-nonexistent-profile-rejected b2bi.create-starter-mapping-template-with-output-sample b2bi.delete-dependency-order-enforced-rejected b2bi.get-transformer-job-cross-transformer-isolation-rejected b2bi.list-capabilities-reflects-created-capability b2bi.list-partnerships-filtered-by-profile-id b2bi.partnership-lifecycle-with-capability b2bi.start-transformer-job-e2e-state-transition b2bi.start-transformer-job-inactive-transformer-rejected b2bi.start-transformer-job-independent-concurrent-runs b2bi.test-conversion-json-to-x12-roundtrip b2bi.test-parsing-malformed-edi-document-error b2bi.test-parsing-x12-850-purchase-order b2bi.test-parsing-x12-split-by-transaction b2bi.update-partnership-capabilities-list-replace
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|PutBucketPolicy|NotImplemented
Issue-Key: b2bi|PutObject|NotImplemented
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# 17 b2bi capability/partnership/transformer-job checks blocked: the S3 dependency answers NotImplemented for PutObject and PutBucketPolicy

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTransformerJob

## Description
Cause 1 of 1: every b2bi flow that needs real S3 objects or a bucket policy is unreachable because the S3 dependency the b2bi setup calls answers NotImplemented.

AWS: b2bi capability input/output locations, instructionsDocuments, StartTransformerJob/GetTransformerJob and the TestParsing/TestConversion document flows all read and write real S3 objects, and the b2bi service principal must be granted access to the staging bucket with a bucket policy. S3 PutObject returns 200 with an ETag, and PutBucketPolicy returns 204 for a well-formed policy (recorded behavior; the suite's own setup performs s3:PutObject and s3:PutBucketPolicy before touching b2bi).
Emulator: both S3 operations answer the scaffold error — "An error occurred (NotImplemented) when calling the PutObject operation: operation PutObject is scaffolded but not implemented in this emulator" (same for PutBucketPolicy) — so 17 b2bi checks abort during setup and the b2bi behavior they cover (capability CRUD, partnership lifecycle and filtering, transformer-job state transitions and cross-transformer isolation, starter mapping templates, dependency-order delete enforcement, X12 parsing) is never exercised at all.
Why: a client cannot run any realistic b2bi workload against this emulator, because every data-plane path is gated on S3 object and bucket-policy writes; the b2bi surface looks implemented but is untestable end to end.
Fix: this is a cross-service dependency: the S3 emulator's PutObject and PutBucketPolicy handlers must serve these requests before the b2bi data plane can be replayed. Note the open issue "Bootstrap the Amazon S3 emulator" (20261021T221645Z-bootstrap-service-com-amazonaws-s3-amazons3.md) — S3 is still pending bootstrap — and the identical cross-service report already filed for MWAA (20261022T022901Z-mwaa-environment-lifecycle-tagging-round-trip-and-rest.md). Filed here under b2bi because these 17 failing checks are b2bi checks and nothing in b2bi can be verified until the dependency answers.
Traces: the 17 check ids listed in trace_ids (11 blocked on PutObject or PutBucketPolicy during bucket staging, the rest on object upload).

## Evidence
Suite: simparity-behavioral-aws-v1, b2bi 0/42 passed. 17 checks [error] with the setup call failing:

botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutBucketPolicy operation: operation PutBucketPolicy is scaffolded but not implemented in this emulator
  -> b2bi.capability-crud-with-transformer-reference, b2bi.create-partnership-nonexistent-profile-rejected, b2bi.delete-dependency-order-enforced-rejected, b2bi.list-capabilities-reflects-created-capability, b2bi.list-partnerships-filtered-by-profile-id, b2bi.partnership-lifecycle-with-capability, b2bi.start-transformer-job-e2e-state-transition, b2bi.test-parsing-x12-850-purchase-order, b2bi.update-partnership-capabilities-list-replace

botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutObject operation: operation PutObject is scaffolded but not implemented in this emulator
  -> b2bi.capability-instructions-documents-s3-roundtrip, b2bi.create-starter-mapping-template-with-output-sample, b2bi.get-transformer-job-cross-transformer-isolation-rejected, b2bi.start-transformer-job-inactive-transformer-rejected, b2bi.start-transformer-job-independent-concurrent-runs, b2bi.test-conversion-json-to-x12-roundtrip, b2bi.test-parsing-malformed-edi-document-error, b2bi.test-parsing-x12-split-by-transaction

Expected: PutObject 200 (with ETag) and PutBucketPolicy 204, after which the b2bi assertions run.

## Steps to Reproduce
1. Boot the emulator and point botocore s3 and b2bi clients at it.
2. s3.create_bucket(Bucket=<staging bucket>) then s3.put_bucket_policy(Bucket=<staging bucket>, Policy=<policy granting the b2bi service principal access>) -> expect 204; emulator returns NotImplemented.
3. s3.put_object(Bucket=<staging bucket>, Key='in/sample.edi', Body=<X12 850 document>) -> expect 200; emulator returns NotImplemented.
4. With the staging bucket populated, run any of the b2bi flows in trace_ids (capability CRUD, partnership lifecycle, StartTransformerJob, TestParsing) — each currently aborts at step 2 or 3.
5. Re-replay the 17 checks listed in trace_ids.
