Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3.cfn-s3-list-stacks-with-unknown-status-filter-rejected cloudformation__s3.cfn-s3-nested-child-stack-counts-for-its-own-bucket cloudformation__s3.cfn-topstacks-nested-stack-lists-as-its-own-prefixed-active-stack cloudformation__s3.stack-nested-child-update-failure-reason-chain cloudformation__s3.cfn-s3-no-op-update-of-bucket-stack-rejected cloudformation__s3.change-set-disable-validation-defers-the-bucket-name-collision-to-execution cloudformation__s3.change-set-pre-deployment-validation-rejects-an-existing-bucket-name cloudformation__s3.e2e-expand-elasticsearch-cfn-template-lifecycle cloudformation__s3.stack-create-stack-policy-url-applied-and-validated cloudformation__s3.stack-update-stack-policy-during-update-url-overrides-once cloudformation__s3.stack-update-stack-policy-url-replaces-stored-policy cloudformation__s3.template-url-missing-object-rejected
Issue-Key: cloudformation__s3.cfn-s3-list-stacks-with-unknown-status-filter-rejected|run#8|AssertionError|assert got == ("ValidationError", _MIXED_FILTER_MESSAGE), (
Issue-Key: cloudformation__s3.cfn-s3-nested-child-stack-counts-for-its-own-bucket|run#19|AssertionError|assert len(summaries) == 2, (
Issue-Key: cloudformation__s3.cfn-topstacks-nested-stack-lists-as-its-own-prefixed-active-stack|run#9|AssertionError|assert len(kids) == 1 and kids[0].startswith(parent + "-Kid-"), (
Issue-Key: cloudformation__s3.stack-nested-child-update-failure-reason-chain|run#9|AssertionError|assert re.fullmatch(rf"arn:aws[a-z-]*:cloudformation:{ctx.region}:{ctx.account}:stack/{parent_name}-Child-[A-Za-z0-9]+/"
Issue-Key: cloudformation__s3.cfn-s3-no-op-update-of-bucket-stack-rejected|run#14|AssertionError|assert status == "UPDATE_COMPLETE", f"the tagged update reached {status!r}"
Issue-Key: cloudformation__s3.change-set-disable-validation-defers-the-bucket-name-collision-to-execution|run#29|AssertionError|assert reason_pattern.match(failed[0]), f"bucket CREATE_FAILED reason: {failed[0]!r}"
Issue-Key: cloudformation__s3.change-set-pre-deployment-validation-rejects-an-existing-bucket-name|run#23|AssertionError|assert failures == [(
Issue-Key: cloudformation__s3.e2e-expand-elasticsearch-cfn-template-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudformation__s3.stack-create-stack-policy-url-applied-and-validated|_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cloudformation__s3.stack-update-stack-policy-during-update-url-overrides-once|run#17|AssertionError|assert denied == [("UPDATE_ROLLBACK_IN_PROGRESS", "Action denied by stack policy: Statement [#1] does not "
Issue-Key: cloudformation__s3.stack-update-stack-policy-url-replaces-stored-policy|run#16|AssertionError|assert denied == [(name, "UPDATE_ROLLBACK_IN_PROGRESS", "Action denied by stack policy: Statement [#1] "
Issue-Key: cloudformation__s3.template-url-missing-object-rejected|run#26|AssertionError|assert got[0] == "ValidationError" and stack_set_message_re.match(got[1]), (
Issue-Commit: 0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9

# cloudformation__s3: 10 net-new causes — nested stacks are invisible to ListStacks and carry a non-ARN physical id, stack-policy enforcement and its denial reasons diverge, and five refusal messages/fields are wrong

> _Note: this issue was identified against the code base at commit `0de1c688e6ce96ac9f39fea8e8ed5bb261aa89c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStacks

## Description
Ten independent root causes, all grounded in failing behavioral checks listed under `### cloudformation__s3`. A fixer must close all ten before submitting; each cause names its own checks.

