Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: osis.concurrent-start-pipeline-exactly-one-succeeds osis.create-pipeline-duplicate-name-rejected osis.create-pipeline-min-greater-than-max-units-rejected osis.create-pipeline-reaches-active-state osis.delete-pipeline-removes-resource osis.native-protocol-http-ingest-round-trip osis.put-resource-policy-invalid-json-rejected osis.resource-policy-lifecycle-round-trip osis.start-pipeline-when-already-active-rejected osis.stop-start-pipeline-transitions
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis|PutBucketTagging|NotImplemented
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# OSIS pipeline lifecycle checks all blocked: S3 PutBucketTagging answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketTagging

## Description
Cause 1 of 1: every OSIS behavioral check that needs a pipeline (create/delete/start/stop/resource-policy/ingest) builds its S3 sink bucket first, and the setup call `s3.put_bucket_tagging` is refused with HTTP 501 `NotImplemented`, so 10 of the 22 checks in this suite never reach a single OSIS operation. Failing check ids: osis.concurrent-start-pipeline-exactly-one-succeeds, osis.create-pipeline-duplicate-name-rejected, osis.create-pipeline-min-greater-than-max-units-rejected, osis.create-pipeline-reaches-active-state, osis.delete-pipeline-removes-resource, osis.native-protocol-http-ingest-round-trip, osis.put-resource-policy-invalid-json-rejected, osis.resource-policy-lifecycle-round-trip, osis.start-pipeline-when-already-active-rejected, osis.stop-start-pipeline-transitions.

AWS: `PutBucketTagging` is a modeled, supported S3 operation. Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json, com.amazonaws.s3#PutBucketTagging): HTTP `PUT /{Bucket}?tagging`, required members `Bucket` (BucketName) and `Tagging` (Tagging container for TagSet), output is Unit (the scaffold's OUTPUT_SCHEMA declares success status 200), and the documented failure modes are `InvalidTag`, `MalformedXML`, `OperationAborted` and `InternalError`. On a bucket that exists and a well-formed TagSet, AWS succeeds with an empty body and the tag set replaces any current tags (readable back via GetBucketTagging). It never answers 501 for a supported operation.

Emulator: the request is rejected with HTTP 501, error code `NotImplemented`, message "operation PutBucketTagging is scaffolded but not implemented in this emulator" (botocore raises ClientError NotImplemented). The handler is still the generated stub: its `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("PutBucketTagging"))`.

Why: a client cannot tag a bucket at all, and because bucket tagging is a routine step in provisioning scripts, every downstream workflow that provisions an S3 sink aborts on an unexpected 501 — here it masks the entire OSIS pipeline control plane (CreatePipeline/StartPipeline/StopPipeline/DeletePipeline, resource policies and HTTP ingest), none of which can be exercised or trusted until the setup call succeeds.

Fix: implement the operation's handler so it validates the TagSet and persists the tag set on the bucket in the s3 service state (with GetBucketTagging/DeleteBucketTagging reading the same state), and delete the SCAFFOLDED marker. Note this cause lives in the S3 crate while the failing checks are OSIS checks; the open issue "Bootstrap the Amazon S3 emulator" (20261021T221645Z-bootstrap-service-com-amazonaws-s3-amazons3.md) covers standing S3 up generally but names no operation and does not carry these OSIS replay keys, and the suite did not mark these failures as already filed.

Traces: the 10 osis check ids listed above; the stub is services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_tagging.rs lines 4-6 (SCAFFOLDED marker) and 207 (the `not_implemented` return).

## Evidence
Expected (AWS): PutBucketTagging on an existing bucket with TagSet [{Key: sim2real, Value: <check>}] succeeds with an empty body and the tags are stored.
Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutBucketTagging operation: operation PutBucketTagging is scaffolded but not implemented in this emulator — HTTP 501, raised from behavioral_harness setup helper `_create_pipeline_role_and_bucket` before any OSIS call is made.
Blast radius from the suite report: osis 1/22 passed; 10 of the 21 failures are this single call.

## Steps to Reproduce
1. Boot the emulator and point an S3 client at it.
2. s3.create_bucket(Bucket="sim2real-osis-behav-x") (succeeds).
3. s3.put_bucket_tagging(Bucket="sim2real-osis-behav-x", Tagging={"TagSet": [{"Key": "sim2real", "Value": "create-pipeline-reaches-active-state"}]}).
4. Observe HTTP 501 NotImplemented instead of a success response; the OSIS checks listed in trace_ids abort here.
