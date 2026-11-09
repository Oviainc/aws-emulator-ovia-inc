Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__s3__s3control.deploy-s3express-directorybucket-invalid-data-redundancy-rejected cloudformation__s3.cdk-s3-public-access-blocked-by-default-flag-governs-whether-a-public-bucket-policy-is-rejected cloudformation__s3.cfn-s3-bucket-cors-allowed-methods-duplicate-rejected cloudformation__s3.cfn-s3-bucket-tags-drift-reports-expected-and-actual cloudformation__s3.cfn-s3-bucket-uppercase-name-rejected-and-never-created cloudformation__s3.deploy-s3-bucket-policy-missing-policydocument-rejected cloudformation__s3.nested-change-set-child-execution-status-unavailable
Issue-Key: cloudformation__s3|CreateStack|ValidationError
Issue-Key: cloudformation__s3.cdk-s3-public-access-blocked-by-default-flag-governs-whether-a-public-bucket-policy-is-rejected|run#24|AssertionError|assert config_off == {
Issue-Key: s3|GetBucketCors|NoSuchCORSConfiguration
Issue-Key: cloudformation__s3.cfn-s3-bucket-tags-drift-reports-expected-and-actual|run#17|AssertionError|assert drifted["StackDriftStatus"] == "DRIFTED", (
Issue-Key: cloudformation__s3.cfn-s3-bucket-uppercase-name-rejected-and-never-created|run#10|AssertionError|assert code == "InternalFailure", f"expected HandlerErrorCode InternalFailure, got {code!r}"
Issue-Key: cloudformation__s3.deploy-s3-bucket-policy-missing-policydocument-rejected|run#19|AssertionError|assert stack_failed_reasons == [_STACK_REASON], (
Issue-Key: cloudformation__s3.nested-change-set-child-execution-status-unavailable|run#16|KeyError|child_cs = _stack_change(root, "Child")["ChangeSetId"]
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudformation__s3: 7 net-new causes — S3Express types unregistered, bucket PublicAccessBlock/CorsConfiguration not applied, drift never detected, wrong resource HandlerErrorCode, missing CREATE_FAILED reason, no nested ChangeSetId

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Seven causes.

Cause 1 of 7: CreateStack rejects AWS::S3Express::DirectoryBucket as an unrecognized resource type.
AWS: AWS::S3Express::DirectoryBucket is a supported resource type; a template declaring it with an invalid DataRedundancy must be refused *by the resource's validation* (the stack rolls back with the S3 Express validation reason), not by template-format validation. Recorded behavior: AWS answers the template-format ValidationError only for types that do not exist.
Emulator: CreateStack answers 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::S3Express::DirectoryBucket]' (fixed text 'Template format error: Unrecognized resource types: [...]', the list taken from the template).
Why: the negative test cannot distinguish 'bad DataRedundancy' from 'type does not exist', and no stack can provision a directory bucket.
Fix: register AWS::S3Express::DirectoryBucket in the CloudFormation resource-type registry and give it a create handler that validates DataRedundancy against the S3 Express rules.
Traces: cloudformation__ec2__s3__s3control.deploy-s3express-directorybucket-invalid-data-redundancy-rejected (listed under this heading).

Cause 2 of 7: AWS::S3::Bucket PublicAccessBlockConfiguration is ignored; GetPublicAccessBlock reports all four flags true.
AWS: the created bucket's public-access block is exactly what the template declares; Smithy: s3 GetPublicAccessBlock returns PublicAccessBlockConfiguration{BlockPublicAcls, IgnorePublicAcls, BlockPublicPolicy, RestrictPublicBuckets} as booleans. With the CDK '@aws-cdk/aws-s3:blockPublicAccessedByDefault'-style flag unset, the synthesized bucket declares only BlockPublicAcls true and the other three false, and a public bucket policy on it is therefore accepted.
Emulator: GetPublicAccessBlock returns {'BlockPublicAcls': True, 'IgnorePublicAcls': True, 'BlockPublicPolicy': True, 'RestrictPublicBuckets': True} — a hard-coded all-true default that overrides the declared configuration.
Why: the emulator then refuses public bucket policies AWS would accept (and would wrongly accept them where AWS blocks), so every policy-dependent deployment diverges.
Fix: the AWS::S3::Bucket provisioner must write the declared PublicAccessBlockConfiguration into the bucket's state, and GetPublicAccessBlock must echo it member-for-member.
Traces: cloudformation__s3.cdk-s3-public-access-blocked-by-default-flag-governs-whether-a-public-bucket-policy-is-rejected.

Cause 3 of 7: AWS::S3::Bucket CorsConfiguration is not applied; GetBucketCors answers NoSuchCORSConfiguration.
AWS: a bucket declared with CorsConfiguration has that rule set after CREATE_COMPLETE, so GetBucketCors returns CORSRules; the duplicate-AllowedMethod case must be refused by the resource's validation. Smithy: s3 GetBucketCors returns CORSConfiguration.CORSRules and NoSuchCORSConfiguration ('The CORS configuration does not exist') is the answer only for a bucket with no CORS configuration.
Emulator: GetBucketCors on the deployed bucket raises NoSuchCORSConfiguration 'The CORS configuration does not exist' — the template's CORS rules never reached the bucket.
Why: browser clients get no CORS headers from buckets whose IaC declares them, and the duplicate-method refusal cannot be observed.
Fix: the AWS::S3::Bucket provisioner must apply CorsConfiguration to the bucket's CORS state (and validate duplicate AllowedMethods at create time).
Traces: cloudformation__s3.cfn-s3-bucket-cors-allowed-methods-duplicate-rejected.