Cause 1 of 10: ListStacks' refusal of an unknown StackStatusFilter member quotes the wrong value, path and enum order.
AWS: Smithy: ListStacks.StackStatusFilter is a list of StackStatus; the whole filter is voided and refused with 400 ValidationError whose message is `1 validation error detected: Value '[<v1>, <v2>, ...]' at 'stackStatusFilter' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [REVIEW_IN_PROGRESS, CREATE_FAILED, UPDATE_ROLLBACK_FAILED, UPDATE_ROLLBACK_IN_PROGRESS, CREATE_IN_PROGRESS, IMPORT_ROLLBACK_IN_PROGRESS, UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS, ROLLBACK_IN_PROGRESS, IMPORT_IN_PROGRESS, DELETE_COMPLETE, UPDATE_COMPLETE, UPDATE_IN_PROGRESS, DELETE_FAILED, IMPORT_COMPLETE, UPDATE_FAILED, DELETE_IN_PROGRESS, ROLLBACK_COMPLETE, ROLLBACK_FAILED, IMPORT_ROLLBACK_COMPLETE, UPDATE_COMPLETE_CLEANUP_IN_PROGRESS, CREATE_COMPLETE, IMPORT_ROLLBACK_FAILED, UPDATE_ROLLBACK_COMPLETE]]`. The only variable part is the bracketed list, which is the request's own StackStatusFilter in the order given — neither sorted nor deduplicated; the enum list is fixed text in that order.
Emulator: quotes only the offending member and indexes it, and drops the outer wrapper: `1 validation error detected: Value 'ACTIVE' at 'stackStatusFilter.2.member' failed to satisfy constraint: Member must satisfy enum value set: [CREATE_IN_PROGRESS, CREATE_FAILED, CREATE_COMPLETE, ...]` — the enum rendered in the model's declaration order and `Member must satisfy constraint: [...]` missing.
Why: a client that reports or matches the refusal sees a different path, a different quoted value and a differently ordered value set.
Fix: ListStacks' input validation and the shared enum-constraint message formatter (list-member refusals must quote the whole list at the member's own path).
Checks: cloudformation__s3.cfn-s3-list-stacks-with-unknown-status-filter-rejected. (The open issue 20261102T101202Z-cloudformation-10-net-new-root-causes-stack-level.md carries the same shape under the `cloudformation` heading; it does not cover this check.)

Cause 2 of 10: nested stacks never appear in ListStacks, so they carry no ParentId/RootId.
AWS: a stack created by an AWS::CloudFormation::Stack resource is itself a stack, named `<parentName>-<logicalId>-<suffix>`. A status-filtered ListStacks returns a StackSummary for it alongside the parent; Smithy: StackSummary.ParentId is 'the stack ID of the direct parent of this stack' and StackSummary.RootId 'the stack ID of the top-level stack', both equal to the parent's StackId for a first-level child, while the root stack's summary carries no ParentId. The parent still counts the nested stack as exactly one resource of type AWS::CloudFormation::Stack.
Emulator: ListStacks returns the parent only; the child is absent, so ParentId/RootId can never be read.
Why: any inventory built from ListStacks under-counts stacks and loses the parent/root relationship entirely.
Fix: the nested-stack provisioner must register child stacks in the stack store as stacks of their own with ParentId/RootId set, and ListStacks' projection must emit them.
Checks: cloudformation__s3.cfn-s3-nested-child-stack-counts-for-its-own-bucket, cloudformation__s3.cfn-topstacks-nested-stack-lists-as-its-own-prefixed-active-stack.

Cause 3 of 10: a nested stack's PhysicalResourceId is a bare name, not a stack ARN.
AWS: DescribeStackResource for an AWS::CloudFormation::Stack resource returns PhysicalResourceId equal to the child's StackId: `arn:<partition>:cloudformation:<region>:<account>:stack/<parentName>-<logicalId>-<suffix>/<uuid>`.
Emulator: returns the bare generated name `<parentName>-<logicalId>-<suffix>` with no arn/partition/region/account/uuid, so it cannot be used as a StackName to read the child back.
Why: a caller that follows the parent's resource to the child stack (the documented way to inspect nested stacks) cannot address it.
Fix: the AWS::CloudFormation::Stack resource provider's physical-id assignment.
Checks: cloudformation__s3.stack-nested-child-update-failure-reason-chain.

Cause 4 of 10: an UpdateStack that only adds Tags to an existing AWS::S3::Bucket rolls the stack back.
AWS: recorded behavior only. Adding a `Tags` property to an already-created AWS::S3::Bucket is an update with no interruption; the stack settles UPDATE_COMPLETE and the bucket keeps its identity.
Emulator: the stack settles UPDATE_ROLLBACK_COMPLETE — the tag-only update of the bucket fails and is rolled back.
Why: a tagging update, one of the most common no-interruption changes, cannot be applied to a CloudFormation-managed bucket; the stack also ends in a status callers treat as a failed deployment.
Fix: the AWS::S3::Bucket resource provider's update path for the Tags property (and the update executor that reports its failure).
Checks: cloudformation__s3.cfn-s3-no-op-update-of-bucket-stack-rejected (its earlier no-op refusal — ValidationError 'No updates are to be performed.' and the unchanged CREATE_COMPLETE/absent LastUpdatedTime — already matches; only the positive-control tagged update diverges).

