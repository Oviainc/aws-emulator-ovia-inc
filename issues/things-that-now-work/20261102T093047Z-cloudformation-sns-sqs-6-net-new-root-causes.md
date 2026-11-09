Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event cloudformation__sns__sqs.cdk-sibling-nested-stacks-deliver-topic-to-queue-across-a-producer-only-update cloudformation__sns__sqs.change-set-empty-notification-arns-stops-delivery-until-restored cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails cloudformation__sns__sqs.custom-resource-service-timeout-out-of-range-fails-the-resource
Issue-Key: cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event|run#23|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in described.get("Outputs", [])}
Issue-Key: cloudformation|DescribeStackResources|ValidationError
Issue-Key: cloudformation__sns__sqs.change-set-empty-notification-arns-stops-delivery-until-restored|_collect#7|AssertionError|raise AssertionError(f"events never delivered: {sorted(want_ids - set(got))!r}")
Issue-Key: cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails|pull#10|Timeout|for msg in resp.get("Messages", []):
Issue-Key: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails|pull#9|Timeout|for msg in resp.get("Messages", []):
Issue-Key: cloudformation__sns__sqs.custom-resource-service-timeout-out-of-range-fails-the-resource|pull#8|Timeout|for msg in resp.get("Messages", []):
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# cloudformation__sns__sqs: 6 net-new root causes — stack Outputs omit OutputValue when AWS::NotificationARNs resolves, nested stacks are not real child stacks, change-set NotificationARNs publish nothing, and three custom-resource rules (GetAtt missing Data key, 4096-byte response limit, ServiceTimeout range) are unimplemented

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
This issue bundles SIX net-new root causes of service `cloudformation__sns__sqs`. All 6 must be closed before submitting. None of the six grounding checks was marked ALREADY FILED or OWNED by the replay report; the two open issues in this service (20261029T183635Z-cloudformation-sns-sqs-a-stack-s-notificationarns-never.md and 20261101T212352Z-cloudformation-sns-sqs-a-custom-resource-whose-servicetoken.md) are named per cause where they describe a neighbouring gap, so a fixer can close them together, but neither carries any of these six check ids.