Cause 4 of 7: stack drift detection never reports DRIFTED.
AWS: after an out-of-band change to a managed resource (here the bucket's tag set), DetectStackDrift/DescribeStackResourceDrifts report StackDriftStatus 'DRIFTED' with ExpectedProperties and ActualProperties for the changed resource. Smithy: cloudformation DescribeStackDriftDetectionStatus.StackDriftStatus enum includes DRIFTED/IN_SYNC.
Emulator: StackDriftStatus is 'IN_SYNC' after the tag change — drift is never computed against live resource state.
Why: drift detection is the sanctioned way to find manual changes; always answering IN_SYNC means callers believe their estate matches the template when it does not.
Fix: the CloudFormation drift-detection handler must compare the stack's recorded resource properties against the owning service's live state and report per-resource expected/actual differences.
Traces: cloudformation__s3.cfn-s3-bucket-tags-drift-reports-expected-and-actual.

Cause 5 of 7: the resource-level HandlerErrorCode for an invalid bucket name is InvalidRequest instead of InternalFailure.
AWS: when AWS::S3::Bucket is declared with an uppercase (invalid) BucketName, the failed resource's event carries HandlerErrorCode 'InternalFailure' and the bucket is never created. Recorded behavior: the check asserts the HandlerErrorCode of the CREATE_FAILED resource.
Emulator: the HandlerErrorCode is 'InvalidRequest'.
Why: tooling that branches on HandlerErrorCode (retry vs. fail-fast) takes the wrong branch, and the failure is classified as the caller's malformed request rather than the provisioning failure AWS reports.
Fix: the AWS::S3::Bucket create handler's error mapping must emit HandlerErrorCode InternalFailure for a bucket-name rejection from S3.
Traces: cloudformation__s3.cfn-s3-bucket-uppercase-name-rejected-and-never-created.

Cause 6 of 7: a stack whose resource omits a required property fails with no CREATE_FAILED reason.
AWS: an AWS::S3::BucketPolicy without PolicyDocument makes the stack CREATE_FAILED with the reason 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.' (fixed text; only the error count varies with the template).
Emulator: the stack's CREATE_FAILED reasons are [] — no reason at all is recorded.
Why: the caller is told the stack failed but not why; the documented pointer to DescribeEvents is missing, so there is nothing to act on.
Fix: the CloudFormation template/resource validation must record that validation-failure reason on the stack's failure event.
Traces: cloudformation__s3.deploy-s3-bucket-policy-missing-policydocument-rejected.

Cause 7 of 7: CreateChangeSet with IncludeNestedStacks mints no child ChangeSetId, so the child change set has no execution status to read.
AWS: with IncludeNestedStacks=true, the parent's Changes[].ResourceChange for the nested AWS::CloudFormation::Stack carries ChangeSetId naming the child change set, and DescribeChangeSet on that id reports the child's ExecutionStatus. Smithy: cloudformation ResourceChange.ChangeSetId and DescribeChangeSet.ExecutionStatus.
Emulator: the nested ResourceChange has no 'ChangeSetId' member — reading it raises KeyError: 'ChangeSetId'.
Why: a caller cannot inspect or gate on what a nested stack will change, which is the entire purpose of IncludeNestedStacks.
Fix: the CreateChangeSet nested-stack expansion must create a change set per nested stack and populate ChangeSetId on the parent's ResourceChange. The open issue 20261107T060626Z-cloudformation-s3-cloudformation-nested-stacks-no-nested-change-set-tree.md describes the same area but does not carry this check's replay key.
Traces: cloudformation__s3.nested-change-set-child-execution-status-unavailable.

## Evidence
Cause 1: ClientError (ValidationError) CreateStack: 'Template format error: Unrecognized resource types: [AWS::S3Express::DirectoryBucket]'.
Cause 2: AssertionError: GetPublicAccessBlock on the unset-flag bucket returned {'BlockPublicAcls': True, 'IgnorePublicAcls': True, 'BlockPublicPolicy': True, 'RestrictPublicBuckets': True}, expected only BlockPublicAcls true and the rest false.
Cause 3: ClientError (NoSuchCORSConfiguration) GetBucketCors: 'The CORS configuration does not exist'.
Cause 4: AssertionError: stack drift status: expected DRIFTED, got 'IN_SYNC'.
Cause 5: AssertionError: expected HandlerErrorCode InternalFailure, got 'InvalidRequest'.
Cause 6: AssertionError: expected the stack's CREATE_FAILED reason to be 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.', got [].
Cause 7: KeyError: 'ChangeSetId' while reading the parent change set's nested ResourceChange.

## Steps to Reproduce
Cause 1: create_stack with AWS::S3Express::DirectoryBucket (invalid DataRedundancy).
Cause 2: cdk deploy / create_stack an AWS::S3::Bucket declaring PublicAccessBlockConfiguration with only BlockPublicAcls true, then s3.get_public_access_block(Bucket=b).
Cause 3: create_stack an AWS::S3::Bucket with CorsConfiguration (duplicate AllowedMethods in the negative case), then s3.get_bucket_cors(Bucket=b).
Cause 4: deploy a tagged AWS::S3::Bucket, change its tag set out of band with s3:PutBucketTagging, then detect_stack_drift and read the status.
Cause 5: create_stack an AWS::S3::Bucket with an uppercase BucketName and read the failed resource's HandlerErrorCode.
Cause 6: create_stack an AWS::S3::BucketPolicy with no PolicyDocument and read the stack's CREATE_FAILED reasons.
Cause 7: create_change_set(..., IncludeNestedStacks=True) on a parent with a nested stack and read Changes[].ResourceChange['ChangeSetId'].
