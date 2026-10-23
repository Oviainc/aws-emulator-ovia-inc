Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.deploy-topic-rule-cfn-routes-message iot__lambda.cfn-deploy-iot-rule-error-action-lambda-positive iot__lambda.cfn-deploy-iot-rule-lambda-env-update-positive iot__lambda.cfn-deploy-iot-rule-missing-lambda-permission-negative iot.shadow-client-token-echoed-not-persisted iot.shadow-oversize-document-rejected iot.topic-rule-with-unconfirmed-destination-blocks-delivery
Issue-Key: sqs|ReceiveMessage|AWS.SimpleQueueService.NonExistentQueue
Issue-Key: iot.shadow-client-token-echoed-not-persisted|run#9|AssertionError|assert accepted.get("clientToken") == token, (
Issue-Key: iot.shadow-oversize-document-rejected|run#13|AssertionError|assert code_message == "Json document too large. Size is " + str(len(json.dumps({"blob": "y" * _OVERSIZE_BYTES}, separators=(",", ":")))) + ", max allowed: 8192
Issue-Key: iot|CreateIntegration|NotImplemented
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# IoT: 4 divergences — CFN stacks reach CREATE_COMPLETE without creating the AWS::SQS::Queue they declare, UpdateThingShadow drops clientToken, shadow size cap is 64 KiB not 8 KiB, apigatewayv2 CreateIntegration is an unimplemented stub

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack (AWS::SQS::Queue provisioning) / UpdateThingShadow / CreateIntegration

## Description
Four independent root causes, all grounded in failing checks whose check-id service token is `iot`. NOTE: two of them are served by crates other than the iot control plane (the iot-data shadow handler and the apigatewayv2 handler) — the service token here is the one the failing check reports, not the crate to patch; each cause names its own component under Fix.

=== Cause 1 of 4: a CloudFormation stack reports CREATE_COMPLETE but the AWS::SQS::Queue resource it declares does not exist in SQS ===
Failing checks: iot.deploy-topic-rule-cfn-routes-message, iot__lambda.cfn-deploy-iot-rule-error-action-lambda-positive, iot__lambda.cfn-deploy-iot-rule-lambda-env-update-positive, iot__lambda.cfn-deploy-iot-rule-missing-lambda-permission-negative.
AWS: recorded behavior only (CloudFormation resource provisioning is not described by the SQS or IoT Smithy models). When a stack containing `AWS::SQS::Queue` reaches CREATE_COMPLETE, the queue really exists: `{"Ref": "Queue"}` resolves to that queue's URL and SQS ReceiveMessage/SendMessage/DeleteMessage on that URL succeed (HTTP 200). A queue URL that names no queue is answered by SQS with HTTP 400 and error code `AWS.SimpleQueueService.NonExistentQueue` — which is exactly what the emulator returns for a URL its own stack just published, so the two halves of the emulator disagree about whether the resource exists.
Emulator: the create-stack waiter reaches CREATE_COMPLETE, DescribeStacks returns an `Outputs` entry whose value is a well-formed queue URL built from the stack/logical-id/suffix convention, and the very first SQS call against that URL is refused with `AWS.SimpleQueueService.NonExistentQueue`. Example from the replay: `The queue referred to by https://sqs.<region>.amazonaws.com/<account>/<stack>-Queue-<suffix> does not exist. Ensure that the QueueUrl is correct and that the queue has not been deleted.` The queue URL is therefore synthesized as a physical id without the corresponding queue being registered in SQS state.
Why: every client that deploys a stack and then uses the resources it created breaks. A caller cannot distinguish 'deployment succeeded' from 'nothing was built': CREATE_COMPLETE plus a Ref output is the contract AWS clients build on, and here it hands back a dangling handle. Any downstream assertion (a rule action that writes into the queue, a consumer draining it) then fails for a reason that looks like a delivery bug rather than a provisioning bug.
Fix: the CloudFormation stack-provisioning component — the resource provisioner for `AWS::SQS::Queue` must create the queue in SQS state (and register it so GetQueueAttributes / ReceiveMessage resolve it) before the stack transitions to CREATE_COMPLETE, and `Ref`/`Fn::GetAtt` must return the URL/ARN of that created queue. The same provisioner path should be audited for the other resource types these stacks declare (AWS::IAM::Role, AWS::Lambda::Function, AWS::IoT::TopicRule), since the same shortcut would explain why the three polling checks below never observe an effect.
Traces: iot.deploy-topic-rule-cfn-routes-message (template has Resources.Queue = AWS::SQS::Queue and Outputs.QueueUrl = {"Ref": "Queue"}); the three iot__lambda cfn-deploy checks fail identically on their FallbackQueue / ResultQueue / GoodQueue outputs. Related, NOT claimed as this cause: iot__lambda.cfn-deploy-iot-rule-lambda-stack-positive, iot__lambda.cfn-deploy-iot-rule-lambda-missing-permission-negative and iot__lambda.cfn-deploy-iot-rule-wrong-lambda-arn-negative time out at 60s inside a 90s CloudWatch-Logs marker poll, i.e. the stack's effect never materializes either; they are not listed in trace_ids because their only evidence is the timeout.

