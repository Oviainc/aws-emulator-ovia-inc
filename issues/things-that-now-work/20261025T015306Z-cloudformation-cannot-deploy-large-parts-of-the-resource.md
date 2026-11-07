Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue cloudformation__iot__lambda.cfn-iot-authorizer-active-create-and-delete-with-stack cloudformation__iot__lambda.cfn-iot-authorizer-duplicate-name-rejected cloudformation.unrecognized-resource-type-rejected-up-front cloudformation__s3__sqs.deploy-cfn-s3-template-findinmap-drives-deployed-queue cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api cloudformation__sqs.cfn-resource-import-adopts-existing-queue-without-recreating-it cloudformation__sqs.cfn-execute-obsolete-changeset-rejected cloudformation.cdk-default-synthesizer-needs-bootstrap-without-user-assets cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift cloudformation.cdk-grant-read-data-permits-real-read-denies-real-write cloudformation__dynamodb.cfn-custom-named-resource-replacement-refused-and-data-survives cloudformation__dynamodb.deploy-dynamodb-2-deletion-protection-blocks-stack-delete cloudformation__dynamodb.deploy-dynamodb-2-update-rollback-removes-added-table cloudformation__dynamodb__lambda.cfn-sam-transform-function-table-end-to-end cloudformation__dynamodb__s3.cfn-stack-policy-protects-dynamodb-table-positive athena__cloudformation__glue.cfn-glue-table-drift-detected-after-out-of-band-column-add athena__cloudformation__glue.cfn-glue-table-explicit-external-table-type-round-trips athena__cloudformation__glue.cfn-glue-table-omits-table-type-and-view-fields athena__cloudformation__glue.cfn-stack-delete-releases-glue-table-and-its-provenance cloudformation.logstream-replacement-orphans-old-stream-in-former-group cloudformation.metricfilter-colliding-rename-rollback-keeps-unrelated-filter cloudformation__sns__sqs.stack-and-system-tags-land-on-created-resources
Issue-Key: cloudformation|CreateStack|ValidationError
Issue-Key: cloudformation__lambda__sqs|CreateStack|ValidationError
Issue-Key: cloudformation__iot__lambda|CreateStack|ValidationError
Issue-Key: cloudformation.unrecognized-resource-type-rejected-up-front|_client_error#15|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation__s3__sqs|CreateStack|NotImplemented
Issue-Key: cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive|_run_shell#19|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift|_cdk#16|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api|_cdk#15|AssertionError|assert rc == expect_rc, f"`cdk {args[0]}` rc={rc}, expected {expect_rc}: {out[-1200:]!r}"
Issue-Key: cloudformation|CreateChangeSet|NotImplemented
Issue-Key: cloudformation__sqs|DescribeChangeSet|ChangeSetNotFound
Issue-Key: dynamodb|PutItem|ResourceNotFoundException
Issue-Key: cloudformation__dynamodb|DescribeTable|ResourceNotFoundException
Issue-Key: cloudformation__dynamodb__lambda|DescribeTable|ResourceNotFoundException
Issue-Key: cloudformation__dynamodb__s3|DescribeTable|ResourceNotFoundException
Issue-Key: athena__cloudformation__glue|GetTable|EntityNotFoundException
Issue-Key: cloudformation.logstream-replacement-orphans-old-stream-in-former-group|run#14|AssertionError|assert _stream_names(logs, first_group) == {_STREAM_NAME, "unrelated"}, (
Issue-Key: cloudwatch-logs|PutMetricFilter|ResourceNotFoundException
Issue-Key: cloudformation__sns__sqs.stack-and-system-tags-land-on-created-resources|run#20|AssertionError|assert queue_tags == {"sim2real-env": "fidelity-check"}, (
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudFormation cannot deploy large parts of the resource surface: GuardHook/LambdaHook and AWS::IoT::Authorizer are unrecognized, TemplateURL and ChangeSetType=IMPORT are stubs, change sets are not addressable by ARN, DynamoDB/Glue/Logs resources are never provisioned, and aws:cloudformation:* system tags leak onto resources (10 causes)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Resolution note: causes 2 to 10 are served — the `AWS::Logs::LogGroup`/`LogStream`/`MetricFilter` and `AWS::IoT::Authorizer` providers were added for causes 2 and 9, and causes 3 to 8 and 10 were already served when this was fixed. Cause 1 is served only in part: `AWS::CloudFormation::GuardHook` and `AWS::CloudFormation::LambdaHook` are recognised resource types, so a template declaring one deploys, but nothing INVOKES a hook yet — no hook activation is stored, no Guard rule set is evaluated and no Lambda hook is called, so a `FAIL`-mode hook does not block a non-compliant resource and a `WARN`-mode one records no warning. That half needs a hook store and a Guard evaluation engine, and is still open: the trace ids `cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create`, `cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create` and `cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue` are not satisfied by this fix._

