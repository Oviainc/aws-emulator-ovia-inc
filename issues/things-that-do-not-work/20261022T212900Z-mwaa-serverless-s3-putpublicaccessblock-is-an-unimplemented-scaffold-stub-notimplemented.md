Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mwaa-serverless.create-workflow-duplicate-name-conflict-rejected mwaa-serverless.create-workflow-duplicate-name-race-exactly-one-wins mwaa-serverless.create-workflow-get-workflow-round-trip mwaa-serverless.create-workflow-invalid-engine-version-rejected mwaa-serverless.create-workflow-invalid-role-arn-pattern-rejected mwaa-serverless.create-workflow-untrusted-execution-role-access-denied-rejected mwaa-serverless.delete-workflow-removes-resource mwaa-serverless.get-workflow-nonexistent-arn-not-found-rejected mwaa-serverless.list-task-instances-for-never-started-run-returns-empty mwaa-serverless.list-workflow-runs-reflects-multiple-runs mwaa-serverless.list-workflow-runs-returns-empty-for-new-workflow mwaa-serverless.list-workflow-versions-reflects-update-history mwaa-serverless.list-workflows-reflects-create-and-delete mwaa-serverless.start-workflow-run-nonexistent-workflow-version-not-found-rejected mwaa-serverless.stop-workflow-run-transitions-to-stopped mwaa-serverless.tag-resource-and-list-tags-round-trip mwaa-serverless.untag-resource-removes-specific-tag mwaa-serverless.update-workflow-creates-new-version-flips-latest mwaa-serverless.workflow-definition-snapshotted-at-creation-unaffected-by-s3-overwrite mwaa-serverless.workflow-run-failure-records-error-message mwaa-serverless.workflow-run-status-progresses-through-lifecycle-states mwaa-serverless.workflow-run-success-end-to-end
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless|PutPublicAccessBlock|NotImplemented
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# S3 PutPublicAccessBlock is an unimplemented scaffold stub (NotImplemented), blocking all 22 mwaa-serverless workflow behavioral checks at bucket setup

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPublicAccessBlock

## Description
Cause 1 of 1: S3's PutPublicAccessBlock is still a scaffold stub and answers an error instead of succeeding, so every mwaa-serverless behavioral check fails before it can issue a single MWAA Serverless call. Each check's setup creates the DAG bucket it will point CreateWorkflow at and hardens it (CreateBucket -> PutBucketTagging -> PutPublicAccessBlock -> PutBucketVersioning); the PutPublicAccessBlock call aborts the setup, so none of the workflow control-plane or run-lifecycle behavior behind it was exercised at all by this sweep.

AWS: PutPublicAccessBlock is a modeled S3 bucket sub-resource write. Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): PutPublicAccessBlock, http PUT /{Bucket}?publicAccessBlock, success code 200, empty output (output_shape Unit), no modeled errors; required input members Bucket (@httpLabel) and PublicAccessBlockConfiguration (@httpPayload, XML name PublicAccessBlockConfiguration, members BlockPublicAcls / IgnorePublicAcls / BlockPublicPolicy / RestrictPublicBuckets), optional ContentMD5, ChecksumAlgorithm (x-amz-sdk-checksum-algorithm), ExpectedBucketOwner (x-amz-expected-bucket-owner); httpChecksum requestChecksumRequired. Real AWS returns 200 with an empty body for an existing bucket owned by the caller, and the configuration is then readable via GetPublicAccessBlock.

Emulator: the call is refused with the scaffold-stub error — error code 'NotImplemented', message (example) 'operation PutPublicAccessBlock is scaffolded but not implemented in this emulator'. The operation is served out of the crate's scaffolded module rather than ops, so no public-access-block state is stored for the bucket either.

Why: any client that hardens a bucket before using it — which is the standard, and in these traces mandatory, setup for an MWAA Serverless DAG bucket — takes a hard client error on a call that always succeeds against AWS. Because this is a setup step, it does not merely degrade one API: it makes the entire mwaa-serverless workflow surface (CreateWorkflow/GetWorkflow round-trip and its duplicate-name, invalid-engine-version, bad-role-ARN and untrusted-role rejections; UpdateWorkflow versioning; ListWorkflows/ListWorkflowVersions/ListWorkflowRuns/ListTaskInstances; Start/StopWorkflowRun and run lifecycle; Tag/UntagResource) unreachable for such a client, and unverified here.

Fix: implement PutPublicAccessBlock in the S3 service crate — move it out of services/service-com-amazonaws-s3-amazons3/src/scaffolded.rs into the implemented operation handlers (src/ops), parse the PublicAccessBlockConfiguration XML payload, persist the four flags on the bucket in the crate's state (so GetPublicAccessBlock/DeletePublicAccessBlock can round-trip it), and return 200 with an empty body. Checks: mwaa-serverless.* (all 22 listed under Traces).

Traces: all 22 failing mwaa-serverless checks error identically at the shared _create_dag_bucket helper's s3.put_public_access_block(...) call; see trace_ids.

## Evidence
Expected (real AWS / Smithy model): PUT /{Bucket}?publicAccessBlock with a PublicAccessBlockConfiguration payload -> HTTP 200, empty body, no error; the four block flags are then persisted on the bucket.

Actual (emulator, identical in all 22 failing checks): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutPublicAccessBlock operation: operation PutPublicAccessBlock is scaffolded but not implemented in this emulator

Suite report: mwaa-serverless 1/23 (4%), 22 failed, every failure verdict 'error' with the NotImplemented ClientError above raised from the check's bucket-setup helper, before any mwaa-serverless API call is made.

## Steps to Reproduce
1. Boot the emulator and point a botocore/boto3 client at it.
2. s3 = client('s3'); s3.create_bucket(Bucket='sim2real-mwaas-dupname-x', CreateBucketConfiguration={'LocationConstraint': <region>} if region != 'us-east-1' else omitted)
3. s3.put_bucket_tagging(Bucket=..., Tagging={'TagSet': [...]})  # succeeds
4. s3.put_public_access_block(Bucket=..., PublicAccessBlockConfiguration={'BlockPublicAcls': True, 'IgnorePublicAcls': True, 'BlockPublicPolicy': True, 'RestrictPublicBuckets': True})
   -> observe ClientError NotImplemented ('operation PutPublicAccessBlock is scaffolded but not implemented in this emulator'); real AWS returns 200 with an empty body.
5. Equivalently, replay any check in trace_ids (e.g. mwaa-serverless.create-workflow-get-workflow-round-trip): it aborts at step 4 of its bucket setup and never reaches CreateWorkflow.