=== Cause 1 of 6: a stack Output whose Value resolves through the AWS::NotificationARNs pseudo-parameter is returned WITHOUT its OutputValue member ===
AWS: Smithy (com.amazonaws.cloudformation#Output) declares the Output structure's members as OutputKey, OutputValue, Description and ExportName. Every Output of a stack that has reached CREATE_COMPLETE is returned by DescribeStacks with BOTH OutputKey and OutputValue; AWS resolves `{"Fn::Join": [",", {"Ref": "AWS::NotificationARNs"}]}` to the comma-joined list of the stack's notification topic ARNs (and to the empty string when the list is empty), so OutputValue is present either way. Recorded behavior only for the pseudo-parameter's resolution (the model carries no intrinsic-function semantics).
Emulator: DescribeStacks answers 200 and the stack carries an Outputs list, but the Output entries have NO OutputValue key at all, so a client that reads `o["OutputValue"]` fails with `KeyError: 'OutputValue'` (the replay's exact failure, in `outputs = {o["OutputKey"]: o["OutputValue"] for o in described.get("Outputs", [])}`).
Why: a client cannot read any output of the stack — not just the unresolvable one. Every SDK that models Output.OutputValue as populated (and every `aws cloudformation describe-stacks --query 'Stacks[0].Outputs'` consumer, and CDK/CLI output plumbing) either throws or silently reads `None`, so a deployment that reports CREATE_COMPLETE hands back no values at all.
Fix: the DescribeStacks output projection and the template Outputs evaluator — resolve Ref AWS::NotificationARNs (and Fn::Join over it) against the stack's recorded NotificationARNs and always serialize OutputValue, using the empty string for an output that resolves to nothing.
Traces: cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event.

=== Cause 2 of 6: an AWS::CloudFormation::Stack (nested stack) resource mints a physical id that is not a stack, so the child stack does not exist ===
AWS: a nested AWS::CloudFormation::Stack resource creates a REAL child stack whose physical resource id is that child's StackId; DescribeStacks and DescribeStackResources on that id answer with the child's own status and resources, and an update that touches only one sibling nested stack updates both children in place. Smithy: DescribeStackResources declares no modeled operation errors; AWS refuses an unresolvable stack with awsQueryError code `ValidationError` at HTTP 400 and the message `Stack with id <stack-name-or-id> does not exist` — the sentence is fixed text and the id is taken verbatim from the request.
Emulator: the nested-stack resource reports a physical id of the shape `<parent>-<LogicalId><LogicalId>Resource<HEX>-<HEX>`, but no stack exists under it: DescribeStackResources on that id is refused `ValidationError` / `Stack with id <that id> does not exist`. The parent reaches CREATE_COMPLETE regardless, so the failure is invisible from the parent.
Why: every nested-stack template — which is what CDK emits for any non-trivial app, and what SAM and most multi-stack IaC produce — deploys green while the child stacks it claims to own do not exist. A client cannot describe, update or drift-detect a child, and resources declared inside a child are never provisioned, so cross-child wiring (here a topic in one child delivering to a queue subscribed in a sibling) is silently inert.
Fix: the CloudFormation provisioner for AWS::CloudFormation::Stack — create a real child stack (own StackId, own events, own resources) from the nested template, record that StackId as the resource's physical id, and route a parent update to an in-place update of each child.
Traces: cloudformation__sns__sqs.cdk-sibling-nested-stacks-deliver-topic-to-queue-across-a-producer-only-update.

=== Cause 3 of 6: NotificationARNs carried by a CHANGE SET publish no stack event, so the cleared-then-restored transition is unobservable ===
AWS: Smithy declares CreateChangeSetInput$NotificationARNs (list of NotificationARN, length 0-5), and DescribeChangeSet/DescribeStacks echo it. Executing a change set publishes one SNS notification for EVERY StackEvent of that execution to each listed topic, as a body of `Key='value'` lines (StackId, Timestamp, EventId, LogicalResourceId, Namespace, PhysicalResourceId, ResourceProperties, ResourceStatus, ResourceStatusReason, ResourceType, StackName, ClientRequestToken). A change set created with an explicit EMPTY NotificationARNs list clears the stack's topics and stops the stream; a later change set that names the topic again resumes it. Recorded behavior only for the exact line set (the model carries no payload schema).
Emulator: the subscribed SQS queue receives nothing for any event of the executed change set — the check fails with `AssertionError: events never delivered: [<ten EventIds>]` after draining for its whole 120 s window — so neither the baseline stream nor the clear/restore transition can be observed.
Why: a client that gates a change-set-driven deployment on the SNS event stream (approval gates, ChatOps, rollback alarms) sees total silence and concludes the execution never changed state; and because the baseline stream is absent, clearing NotificationARNs looks identical to a working configuration.
Fix: the ExecuteChangeSet path's stack-event emission — fan every recorded stack event out to the stack's current NotificationARNs — plus the stack-state store that must take the change set's NotificationARNs (including an explicit empty list that clears them). The open issue 20261029T183635Z-cloudformation-sns-sqs-a-stack-s-notificationarns-never.md describes the same silence for the CreateStack/UpdateStack/DeleteStack path and should be closed together with this cause, but it does not carry this check's id or the change-set path.
Traces: cloudformation__sns__sqs.change-set-empty-notification-arns-stops-delivery-until-restored.

=== Cause 4 of 6: Fn::GetAtt on a key absent from a custom resource provider's response Data does not fail the consuming resource ===
AWS: the custom-resource protocol is not in the Smithy model (recorded behavior only). When a provider answers SUCCESS with a `Data` map, `Fn::GetAtt [<Logical>, <Key>]` resolves from that map for every key the provider returned, and a GetAtt naming a key the Data LACKS fails the consuming resource with CloudFormation's attribute error naming the resource and the attribute, which rolls the stack back; the sibling stack whose GetAtt names a key the provider DID return completes and the resolved value is delivered in the consumer's own provider request.
Emulator: unreachable — no provider request is ever published to the SNS topic named by ServiceToken, so the check blocks on the first `Create` request for the Provider resource and is killed at the harness ceiling (`Check timed out after 60.0s`). Neither the positive GetAtt resolution nor the missing-key failure is implemented.
Why: a template that consumes a provider's outputs through GetAtt either hangs forever or, once the provider path exists, silently resolves an attribute the provider never returned — so a typo in an attribute name ships as a stack that deploys green with an empty value instead of failing at create time.
Fix: the custom-resource provisioner's attribute resolution — bind the provider response's `Data` map to the resource's attributes, resolve Fn::GetAtt from it, and fail the consuming resource with the attribute error when the key is absent. The precondition (publishing the provider request at all) is the subject of the open issue 20261101T212352Z-cloudformation-sns-sqs-a-custom-resource-whose-servicetoken.md, which does not carry this check's id.
Traces: cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails.

=== Cause 5 of 6: the 4,096-byte limit on a custom-resource provider response is not enforced ===
AWS: recorded behavior only (no Smithy coverage). A provider response document of exactly 4,096 bytes completes its custom resource (CREATE_COMPLETE); one of 4,097 bytes is rejected, and the dependent resource gets a CREATE_FAILED event whose ResourceStatusReason is CloudFormation's response-too-long sentence. The stack then rolls back, deleting the over-long resource under the PhysicalResourceId CloudFormation itself assigned before deleting the resource it depended on.
Emulator: unreachable and unimplemented — the check publishes nothing to the provider topic, blocks on the `Fits` Create request and is killed at `Check timed out after 60.0s`, so neither the accepted boundary size nor the refusal of the byte over it exists.
Why: a provider that returns slightly too much data fails in the real account with a diagnosable reason and succeeds here, so an over-long response ships undetected and the rollback ordering a client depends on to release provider-side resources never runs.
Fix: the custom-resource response handler — measure the serialized response body, accept it up to 4,096 bytes, and on an over-long body fail the resource with the response-too-long reason and drive the rollback (deleting under CloudFormation's own physical id, dependents first).
Traces: cloudformation__sns__sqs.custom-resource-response-over-4096-bytes-fails.

=== Cause 6 of 6: a custom resource's ServiceTimeout outside 1-14400 is not refused before the provider is called ===
AWS: documented for AWS::CloudFormation::CustomResource — `ServiceTimeout` must be an integer between 1 and 14400 seconds. A value of `0` or `14401` fails the resource BEFORE any request is published to the provider, with the CREATE_FAILED ResourceStatusReason exactly `ServiceTimeout must be an integer between 1 and 14400 seconds` (entirely fixed text, no values interpolated), and the stack ends ROLLBACK_COMPLETE; the boundary value `14400` is accepted and the request IS sent, completing when the provider answers. Recorded behavior for the stack statuses; the bound is documented.
Emulator: unreachable and unimplemented — the control stack with ServiceTimeout `14400` never produces a provider request, so the check is killed at `Check timed out after 60.0s`; nothing validates the ServiceTimeout range, and nothing guarantees that an out-of-range value reaches no provider.
Why: an out-of-range ServiceTimeout is accepted, so a template error surfaces as a stack that waits forever instead of a CREATE_FAILED with the sentence that names the rule; and a provider can be invoked for a resource AWS would never have dispatched, making the provider do work for a resource that cannot exist.
Fix: the custom-resource provisioner's property validation — range-check ServiceTimeout before dispatch, emit the CREATE_FAILED reason above verbatim, roll the stack back, and publish no provider request for the refused resource.
Traces: cloudformation__sns__sqs.custom-resource-service-timeout-out-of-range-fails-the-resource.

## Evidence
Cause 1 — cloudformation__sns__sqs.cdk-deploy-notification-arns-deliver-every-stack-event [error]:
  File "check_...cdk_deploy_notification_arns_deliver_every_stack_event.py", line 66, in run
    outputs = {o["OutputKey"]: o["OutputValue"] for o in described.get("Outputs", [])}
  KeyError: 'OutputValue'
  expected: DescribeStacks returns Outputs == [{OutputKey: "NotificationArns", OutputValue: "<the stack's notification topic ARN>"}] and NotificationARNs == [<that topic>].
  actual: an Outputs entry with no OutputValue member.

Cause 2 — cloudformation__sns__sqs.cdk-sibling-nested-stacks-deliver-topic-to-queue-across-a-producer-only-update [error]:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the DescribeStackResources operation: Stack with id sim2real-cdk-sib-<hex>-ProducerNestedStackProducerNestedStackResource<HEX>-<HEX> does not exist
  expected: that id names a real child stack whose resources DescribeStackResources returns.
  actual: the parent is CREATE_COMPLETE, the nested-stack physical id resolves to nothing.

Cause 3 — cloudformation__sns__sqs.change-set-empty-notification-arns-stops-delivery-until-restored [mismatch]:
  AssertionError: events never delivered: ['0274a32d-…', '53f4761e-…', '58e74db0-…', '610a556c-…', '745d6b24-…', '84e31daf-…', '962f7c5e-…', 'a7f92812-…', 'b94257d9-…', 'd618e932-…']
  expected: one Key='value' notification per StackEvent of the executed change set on the subscribed queue.
  actual: the queue stays empty for every event id DescribeStackEvents reports.

Causes 4, 5 and 6 — all three custom-resource checks report "Error: Check timed out after 60.0s" while blocked on the first provider request (`box.get(... RequestType == "Create")`), because nothing is published to the SNS ServiceToken topic. Expected, and never reached:
  - GetAtt on a key the Data lacks → the consuming resource CREATE_FAILED with the attribute error; GetAtt on a returned key → the value delivered to the consumer's request.
  - a 4,096-byte response → CREATE_COMPLETE; a 4,097-byte response → CREATE_FAILED with the response-too-long reason, then rollback deleting the over-long resource under CloudFormation's own physical id before its dependency.
  - ServiceTimeout "0" / "14401" → ROLLBACK_COMPLETE with the single CREATE_FAILED reason "ServiceTimeout must be an integer between 1 and 14400 seconds" and NO request reaching the provider, while "14400" is dispatched and completes.

## Steps to Reproduce
Boot the emulator and replay the six check ids in trace_ids from suite simparity-behavioral-aws-v1.

By hand:
1. Cause 1/3: sns.CreateTopic; sqs.CreateQueue; set a queue policy allowing Principal {"Service":"sns.amazonaws.com"} sqs:SendMessage conditioned on aws:SourceArn = the topic; sns.Subscribe(Protocol="sqs", Endpoint=<queue arn>). Then (1) create a stack with --notification-arns <topic> whose template declares Outputs {"NotificationArns": {"Value": {"Fn::Join": [",", {"Ref": "AWS::NotificationARNs"}]}}} and call DescribeStacks — the Outputs entry has no OutputValue; and (3) cloudformation.CreateChangeSet(ChangeSetType="CREATE", NotificationARNs=[<topic>]) + ExecuteChangeSet, then DescribeStackEvents and drain the queue — no event is delivered. Repeat with a change set carrying NotificationARNs=[] and then one naming the topic again.
2. Cause 2: deploy a template whose root declares two AWS::CloudFormation::Stack resources, read the nested resources' PhysicalResourceId from DescribeStackResources on the parent, and call DescribeStackResources(StackName=<that id>) — ValidationError "Stack with id <id> does not exist".
3. Causes 4-6: cloudformation.CreateStack with a Custom::* resource whose ServiceToken is the topic ARN (variants: a consumer with Fn::GetAtt ["Provider", "Missing"]; two Custom:: resources answered with 4,096- and 4,097-byte response documents; ServiceTimeout "0", "14401" and "14400") and read the queue — no provider request is ever published, and for ServiceTimeout "0"/"14401" no CREATE_FAILED event with the range sentence is recorded.
