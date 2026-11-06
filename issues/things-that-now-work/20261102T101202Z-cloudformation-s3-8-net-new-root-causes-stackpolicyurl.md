Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3.stack-create-stack-policy-url-applied-and-validated cloudformation__s3.stack-create-stack-policy-url-missing-object-rejected cloudformation__s3.stack-update-stack-policy-during-update-url-overrides-once cloudformation__s3.stack-update-stack-policy-url-replaces-stored-policy cloudformation__s3.cfn-s3-nested-child-stack-counts-for-its-own-bucket cloudformation__s3.cfn-topstacks-nested-stack-lists-as-its-own-prefixed-active-stack cloudformation__s3.stack-nested-child-update-failure-reason-chain cloudformation__s3.change-set-disable-validation-defers-the-bucket-name-collision-to-execution cloudformation__s3.change-set-pre-deployment-validation-rejects-an-existing-bucket-name cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy cloudformation__s3.cfn-s3-list-stacks-with-unknown-status-filter-rejected cloudformation__s3.cfn-s3-no-op-update-of-bucket-stack-rejected cloudformation__s3.e2e-expand-elasticsearch-cfn-template-lifecycle cloudformation__s3.template-url-missing-object-rejected
Issue-Key: cloudformation__s3|CreateStack|NotImplemented
Issue-Key: cloudformation__s3.stack-create-stack-policy-url-missing-object-rejected|run#8|AssertionError|assert got == want, f"expected {want!r}, got {got!r}"
Issue-Key: cloudformation__s3|UpdateStack|NotImplemented
Issue-Key: cloudformation__s3.cfn-s3-nested-child-stack-counts-for-its-own-bucket|run#19|AssertionError|assert len(summaries) == 2, (
Issue-Key: cloudformation__s3.cfn-topstacks-nested-stack-lists-as-its-own-prefixed-active-stack|run#9|AssertionError|assert len(kids) == 1 and kids[0].startswith(parent + "-Kid-"), (
Issue-Key: cloudformation__s3.stack-nested-child-update-failure-reason-chain|run#9|AssertionError|assert re.fullmatch(rf"arn:aws[a-z-]*:cloudformation:{ctx.region}:{ctx.account}:stack/{parent_name}-Child-[A-Za-z0-9]+/"
Issue-Key: cloudformation__s3.change-set-disable-validation-defers-the-bucket-name-collision-to-execution|run#10|AssertionError|assert got == want, f"with validation on: expected {want!r}, got {got!r}"
Issue-Key: cloudformation__s3.change-set-pre-deployment-validation-rejects-an-existing-bucket-name|run#19|AssertionError|assert (cs["Status"], cs["ExecutionStatus"]) == ("FAILED", "UNAVAILABLE"), (
Issue-Key: cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing|run#28|AssertionError|assert _bucket_keys(s3, allowed_bucket) == {"payload.txt"}, (
Issue-Key: cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy|run#25|AssertionError|assert keys_after_deploy == {"keep.txt", "prune-me.txt"}, (
Issue-Key: cloudformation__s3.cfn-s3-list-stacks-with-unknown-status-filter-rejected|run#8|AssertionError|assert got == ("ValidationError", _MIXED_FILTER_MESSAGE), (
Issue-Key: cloudformation__s3.cfn-s3-no-op-update-of-bucket-stack-rejected|run#14|AssertionError|assert status == "UPDATE_COMPLETE", f"the tagged update reached {status!r}"
Issue-Key: cloudformation__s3.e2e-expand-elasticsearch-cfn-template-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudformation__s3.template-url-missing-object-rejected|run#26|AssertionError|assert got[0] == "ValidationError" and stack_set_message_re.match(got[1]), (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/s3: 8 net-new root causes — StackPolicyURL unimplemented, nested stacks unlisted, change-set pre-deployment validation missing, BucketDeployment lands nothing, S3/validation refusal messages

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Eight distinct net-new root causes. Open issue 20261029T211510Z-cloudformation-s3-createchangeset-template-source-validation-7-divergent-refusals-for.md owns the CreateChangeSet template-source refusals; none of the replay keys below are carried by an open issue.