## Operation
CreateStack

## Description
This issue bundles 10 net-new root causes covering what CloudFormation can and cannot deploy. The open issue 20261023T000000Z-cloudformation-stacks-report-create-complete-but-provision-no-resources.md describes the never-provisioned family in general terms and several per-service issues exist (e.g. 20261023T004328Z for Lambda); none of them carries the replay keys below.

Cause 1 of 10: the CloudFormation Hooks resource types are unrecognized.
AWS: AWS::CloudFormation::GuardHook and AWS::CloudFormation::LambdaHook are registered resource types; a FAIL-mode hook blocks a non-compliant resource create and a WARN-mode hook allows it with a warning (Smithy models the hook fields on StackEvent: HookType, HookStatus, HookStatusReason, HookInvocationPoint, HookFailureMode).
Emulator: CreateStack answers 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::CloudFormation::GuardHook]' (and the same for LambdaHook), so no hook can be deployed or exercised.
Why: hook-enforced guardrails cannot be modelled at all, and the refusal is at the wrong layer (template format) for a type AWS supports.
Fix: register these types in the cloudformation resource-type registry and implement their providers plus hook invocation in the provisioning engine.
Traces: cloudformation.deep-cloudformation-1-cfn-enforce-fail-hook-blocks-unencrypted-queue-create, cloudformation.deep-cloudformation-1-cfn-enforce-warn-hook-flags-but-allows-unencrypted-queue-create, cloudformation__lambda__sqs.deploy-cloudformation-2-lambdahook-rejects-deny-queue.

Cause 2 of 10: AWS::IoT::Authorizer is unrecognized.
AWS: the type creates an IoT custom authorizer; a duplicate name is refused by IoT, and the authorizer is deleted with the stack.
Emulator: CreateStack answers ValidationError 'Template format error: Unrecognized resource types: [AWS::IoT::Authorizer]'.
Fix: register the type and implement its provider against the IoT service state.
Traces: cloudformation__iot__lambda.cfn-iot-authorizer-active-create-and-delete-with-stack, cloudformation__iot__lambda.cfn-iot-authorizer-duplicate-name-rejected.

Cause 3 of 10: conversely, a genuinely unknown resource type is NOT rejected up front.
AWS: ValidateTemplate/GetTemplateSummary refuse a template naming a type that does not exist.
Emulator: the call succeeds, returning {'Parameters': []} with HTTP 200.
Why: together with causes 1-2 the registry is wrong in both directions — supported types are refused and unsupported ones are accepted.
Fix: drive both paths from one resource-type registry.
Traces: cloudformation.unrecognized-resource-type-rejected-up-front.

