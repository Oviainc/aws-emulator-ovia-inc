Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.cfn-cli-missing-s3-getobject-permission-blocks-template-fetch-negative cloudformation__dynamodb__s3.cfn-deploy-s3-bucket-deletion-policy-retain-positive cloudformation__dynamodb__s3.cfn-nested-stack-template-from-s3-dynamodb-resource-positive cloudformation__dynamodb__s3.s3-replication-role-missing-trust-negative
Issue-Key: cloudformation__dynamodb__s3.cfn-cli-missing-s3-getobject-permission-blocks-template-fetch-negative|_client_error#28|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded: {result!r}")
Issue-Key: s3|HeadBucket|404
Issue-Key: cloudformation__dynamodb__s3.cfn-nested-stack-template-from-s3-dynamodb-resource-positive|_deploy_cfn#12|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__dynamodb__s3.s3-replication-role-missing-trust-negative|_assert_never_replicates#10|AssertionError|raise AssertionError(
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# CloudFormation/S3: template-fetch authorization, DeletionPolicy Retain on buckets, nested-stack outputs and replication-role trust (4 causes)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four net-new root causes in the cloudformation__dynamodb__s3 group. Each needs its own fix.

Cause 1 of 4: CreateStack with a TemplateURL the caller may not read is accepted.
AWS: CloudFormation fetches the template as the calling principal; if that principal lacks `s3:GetObject` on the template object, the call is refused with a client error (HTTP 400) naming the access failure, and no stack is created.
Emulator: the call SUCCEEDS and returns a StackId (HTTP 200).
Why: template-fetch authorization is not enforced, so a principal deploys templates it cannot read — and the negative path a caller must handle never occurs.
Fix: the CreateStack handler's TemplateURL fetch (authorize the read as the caller before accepting the stack).
Traces: cloudformation__dynamodb__s3.cfn-cli-missing-s3-getobject-permission-blocks-template-fetch-negative

Cause 2 of 4: an S3 bucket with DeletionPolicy: Retain is removed with the stack.
AWS: deleting a stack leaves a resource whose DeletionPolicy is `Retain` in place; `HeadBucket` on the retained bucket still answers 200.
Emulator: the bucket is gone — `HeadBucket` answers HTTP 404 `Not Found`.
Why: DeletionPolicy Retain is the mechanism that protects data from a stack delete; ignoring it destroys data a caller was promised would survive. (The open issue 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md carries the sibling bucket-retention check but not this replay key.)
Fix: the cloudformation stack-delete path's DeletionPolicy handling for AWS::S3::Bucket.
Traces: cloudformation__dynamodb__s3.cfn-deploy-s3-bucket-deletion-policy-retain-positive

Cause 3 of 4: a nested stack whose template comes from S3 produces outputs with no OutputValue.
AWS: the parent stack's Outputs resolve from the nested stack's outputs and are returned by DescribeStacks with an `OutputValue`.
Emulator: reading the Output raises `KeyError: 'OutputValue'`.
Why: nested-stack composition yields nothing a caller can consume, so the resources the nested template created are undiscoverable.
Fix: the cloudformation nested-stack output propagation and the DescribeStacks serializer (OutputValue must always be present).
Traces: cloudformation__dynamodb__s3.cfn-nested-stack-template-from-s3-dynamodb-resource-positive

Cause 4 of 4: S3 replication runs with a role S3 may not assume.
AWS: cross-bucket replication only happens if the replication role's trust policy permits `s3.amazonaws.com` to assume it; with that principal missing, the object is never replicated.
Emulator: the object appears in the destination bucket anyway.
Why: the trust policy is the control that stops a bucket from writing into another bucket; ignoring it replicates data that in AWS would stay put — a silent data-placement divergence.
Fix: the s3 replication driver's role assumption (evaluate the role's trust policy before replicating).
Traces: cloudformation__dynamodb__s3.s3-replication-role-missing-trust-negative

## Evidence
- cfn-cli-missing-s3-getobject-permission-blocks-template-fetch-negative: AssertionError: expected a ClientError but the call succeeded: {'StackId': 'arn:aws:cloudformation:...:stack/<name>/<id>', 'OperationId': ..., 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.
- cfn-deploy-s3-bucket-deletion-policy-retain-positive: botocore.exceptions.ClientError: An error occurred (404) when calling the HeadBucket operation: Not Found.
- cfn-nested-stack-template-from-s3-dynamodb-resource-positive: KeyError: 'OutputValue'.
- s3-replication-role-missing-trust-negative: AssertionError: object 'replicated/should-not-appear.txt' replicated into '<destination bucket>' despite the role's trust policy not permitting s3.amazonaws.com to assume it.

## Steps to Reproduce
Boot the emulator and replay cloudformation__dynamodb__s3.cfn-cli-missing-s3-getobject-permission-blocks-template-fetch-negative, .cfn-deploy-s3-bucket-deletion-policy-retain-positive, .cfn-nested-stack-template-from-s3-dynamodb-resource-positive and .s3-replication-role-missing-trust-negative.