Cause 1 of 8: stack policies cannot be supplied from S3 at all.
AWS: Smithy: CreateStack.StackPolicyURL (length 1..5120) and UpdateStack.StackPolicyURL/StackPolicyDuringUpdateURL — the policy is fetched from S3 and applied; a URL pointing at a missing object is refused with ValidationError naming the bucket/key.
Emulator: all three answer ('NotImplemented', 'operation CreateStack with StackPolicyURL (fetching a stack policy from Amazon S3) is scaffolded but not implemented in this emulator') / the UpdateStack equivalents.
Why: a client cannot protect stack resources with a policy stored in S3, and the missing-object negative case cannot be distinguished from an unimplemented feature.
Fix: StackPolicyURL/StackPolicyDuringUpdateURL handling in the CreateStack and UpdateStack handlers (S3 fetch + policy store).
Traces: cloudformation__s3.stack-create-stack-policy-url-applied-and-validated, cloudformation__s3.stack-create-stack-policy-url-missing-object-rejected, cloudformation__s3.stack-update-stack-policy-during-update-url-overrides-once, cloudformation__s3.stack-update-stack-policy-url-replaces-stored-policy.

Cause 2 of 8: nested child stacks are neither listed nor addressable as stacks.
AWS: recorded behavior only — a nested stack is a stack: ListStacks returns the parent and the child (the child's name is '<parent>-<LogicalId>-<suffix>'), and the nested resource's physical id is the child's stack ARN, which is what makes the failure-reason chain and per-stack resource counts work.
Emulator: ListStacks returns only the parent ([<parent>] / []) and the nested resource's physical id is a generated name, not an ARN.
Why: nested stacks are invisible to inventory, cost/resource breakdowns and failure-reason navigation.
Fix: nested-stack provisioning must register a child stack (ARN as physical id) and include it in ListStacks/DescribeStacks.
Traces: cloudformation__s3.cfn-s3-nested-child-stack-counts-for-its-own-bucket, cloudformation__s3.cfn-topstacks-nested-stack-lists-as-its-own-prefixed-active-stack, cloudformation__s3.stack-nested-child-update-failure-reason-chain.

Cause 3 of 8: change sets are not pre-deployment validated.
AWS: recorded behavior only — with validation enabled, CreateChangeSet runs early validation and a template whose bucket name is already taken fails the change set: Status FAILED, ExecutionStatus UNAVAILABLE, StatusReason 'The following hook(s)/validation failed: [AWS::EarlyValidation::ResourceExistenceCheck]. To troubleshoot Early Validation errors, use the DescribeEvents API for detailed failure information.'
Emulator: the change set reaches CREATE_COMPLETE/AVAILABLE with no StatusReason, so the collision is only discovered at execution time.
Why: the whole point of validating before executing is lost, and DisableValidation has no observable effect.
Fix: the early-validation stage of the CreateChangeSet handler.
Traces: cloudformation__s3.change-set-disable-validation-defers-the-bucket-name-collision-to-execution, cloudformation__s3.change-set-pre-deployment-validation-rejects-an-existing-bucket-name.

Cause 4 of 8: CDK BucketDeployment delivers no objects.
AWS: recorded behavior only — a BucketDeployment custom resource uploads its source objects, so after the deploy the destination bucket holds exactly those keys (and prunes removed ones); when the role is denied PutObject the create fails and nothing lands.
Emulator: the positive control bucket holds [] after a successful deploy — nothing is ever delivered, so neither the positive nor the negative arm is meaningful.
Why: asset/content deployment through CloudFormation silently produces empty buckets.
Fix: the custom-resource execution path behind BucketDeployment (the provider's S3 writes), not the bucket state.
Traces: cloudformation__s3.deploy-cdk-bucket-deployment-denied-put-blocks-create-and-lands-nothing, cloudformation__s3.deploy-cdk-bucket-deployment-lands-prunes-and-retains-on-destroy.

Cause 5 of 8: the ListStacks StackStatusFilter enum refusal does not match AWS.
AWS: recorded behavior only — a filter list mixing a valid value with an unknown one is refused with AWS's enum ValidationError for that member path and value set.
Emulator: ValidationError "1 validation error detected: Value 'ACTIVE' at 'stackStatusFilter.2.member' failed to satisfy constraint: Member must satisfy enum value set: [...]" which differs from the recorded refusal.
Why: clients matching the published message do not recognise the refusal.
Fix: the enum-constraint message for ListStacks.StackStatusFilter.
Traces: cloudformation__s3.cfn-s3-list-stacks-with-unknown-status-filter-rejected.

Cause 6 of 8: a tags-only update of a bucket stack is rolled back.
AWS: recorded behavior only — an update that changes only the stack's tags succeeds (UPDATE_COMPLETE) while a true no-op update is refused with 'No updates are to be performed.'
Emulator: the tagged update settles UPDATE_ROLLBACK_COMPLETE.
Why: re-tagging a stack destroys the deploy instead of applying the tags.
Fix: the UpdateStack differ/tag application path.
Traces: cloudformation__s3.cfn-s3-no-op-update-of-bucket-stack-rejected.

Cause 7 of 8: GetTemplateSummary accepts a template AWS refuses.
AWS: recorded behavior only — GetTemplateSummary refuses a template whose Fn::Sub references a mapping value, with a ValidationError describing the unsupported reference.
Emulator: the call succeeds ({'Code': '', 'Message': 'the call succeeded'}).
Why: a client validating a template before deploying is told an invalid template is fine.
Fix: template validation in the GetTemplateSummary handler.
Traces: cloudformation__s3.e2e-expand-elasticsearch-cfn-template-lifecycle.

Cause 8 of 8: S3 fetch failures for a TemplateURL are reported with an internal message instead of AWS's S3 error text.
AWS: recorded behavior only — a TemplateURL whose object is missing is refused with ValidationError carrying S3's error text, e.g. 'S3 error: Access Denied\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html'.
Emulator: ValidationError 'S3 object does not exist. Bucket: <bucket>, Key: <key>. Error: NoSuchKey' — an emulator-specific sentence that also leaks which of bucket/key/version was missing.
Why: clients matching AWS's S3-error wording do not recognise the refusal.
Fix: the S3 template/stack-policy fetch error mapping shared by CreateStack/CreateStackSet.
Traces: cloudformation__s3.template-url-missing-object-rejected.

## Evidence
C1: ('NotImplemented', 'operation CreateStack with StackPolicyURL (fetching a stack policy from Amazon S3) is scaffolded but not implemented in this emulator'), and the UpdateStack StackPolicyURL / StackPolicyDuringUpdateURL equivalents; the missing-object check expected ('ValidationError', 'S3 object does not exist. Bucket: <b>, Key: <k>. Error: NoSuchKey').
C2: "ListStacks should list parent and child, got ['<parent>']"; "the nested stack must list as its own active stack named '<parent>-Kid-'...; got []"; "the nested stack id '<parent>-Child-<suffix>'".
C3: expected ('FAILED','UNAVAILABLE','The following hook(s)/validation failed: [AWS::EarlyValidation::ResourceExistenceCheck]. To troubleshoot Early Validation errors, use the DescribeEvents API for detailed failure information.'), got ('CREATE_COMPLETE','AVAILABLE',None); and 'validation must fail the change set: (CREATE_COMPLETE, None)'.
C4: "control: '<bucket>' holds [], expected the deployed {'payload.txt'}"; "after deploy, '<bucket>' holds keys [], expected exactly {'keep.txt','prune-me.txt'}".
C5: got ('ValidationError', "1 validation error detected: Value 'ACTIVE' at 'stackStatusFilter.2.member' failed to satisfy constraint: Member must satisfy enum value set: [...]").
C6: "the tagged update reached 'UPDATE_ROLLBACK_COMPLETE'".
C7: "GetTemplateSummary on the baseline returned {'Code': '', 'Message': 'the call succeeded'}; real AWS refuses its Fn::Sub reference to a mapping value".
C8: expected 'S3 error: Access Denied\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html', got 'S3 object does not exist. Bucket: <bucket>, Key: <key>. Error: NoSuchKey'.
Bucket/stack/key names are per-run fillers.

## Steps to Reproduce
Replay the checks in trace_ids: 1) CreateStack/UpdateStack with StackPolicyURL and StackPolicyDuringUpdateURL (valid and missing object); 2) deploy a stack with an AWS::CloudFormation::Stack child and call ListStacks/DescribeStackResource; 3) CreateChangeSet for a template whose bucket name already exists, with and without DisableValidation; 4) deploy a CDK app using BucketDeployment and list the destination bucket; 5) ListStacks with StackStatusFilter=['CREATE_COMPLETE','ACTIVE']; 6) UpdateStack changing only Tags; 7) GetTemplateSummary on a template whose Fn::Sub references a mapping value; 8) CreateStack/CreateStackSet with a TemplateURL whose object is missing.