Cause 4 of 10: TemplateURL (template fetched from S3) is an unimplemented stub on CreateStack and CreateChangeSet.
AWS: Smithy: CreateStack.TemplateURL and CreateChangeSet.TemplateURL (length 1-5120) are first-class alternatives to TemplateBody; CloudFormation fetches the object from S3.
Emulator: 'operation CreateStack with TemplateURL (fetching a template from Amazon S3) is scaffolded but not implemented in this emulator' / the same for CreateChangeSet, surfaced as NotImplemented and as `cdk deploy` rc=1 / `aws cloudformation deploy` rc=254.
Why: every CDK and `aws cloudformation deploy` workflow publishes the template to S3 first, so none of them can deploy. (The open issue 20261024T150703Z-cloudwatch-glue-lambda-logs-s3-sfn-cloudformation-createchangeset.md records the CreateChangeSet half for other check ids.)
Fix: implement TemplateURL fetching in the CreateStack/UpdateStack/CreateChangeSet handlers.
Traces: cloudformation__s3__sqs.deploy-cfn-s3-template-findinmap-drives-deployed-queue, cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive, cloudformation__ec2.cdk-diff-sees-a-template-change-and-is-blind-to-resource-drift, cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api.

Cause 5 of 10: CreateChangeSet with ChangeSetType=IMPORT is an unimplemented stub.
AWS: an IMPORT change set adopts an existing resource into a stack without recreating it.
Emulator: NotImplemented — 'operation CreateChangeSet with ChangeSetType IMPORT (importing existing resources) is scaffolded but not implemented'.
Fix: the CreateChangeSet handler's IMPORT path plus resource adoption in the stack state.
Traces: cloudformation__sqs.cfn-resource-import-adopts-existing-queue-without-recreating-it.

Cause 6 of 10: a change set cannot be addressed by its own ARN.
AWS: DescribeChangeSet accepts the change-set ARN that CreateChangeSet returned (Smithy: ChangeSetNameOrId pattern admits 'arn:...').
Emulator: answers 404 ChangeSetNotFound for the ARN it just issued.
Why: the documented way to track a change set (by Id) does not work.
Fix: change-set lookup by ARN in the DescribeChangeSet/ExecuteChangeSet handlers.
Traces: cloudformation__sqs.cfn-execute-obsolete-changeset-rejected.

Cause 7 of 10: AWS::DynamoDB::Table is never created in DynamoDB.
AWS: after CREATE_COMPLETE the table exists; DescribeTable/PutItem against it succeed.
Emulator: the stack completes but DynamoDB answers ResourceNotFoundException ('Requested resource not found' / 'Table: <name> not found') for the table the stack declared.
Fix: the AWS::DynamoDB::Table provider must create the table in the DynamoDB service state.
Traces: cloudformation.cdk-default-synthesizer-needs-bootstrap-without-user-assets, cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift, cloudformation.cdk-grant-read-data-permits-real-read-denies-real-write, cloudformation__dynamodb.cfn-custom-named-resource-replacement-refused-and-data-survives, cloudformation__dynamodb.deploy-dynamodb-2-deletion-protection-blocks-stack-delete, cloudformation__dynamodb.deploy-dynamodb-2-update-rollback-removes-added-table, cloudformation__dynamodb__lambda.cfn-sam-transform-function-table-end-to-end, cloudformation__dynamodb__s3.cfn-stack-policy-protects-dynamodb-table-positive.