=== Cause 2 of 4: UpdateThingShadow does not echo clientToken in its reply ===
Failing check: iot.shadow-client-token-echoed-not-persisted.
AWS: recorded behavior only for the document body (Smithy: UpdateThingShadow input/output `payload` is an unstructured `JsonDocument` blob, so the model says nothing about its keys). Recorded: the 200 reply body for a request whose document carries `clientToken` is exactly `{"state","metadata","version","timestamp","clientToken"}` — the request's clientToken is echoed back, and the PERSISTED document (what GetThingShadow returns) does not contain clientToken but does contain the server-computed `state.delta`.
Emulator: the reply document has no `clientToken` member at all (`got None`), so the reply is indistinguishable from one for a request that never supplied a token.
Why: clientToken is the correlation key an MQTT/HTTP shadow client uses to match a reply to the request it issued; with it dropped, a client that multiplexes shadow updates cannot attribute the acknowledgement, and a client that waits for its own token's reply hangs.
Fix: the UpdateThingShadow handler's response serializer in the iot-data (iotdataplane) crate — carry the request document's `clientToken` through to the reply document while continuing to exclude it from the stored document.
Traces: iot.shadow-client-token-echoed-not-persisted.

=== Cause 3 of 4: the shadow-document size ceiling is 64 KiB instead of 8 KiB, and the 413 message is an emulator-internal sentence ===
Failing check: iot.shadow-oversize-document-rejected.
AWS: Smithy: UpdateThingShadow models `RequestEntityTooLargeException` with httpError 413. Recorded: the limit applied is 8192 bytes over the compact desired-section object, and the message is built from the request and the constraint — `Json document too large.  Size is <size>, max allowed: 8192` — so the rule that produces the message is known and the limit value in it is a requirement.
Emulator: the request is refused with the right code (RequestEntityTooLargeException) but the enforced ceiling is 65536 bytes and the message is `this emulator reads a state document of at most 65536 bytes, and this request would need 65572`. A document between 8 KiB and 64 KiB — which AWS refuses — is therefore ACCEPTED by the emulator, and the limit is applied to the whole request payload rather than to the sized state object.
Why: the 8 KB cap is the durability contract that stops a device shadow being used as a blob store. An emulator with an 8x larger ceiling lets a device write documents that production AWS will reject, so the failure only appears after deployment; and the message does not tell the caller what the real limit is.
Fix: the UpdateThingShadow handler's request validation in the iot-data (iotdataplane) crate — size the compact state/desired object, compare against 8192, and build the message from the measured size and that limit.
Traces: iot.shadow-oversize-document-rejected.

=== Cause 4 of 4: apigatewayv2 CreateIntegration is an unimplemented scaffold stub ===
Failing check: iot.topic-rule-with-unconfirmed-destination-blocks-delivery.
AWS: CreateIntegration is a modeled apigatewayv2 operation that creates an integration on an HTTP API and returns 201 with the integration's id; the check uses it to front a recorder Lambda with a publicly reachable HTTPS endpoint so an IoT topic-rule destination can be confirmed.
Emulator: the call is refused with error code `NotImplemented` and the message `operation CreateIntegration is scaffolded but not implemented in this emulator`, which aborts the check before any IoT behavior is exercised. No open issue covers this operation (searched: no existing issue mentions CreateIntegration).
Why: any caller building an HTTP API — and every IoT topic-rule-destination confirmation flow that needs a real HTTPS callback — cannot proceed at all; an unimplemented operation is a hard stop rather than a wrong answer.
Fix: implement the CreateIntegration handler in the apigatewayv2 crate (create the integration resource in that service's state and return the modeled CreateIntegrationResult), then re-replay the IoT check.
Traces: iot.topic-rule-with-unconfirmed-destination-blocks-delivery.

## Evidence
Cause 1 — expected: after `cfn.get_waiter('stack_create_complete')` returns, `sqs.receive_message(QueueUrl=<Outputs.QueueUrl>)` succeeds (200). Actual: HTTP 400 `AWS.SimpleQueueService.NonExistentQueue` — "The queue referred to by https://sqs.us-west-2.amazonaws.com/000000000000/sim2real-topicrule-stack-6950d1707c81-Queue-5C9485B486CF does not exist..." (same shape for the FallbackQueue/ResultQueue/GoodQueue outputs of the three iot__lambda stacks).
Cause 2 — expected reply keys {state, metadata, version, timestamp, clientToken} with clientToken == the request's token. Actual: AssertionError "UpdateThingShadow must echo the request clientToken, got None".
Cause 3 — expected: RequestEntityTooLargeException (413) with message "Json document too large.  Size is <N>, max allowed: 8192". Actual: RequestEntityTooLargeException with message 'this emulator reads a state document of at most 65536 bytes, and this request would need 65572' — i.e. the enforced ceiling is 65536, not 8192.
Cause 4 — expected: CreateIntegration returns 201 with an IntegrationId. Actual: ClientError (NotImplemented) "operation CreateIntegration is scaffolded but not implemented in this emulator".

## Steps to Reproduce
Cause 1: create a CloudFormation stack whose template contains `{"Resources": {"Queue": {"Type": "AWS::SQS::Queue"}}, "Outputs": {"QueueUrl": {"Value": {"Ref": "Queue"}}}}`; wait for CREATE_COMPLETE; read Outputs.QueueUrl from DescribeStacks; call sqs.ReceiveMessage on it -> AWS.SimpleQueueService.NonExistentQueue.
Cause 2: iot.CreateThing(thingName=T); iot-data.UpdateThingShadow(thingName=T, payload={"clientToken":"tok-1","state":{"desired":{"a":1}}}); inspect the reply document for clientToken.
Cause 3: iot-data.UpdateThingShadow(thingName=T, payload=json of {"state":{"desired":{"blob":"y"*65536}}}) -> compare the 413 message/limit; then repeat with a ~20 KB document, which AWS refuses and the emulator accepts.
Cause 4: apigatewayv2.CreateApi(ProtocolType="HTTP") then apigatewayv2.CreateIntegration(ApiId=..., IntegrationType="AWS_PROXY", IntegrationUri=<lambda arn>, PayloadFormatVersion="2.0") -> NotImplemented.
