Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason|pull#17|Timeout|for msg in resp.get("Messages", []):
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old|pull#12|Timeout|for msg in resp.get("Messages", []):
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data|pull#14|Timeout|for msg in resp.get("Messages", []):
Issue-Commit: 446e5d40b4aa72232e72e50ee85b856a1859ebd6

# cloudformation__sns__sqs: a Custom:: resource whose ServiceToken is an SNS topic is never invoked, so provider SUCCESS/FAILED verdicts, GetAtt Data, replacement cleanup Deletes and the rollback reason are all unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `446e5d40b4aa72232e72e50ee85b856a1859ebd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause of service `cloudformation__sns__sqs`, grounded in the three failing behavioral checks in trace_ids. None of the three was marked ALREADY FILED or OWNED by the replay report. The open issue 20261025T180059Z-cloudformation-custom-resource-providers-and-hook-configuration.md describes the same provisioner gap and lists two of these three check ids, but did not own them in this replay and does not carry the FAILED-verdict/rollback-reason check at all; it is named here so a fixer can close both together.

=== Cause 1 of 1: CloudFormation publishes no custom-resource lifecycle request to the SNS topic named by ServiceToken ===
AWS: the custom-resource protocol is not in the Smithy model (recorded behavior only). For a resource of type `Custom::<Name>` / `AWS::CloudFormation::CustomResource`, CloudFormation publishes a provider request to the SNS topic ARN in `ServiceToken` for every Create/Update/Delete that touches the resource, and holds the resource in `*_IN_PROGRESS` until the provider PUTs a response document to the request's presigned `ResponseURL`. The request carries `RequestType`, `RequestId`, `StackId`, `LogicalResourceId`, `ResponseURL`, `ResourceProperties`, plus `PhysicalResourceId` and `OldResourceProperties` on Update/Delete. The response's `Status` decides the outcome:
  * `SUCCESS` -> the resource reaches CREATE_COMPLETE/UPDATE_COMPLETE and `Fn::GetAtt <Logical>.<Key>` resolves from the response's `Data` map, surfacing in stack Outputs.
  * `FAILED` with a `Reason` -> the resource gets a CREATE_FAILED event whose `ResourceStatusReason` is `Received response status [FAILED] from custom resource. Message returned: <Reason> (RequestId: <RequestId>)` and whose `PhysicalResourceId` is the id the provider returned with the FAILED response; the stack then goes ROLLBACK_IN_PROGRESS with stack-level `ResourceStatusReason` `The following resource(s) failed to create: [<LogicalId>]. Rollback requested by user.`, a `Delete` request for that same PhysicalResourceId is published, and after it is answered the stack is ROLLBACK_COMPLETE with no resolved Outputs and a Thing DELETE_COMPLETE event. In each message above the bracketed/quoted text is fixed and the Reason, RequestId, LogicalResourceId and PhysicalResourceId are values taken from the request and the provider's response.
  * an Update answered with a NEW `PhysicalResourceId` is a replacement: the stack goes UPDATE_COMPLETE_CLEANUP_IN_PROGRESS and a cleanup `Delete` for the OLD id with the OLD properties is published; the same id means an in-place update and no Delete.
