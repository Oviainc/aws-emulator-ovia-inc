Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__servicecatalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action
Issue-Key: cloudformation__servicecatalog-appregistry|ListTagsForResource|NotImplemented
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# SNS ListTagsForResource is an unimplemented scaffold stub: returns NotImplemented instead of the resource's tag list, blocking all cross-service tag-propagation verification

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
One root cause.

Cause 1 of 1: Amazon SNS `ListTagsForResource` is scaffolded but not implemented, so it answers an off-model `NotImplemented` error for every call, including for a topic that demonstrably exists in the emulator.

AWS: Smithy (services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json) models `ListTagsForResource` with input `ListTagsForResourceRequest{ResourceArn: AmazonResourceName (required, @length 1..1011)}` and output `ListTagsForResourceResponse{Tags: TagList}`. Its only modeled errors are AuthorizationErrorException (403, awsQueryError AuthorizationError), ConcurrentAccessException (400, ConcurrentAccess), InvalidParameterException (400, InvalidParameter), ResourceNotFoundException (404, ResourceNotFound) and TagPolicyException (400, TagPolicy). For an existing topic ARN, AWS returns HTTP 200 with the complete tag set for that topic — which, for a topic created by a CloudFormation stack, includes the stack's own resource-level tags plus the CloudFormation-injected `aws:cloudformation:stack-name`, `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id` tags. `NotImplemented` is not a modeled error of this operation.

Emulator: the call fails with `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator`. The topic was created successfully moments earlier as a CloudFormation stack resource (CFN CreateStack/DescribeStacks succeeded and the stack reached CREATE_COMPLETE, and the stack output exposed the topic ARN), so this is not a missing-resource condition — the operation simply has no implementation behind it.

Why: a client cannot read SNS topic tags at all. Any tag-driven workflow breaks: cost-allocation/ownership tag reads, tag-based access or policy checks, `awsApplication`-style application-association discovery, idempotent reconcilers that diff desired vs. actual tags, and any assertion that CloudFormation propagated stack tags onto the topic it owns. Worse, the failure surfaces as `NotImplemented` rather than a modeled SNS error, so SDK retry/error-handling paths written against the modeled error set (e.g. catching ResourceNotFoundException) do not match, and the exception escapes as an unmodeled client error. This single stub also makes an entire class of cross-service fidelity unverifiable: the behavioral check uses SNS tag listing as the independent read-back oracle for CloudFormation tag propagation and for AppRegistry's SyncResource no-action contract, and it aborts at the first SNS tag read before the AppRegistry `SyncResource` CFN_STACK / RESOURCE_TAG_VALUE assertions are ever exercised.

Fix: implement the `ListTagsForResource` handler in the SNS service crate (services/service-com-amazonaws-sns-amazonsimplenotificationservice) — resolve `ResourceArn` against SNS topic state, return 200 with `Tags` as the topic's stored tag list (empty list when the topic has no tags), and raise the modeled `ResourceNotFoundException` (404 / awsQueryError `ResourceNotFound`) for an ARN that names no existing topic and `InvalidParameterException` (400 / `InvalidParameter`) for a malformed ARN. This requires SNS topic tag state to be populated by topic creation and by the CloudFormation stack-resource provisioning path (stack tags plus the `aws:cloudformation:*` tags), so the read has something truthful to return.

Traces: cloudformation__service-catalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action (errors at the first `sns.list_tags_for_resource(ResourceArn=<topic arn>)` call).

Note on attribution: the failing check id's leading token is the composite cross-service token `cloudformation__service-catalog-appregistry` (its SERVICES list is ["cloudformation", "service-catalog-appregistry"]), but the diverging operation and the defect both belong to SNS, which is stood up in this build. No existing open issue covers it; the three open ListTagsForResource issues are for other services (launch-wizard, mediapackagev2, states/sfn).

## Evidence
Expected (AWS / Smithy ListTagsForResourceResponse): HTTP 200 with body {"Tags": [...]} listing the topic's tags. For a topic created by a CFN stack this includes the stack's resource tags (example from the recording: sim2real:managed=true, sim2real:prefix=sim2real-xsvc) and CloudFormation's injected tags aws:cloudformation:stack-name=<stack name>, aws:cloudformation:logical-id=ProbeTopic. Modeled error set: AuthorizationErrorException(403), ConcurrentAccessException(400), InvalidParameterException(400), ResourceNotFoundException(404), TagPolicyException(400).

Actual (emulator): error code `NotImplemented` with message `operation ListTagsForResource is scaffolded but not implemented in this emulator` (example text), raised as botocore ClientError:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator

`NotImplemented` is not in the operation's modeled error set, and the target topic existed (created via CloudFormation, stack reached CREATE_COMPLETE, topic ARN read from the stack's TopicArn output) so no modeled error applied either.

Downstream effect observed: the check aborts before reaching its AppRegistry assertions, so SyncResource(resourceType=CFN_STACK) actionTaken/resourceArn/applicationArn behavior and the SyncResource(resourceType=RESOURCE_TAG_VALUE) ValidationException rejection are left unexercised by this run.

## Steps to Reproduce
1. Boot the emulator and create an SNS topic that carries tags — e.g. deploy a CloudFormation stack with Tags set and a single `AWS::SNS::Topic` resource (logical id ProbeTopic) that outputs the topic ARN, and wait for CREATE_COMPLETE.
2. Read the topic ARN from the stack's outputs via `cloudformation describe-stacks`.
3. Call `sns list-tags-for-resource --resource-arn <topic arn>`.
4. Observe: the call fails with error code `NotImplemented` instead of returning HTTP 200 with a `Tags` list containing the topic's tags.
5. Equivalent minimal repro without CFN: `sns create-topic --name t1 --tags Key=k,Value=v`, then `sns list-tags-for-resource --resource-arn <topic arn>` — same `NotImplemented`.
6. Replay the behavioral check `cloudformation__service-catalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action` to confirm the fix; it must get past its first SNS tag read.