Cause 8 of 10: AWS::Glue::Database / AWS::Glue::Table are never created in Glue.
AWS: the stack's Glue database and table are readable with glue:GetTable after CREATE_COMPLETE, including TableType/view fields and their stack provenance, and are released on stack delete.
Emulator: glue:GetTable answers EntityNotFoundException 'Database <name> not found.'
Fix: the AWS::Glue::Database/Table providers must create the entities in the Glue service state.
Traces: the four athena__cloudformation__glue.cfn-glue-* / cfn-stack-delete-releases-glue-table-and-its-provenance checks. (Service attribution: these check ids carry a compound token; the failing call is glue:GetTable after a CloudFormation deploy, and the defect is CloudFormation's resource provider, so the issue is filed under cloudformation. The athena half of those checks is blocked by the separately filed athena StartQueryExecution stub.)

Cause 9 of 10: AWS::Logs::LogGroup / AWS::Logs::LogStream are never created in CloudWatch Logs.
AWS: the declared log group and stream exist after CREATE_COMPLETE; a replacement moves the stream and leaves the old one in its former group; PutMetricFilter against the declared group succeeds.
Emulator: the first group lists only an unrelated stream, and logs:PutMetricFilter answers ResourceNotFoundException 'The specified log group does not exist.'
Fix: the AWS::Logs::* providers must create the resources in the Logs service state.
Traces: cloudformation.logstream-replacement-orphans-old-stream-in-former-group, cloudformation.metricfilter-colliding-rename-rollback-keeps-unrelated-filter.

Cause 10 of 10: the aws:cloudformation:* system tags are propagated onto created resources that AWS does not tag.
AWS: CloudFormation does not propagate aws:cloudformation:logical-id / stack-id / stack-name to AWS::SQS::Queue; the queue carries only the stack's own user tags.
Emulator: the queue carries all three system tags in addition to the user tag.
Why: callers that enumerate a resource's tags (cost allocation, tag-based policy) see tags AWS never sets, and the reserved aws: namespace is populated by the emulator itself.
Fix: the tag-propagation step of the provisioning engine must apply the per-resource-type propagation rules.
Traces: cloudformation__sns__sqs.stack-and-system-tags-land-on-created-resources.

## Evidence
Replayed suite:
- GuardHook/LambdaHook: CreateStack -> 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::CloudFormation::GuardHook]' / '[AWS::CloudFormation::LambdaHook]'.
- IoT: 'Unrecognized resource types: [AWS::IoT::Authorizer]'.
- unrecognized-resource-type-rejected-up-front: "real AWS rejects this call, but it SUCCEEDED and returned {'Parameters': [], ... 'HTTPStatusCode': 200}".
- TemplateURL: "An error occurred (NotImplemented) when calling the CreateStack operation: operation CreateStack with TemplateURL (fetching a template from Amazon S3) is scaffolded but not implemented in this emulator"; `cdk deploy` rc=1 ending in 'operation CreateChangeSet with TemplateURL ... is scaffolded but not implemented'; `aws` CLI rc=254.
- IMPORT: "(NotImplemented) ... operation CreateChangeSet with ChangeSetType IMPORT (importing existing resources) is scaffolded but not implemented".
- change-set ARN: "(ChangeSetNotFound) when calling the DescribeChangeSet operation: ChangeSet [arn:aws:cloudformation:us-west-2:000000000000:changeSet/...] does not exist".
- DynamoDB: "(ResourceNotFoundException) when calling the PutItem operation: Requested resource not found" and "... DescribeTable ... Table: <name> not found" on CREATE_COMPLETE stacks.
- Glue: "(EntityNotFoundException) when calling the GetTable operation: Database <name> not found."
- Logs: "(ResourceNotFoundException) when calling the PutMetricFilter operation: The specified log group does not exist."; "first group streams are {'unrelated'}".
- System tags: got {'aws:cloudformation:logical-id': 'Queue', 'aws:cloudformation:stack-id': 'arn:...', 'aws:cloudformation:stack-name': '...', 'sim2real-env': 'fidelity-check'} where only the user tag was owed.

## Steps to Reproduce
1. Boot the emulator, replay the suite, re-run the trace_ids.
2. Cause 4 repro: PutObject a template to S3 and CreateStack(TemplateURL=...) — NotImplemented.
3. Cause 7 repro: CreateStack with an AWS::DynamoDB::Table, wait CREATE_COMPLETE, then dynamodb:DescribeTable on the declared name.
4. Cause 8 repro: CreateStack with AWS::Glue::Database + AWS::Glue::Table, then glue:GetTable.
5. Cause 10 repro: CreateStack with an AWS::SQS::Queue and one stack tag, then sqs:ListQueueTags.