Cause 5 of 10: the CREATE_FAILED reason for an S3 bucket-name collision carries S3's prose instead of the resource handler's message.
AWS: the failing bucket's ResourceStatusReason is `Resource handler returned message: "<bucketName> already exists (Service: S3, Status Code: 0, Request ID: null)" (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)`. Fixed text apart from the bucket name (from the template) and the RequestToken (one per call).
Emulator: `Resource handler returned message: "Your previous request to create the named bucket succeeded and you already own it." (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)` — the inner message is S3's BucketAlreadyOwnedByYou sentence rather than `<bucketName> already exists (Service: S3, Status Code: 0, Request ID: null)`, and it names no bucket.
Why: the event does not say which bucket name collided, and a client matching the handler's documented wrapping fails.
Fix: the AWS::S3::Bucket resource provider's create-failure mapping (how it wraps the S3 refusal into ResourceStatusReason).
Checks: cloudformation__s3.change-set-disable-validation-defers-the-bucket-name-collision-to-execution.

Cause 6 of 10: DescribeEvents' VALIDATION_ERROR event omits ValidationPath.
AWS: Smithy: OperationEvent.ValidationPath is 'The path within the resource where the validation was applied'; for a NAME_CONFLICT_VALIDATION failure on a template resource it is `/Resources/<LogicalResourceId>`.
Emulator: every other field of the event matches (EventType VALIDATION_ERROR, LogicalResourceId, ResourceType, ValidationName NAME_CONFLICT_VALIDATION, ValidationStatus FAILED, ValidationFailureMode FAIL, ValidationStatusReason `Resource of type '<type>' with identifier '<id>' already exists.`) but ValidationPath is absent.
Why: a caller cannot locate the failing element inside the template, which is the field's only purpose.
Fix: the early-validation event emitter for CreateChangeSet (the DescribeEvents serializer must populate ValidationPath).
Checks: cloudformation__s3.change-set-pre-deployment-validation-rejects-an-existing-bucket-name.

Cause 7 of 10: Fn::Sub accepts a dotted reference that names a mapping path.
AWS: Fn::Sub resolves `${Logical.Attribute}` as a resource attribute only; a three-part reference naming a mapping value is refused by GetTemplateSummary with 400 ValidationError `Template error: instance of Fn::Sub references invalid resource attribute <reference>`, where `<reference>` is the dotted string from the template and the rest is fixed text.
Emulator: GetTemplateSummary returns 200 for such a template, accepting any `${A.B.C}`.
Why: a template that real AWS refuses up front is accepted here, so the error surfaces only at provisioning (or not at all) and templates validated against the emulator are not deployable.
Fix: Fn::Sub validation in the template parser used by GetTemplateSummary/ValidateTemplate/CreateChangeSet.
Checks: cloudformation__s3.e2e-expand-elasticsearch-cfn-template-lifecycle.

Cause 8 of 10: UpdateStack does not pre-validate the stored stack policy against the submitted template.
AWS: when the stored stack policy names a logical id the submitted template does not declare, UpdateStack is refused outright with 400 ValidationError `Error validating existing stack policy: Unknown logical id 'LogicalResourceId/<id>' in statement {} - stack policies can only be applied to logical ids referenced in the template`; the stack keeps its previous status and emits no stack events. Documented at https://docs.aws.amazon.com/AWSCloudFormation/latest/APIReference/API_CreateStack.html (StackPolicyURL).
Emulator: UpdateStack returns 200 (StackId plus an OperationId) and proceeds with the update.
Why: an update AWS rejects before touching anything is executed here, and a client relying on the refusal to protect the policy-protected resource loses that protection.
Fix: UpdateStack's handler validation of the stored stack policy against the submitted template.
Checks: cloudformation__s3.stack-create-stack-policy-url-applied-and-validated (the CreateStack StackPolicyURL path and GetStackPolicy round-trip before it already match).