Emulator: nothing is ever published to the ServiceToken topic. All three checks wire an SQS queue to an SNS topic (with a queue policy allowing sns.amazonaws.com for that topic ARN), create the stack with the topic as `ServiceToken`, and then block reading the queue for the first `Create` request. No request arrives, so each check makes no further progress and is killed at the harness ceiling: `Check timed out after 60.0s`. None of the downstream rules — the SUCCESS/Data/GetAtt round trip, the FAILED reason template, the rollback Delete for the provider's id, or the replacement cleanup Delete — is ever reached.
Why: custom resources are unusable end to end over SNS. A client deploying any template with a `Custom::*` resource (and every CDK/SAM construct that is implemented as one) gets a stack that never settles rather than a provisioned resource, gets no diagnosable reason when its provider fails, and never sees the cleanup Delete that lets it release what the provider created — so provider-side resources leak silently.
Fix: the CloudFormation provisioning path needs a provisioner arm for `Custom::*` / `AWS::CloudFormation::CustomResource` that (1) publishes the provider request to the `ServiceToken` (SNS Publish for a topic ARN), (2) serves a presigned `ResponseURL` in process and holds the resource at a stabilisation barrier until the response lands, carrying the provider's `PhysicalResourceId` and `Data` back so `Fn::GetAtt` and stack Outputs resolve from `Data`, (3) maps a `FAILED` response onto the CREATE_FAILED event text and reason above and drives the stack rollback, publishing the Delete for the id the provider returned, and (4) implements the replacement rule on a changed `PhysicalResourceId` (UPDATE_COMPLETE_CLEANUP_IN_PROGRESS plus the cleanup Delete of the old id with the old properties).
Traces: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason, cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old, cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-request-roundtrip-and-getatt-returns-response-data.

## Evidence
All three checks [mismatch], identical shape:
  expected: after CreateStack over {"Resources":{"Thing":{"Type":"Custom::Verdict","Properties":{"ServiceToken":"<SNS topic ARN>","Marker":"<unique>"}}},"Outputs":{"Greeting":{"Value":{"Fn::GetAtt":["Thing","Greeting"]}}}}, a Create provider request (RequestType="Create", with StackId, LogicalResourceId, RequestId, ResponseURL, ResourceProperties) is published to the topic and arrives on the subscribed queue within the check's 150s request window.
  actual: no message is ever delivered to the queue; each check spends its whole window in ReceiveMessage and the harness reports `Check timed out after 60.0s`.

Never reached, and therefore also unverified by this replay (same cause):
  - control stack: CREATE_COMPLETE with Outputs == {"Greeting": <value from the response Data>}
  - subject stack: Thing CREATE_FAILED with ResourceStatusReason "Received response status [FAILED] from custom resource. Message returned: <Reason> (RequestId: <RequestId>)" and PhysicalResourceId == the id returned with the FAILED response
  - stack ROLLBACK_IN_PROGRESS with ResourceStatusReason "The following resource(s) failed to create: [Thing]. Rollback requested by user.", then ROLLBACK_COMPLETE with no Outputs and a Thing DELETE_COMPLETE event
  - an Update answered with a new PhysicalResourceId producing a cleanup Delete for the old id

## Steps to Reproduce
Boot the emulator and replay the three checks in trace_ids from suite simparity-behavioral-aws-v1.

By hand:
1. sns.CreateTopic; sqs.CreateQueue; read the queue's QueueArn; sqs.SetQueueAttributes with a Policy allowing Principal {"Service":"sns.amazonaws.com"} sqs:SendMessage on that ARN with Condition ArnEquals aws:SourceArn = the topic ARN; sns.Subscribe(Protocol="sqs", Endpoint=<queue ARN>).
2. cloudformation.CreateStack with TemplateBody {"Resources":{"Thing":{"Type":"Custom::Verdict","Properties":{"ServiceToken":"<topic ARN>","Marker":"m-1"}}},"Outputs":{"Greeting":{"Value":{"Fn::GetAtt":["Thing","Greeting"]}}}}.
3. sqs.ReceiveMessage on the queue — against AWS an SNS envelope whose Message is the provider request arrives; against the emulator nothing arrives and the stack never settles behind the provider.
4. To exercise the rest once step 3 works: PUT {"Status":"SUCCESS","PhysicalResourceId":"<pid>","StackId":...,"RequestId":...,"LogicalResourceId":"Thing","Data":{"Greeting":"hello"}} to the request's ResponseURL and check Outputs; repeat with {"Status":"FAILED","Reason":"<reason>"} and check the CREATE_FAILED reason text, the rollback Delete for that PhysicalResourceId, and ROLLBACK_COMPLETE with no Outputs.