Cause 9 of 10: the stack-policy denial event names the attempted change and a 0-based statement index instead of the denying statement.
AWS: the stack-level UPDATE_ROLLBACK_IN_PROGRESS event's ResourceStatusReason is `Action denied by stack policy: Statement [#<n>] does not allow [<the statement's Action>] for resource [<the statement's Resource>];` — `<n>` is the 1-based index of the denying statement in the stored policy, and the action and resource are that DENY statement's own patterns, not the change being attempted. No DELETE_* event is emitted for the protected resource.
Emulator: emits `Action denied by stack policy: Statement [#0] does not allow [Update:Delete] for resource [LogicalResourceId/<logicalId>];` — a 0-based index, with the attempted action and the target resource's logical id substituted for the statement's Action and Resource.
Why: the reason no longer identifies which policy statement blocked the update or what it actually forbids, which is the only thing it exists to say.
Fix: the stack-policy evaluator's denial-reason formatting in the update executor.
Checks: cloudformation__s3.stack-update-stack-policy-during-update-url-overrides-once, cloudformation__s3.stack-update-stack-policy-url-replaces-stored-policy.

Cause 10 of 10: CreateStackSet's refusal of a TemplateURL naming a missing S3 key carries the wrong message.
AWS: the refusal is 400 ValidationError with message `S3 object does not exist. Bucket: <bucket>, Key: <key>. Error: NoSuchKey (Service: AmazonCloudFormation; Status Code: 400; Error Code: ValidationError; Request ID: <uuid>; Proxy: null)` — the same fetch sentence GetTemplateSummary and EstimateTemplateCost return (both of which the emulator already matches), plus the StackSets control-plane suffix whose only variable part is the request id. No stack set is created (DescribeStackSet then answers StackSetNotFoundException `StackSet <name> not found`).
Emulator: `S3 error: The specified key does not exist.\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html` — S3's own error text leaked through, naming neither bucket nor key and carrying no StackSets suffix.
Why: the same missing object produces three different messages across the three operations that fetch a TemplateURL, and the one from CreateStackSet does not say which object is missing.
Fix: CreateStackSet's template-fetch error mapping (reuse the shared S3 fetch refusal and add the StackSets control-plane suffix).
Checks: cloudformation__s3.template-url-missing-object-rejected.

## Evidence
Cause 1 — ListStacks(StackStatusFilter=['CREATE_COMPLETE','ACTIVE']):
Expected ('ValidationError', "1 validation error detected: Value '[CREATE_COMPLETE, ACTIVE]' at 'stackStatusFilter' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [REVIEW_IN_PROGRESS, CREATE_FAILED, UPDATE_ROLLBACK_FAILED, ...]]")
Actual   ('ValidationError', "1 validation error detected: Value 'ACTIVE' at 'stackStatusFilter.2.member' failed to satisfy constraint: Member must satisfy enum value set: [CREATE_IN_PROGRESS, CREATE_FAILED, CREATE_COMPLETE, ...]")

Cause 2 — ListStacks(StackStatusFilter=['CREATE_COMPLETE','UPDATE_COMPLETE']) after creating a parent with a nested child:
Expected both the parent and `<parent>-<logicalId>-<suffix>`, the child's summary carrying ParentId=RootId=<parent StackId>.
Actual   ['sim2real-cfns3-nest-<id>-parent'] (child missing); second check: the non-parent entries were [].

Cause 3 — DescribeStackResource(StackName=<parent>, LogicalResourceId='Child').PhysicalResourceId:
Expected `arn:aws:cloudformation:<region>:<account>:stack/<parent>-Child-<suffix>/<uuid>`
Actual   'sim2real-cfn-nest-6f0555866de9-Child-AC475E18CA63'

Cause 4 — UpdateStack adding Tags=[{Key,Value}] to the stack's AWS::S3::Bucket:
Expected terminal status 'UPDATE_COMPLETE'; Actual 'UPDATE_ROLLBACK_COMPLETE'.

Cause 5 — CREATE_FAILED ResourceStatusReason for the colliding bucket:
Expected `Resource handler returned message: "<bucket> already exists (Service: S3, Status Code: 0, Request ID: null)" (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)`
Actual   `Resource handler returned message: "Your previous request to create the named bucket succeeded and you already own it." (RequestToken: 86628381-..., HandlerErrorCode: AlreadyExists)`

Cause 6 — DescribeEvents(ChangeSetName=<id>) VALIDATION_ERROR tuple (EventType, LogicalResourceId, ResourceType, ValidationName, ValidationStatus, ValidationFailureMode, ValidationStatusReason, ValidationPath):
Expected (..., '/Resources/Bucket'); Actual (..., None).

Cause 7 — GetTemplateSummary(TemplateURL=<template whose AccessPolicies uses !Sub 'arn:aws:iam::${StageConfig.alpha.LambdaAccount1}:root'>):
Expected ('ValidationError', 'Template error: instance of Fn::Sub references invalid resource attribute StageConfig.alpha.LambdaAccount1'); Actual HTTP 200 (the call succeeded).

Cause 8 — UpdateStack(TemplateBody=<template omitting the policy-protected logical id 'Keep'>) on a stack whose stored policy denies Update:Delete on LogicalResourceId/Keep:
Expected ('ValidationError', "Error validating existing stack policy: Unknown logical id 'LogicalResourceId/Keep' in statement {} - stack policies can only be applied to logical ids referenced in the template"), the stack left UPDATE_COMPLETE with no new events.
Actual   HTTP 200 with {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-cfn-cpurl-<id>/<uuid>', 'OperationId': '<uuid>'}.

Cause 9 — stored policy [{Deny,Update:Delete,*},{Allow,Update:*,*}], UpdateStack removing resource C:
Expected stack-level event ('UPDATE_ROLLBACK_IN_PROGRESS', 'Action denied by stack policy: Statement [#1] does not allow [Update:Delete] for resource [*];')
Actual   ('UPDATE_ROLLBACK_IN_PROGRESS', 'Action denied by stack policy: Statement [#0] does not allow [Update:Delete] for resource [LogicalResourceId/C];')
Second check (deny-all stored policy, removing 'Keep'): expected 'Statement [#1] does not allow [Update:*] for resource [*];', actual 'Statement [#0] does not allow [Update:Delete] for resource [LogicalResourceId/Keep];'.

Cause 10 — CreateStackSet(StackSetName=<n>, TemplateURL=https://s3.<region>.amazonaws.com/<bucket>/absent.json):
Expected ('ValidationError', 'S3 object does not exist. Bucket: <bucket>, Key: absent.json. Error: NoSuchKey (Service: AmazonCloudFormation; Status Code: 400; Error Code: ValidationError; Request ID: <uuid>; Proxy: null)')
Actual   ('ValidationError', 'S3 error: The specified key does not exist.\nFor more information check http://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html')

All bucket/stack/parameter names, request ids and asset hashes above are per-run values filling the message templates, not requirements.

## Steps to Reproduce
Cause 1: ListStacks(StackStatusFilter=['CREATE_COMPLETE','ACTIVE']) and again with the two values reversed; compare Error.Code/Message.
Cause 2: CreateStack a parent whose template holds an AWS::CloudFormation::Stack child (TemplateURL in S3) plus a WaitConditionHandle; after CREATE_COMPLETE page ListStacks(StackStatusFilter=['CREATE_COMPLETE','UPDATE_COMPLETE']) and look for `<parent>-<logicalId>-<suffix>` and its ParentId/RootId.
Cause 3: for that same parent, DescribeStackResource(LogicalResourceId=<child logical id>) and inspect PhysicalResourceId.
Cause 4: CreateStack a template with one AWS::S3::Bucket; after CREATE_COMPLETE, UpdateStack with the same template plus Tags on the bucket and poll the status.
Cause 5: create a bucket out of band; CreateChangeSet (DisableValidation) for a stack declaring that BucketName, execute it, and read the bucket's CREATE_FAILED ResourceStatusReason from DescribeStackEvents.
Cause 6: create a bucket out of band; CreateChangeSet(ChangeSetType='CREATE') for a stack declaring that BucketName and call DescribeEvents(ChangeSetName=<id>); inspect the VALIDATION_ERROR event's ValidationPath.
Cause 7: GetTemplateSummary(TemplateURL=<template using Fn::Sub with a three-part ${Map.Key.Sub} reference>).
Cause 8: CreateStack with StackPolicyURL denying Update:Delete on LogicalResourceId/Keep; UpdateStack with a template that omits Keep.
Cause 9: CreateStack with StackPolicyBody denying Update:Delete (or Update:*) on '*'; UpdateStack removing a resource and read the stack-level UPDATE_ROLLBACK_IN_PROGRESS event's ResourceStatusReason.
Cause 10: CreateStackSet(TemplateURL=<s3 url of a key that does not exist>) and compare Error.Message with GetTemplateSummary/EstimateTemplateCost for the same URL.
Replay: the twelve check ids in trace_ids.
