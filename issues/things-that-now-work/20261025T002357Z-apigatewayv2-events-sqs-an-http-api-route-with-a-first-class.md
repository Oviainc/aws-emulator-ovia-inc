Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing
Issue-Key: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue|_poll_http#27|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing|_poll_http#26|Timeout|time.sleep(interval_s)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# An HTTP API route with a first-class EventBridge PutEvents integration never puts an event, so neither the custom-bus routing nor the permission-denied negative is observable (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1 — an apigatewayv2 HTTP API route whose integration is the first-class EventBridge-PutEvents subtype does not issue PutEvents when the route is invoked.
AWS: an HTTP API route with `IntegrationType: AWS_PROXY` and `IntegrationSubtype: EventBridge-PutEvents`, a CredentialsArn naming a role that holds events:PutEvents, and RequestParameters mapping Source/DetailType/Detail/EventBusName from the request, makes API Gateway call EventBridge PutEvents on every matching request. The event then lands on the named custom bus, where a rule on it delivers to its SQS target; a near-miss rule differing only in letter case must receive nothing, because EventBridge string matching is exact and case-sensitive. If the CredentialsArn role does NOT hold events:PutEvents, the integration's PutEvents is denied and nothing is delivered at all.
Emulator: no event ever reaches the bus. Both checks complete CreateApi/CreateIntegration/CreateRoute/CreateStage, CreateEventBus, PutRule and PutTargets (FailedEntryCount 0) and the real HTTP POST to the stage, then poll the rule's queue; the message never arrives and the replay cuts each check off at 60 s. Because the positive leg never fires, the permission-denied negative cannot be graded either: "denied" and "integration never invoked" look identical.
Why: the subtype integration exists precisely so a client can publish to EventBridge with no Lambda in between. Every control-plane call and the HTTP request itself report success while no event is produced, so the client loses every request and has no failure signal; and it cannot tell a missing IAM grant from a broken integration.
Fix: the apigatewayv2 integration executor for IntegrationSubtype `EventBridge-PutEvents` — the RequestParameters mapping, the CredentialsArn assumption and the PutEvents call it must issue — in services/service-com-amazonaws-apigatewayv2-apigatewayv2, together with its authorization of that call.
Traces: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue, apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing.

## Evidence
Both checks report "Check timed out after 60.0s". Prior calls returned 2xx: CreateApi, CreateIntegration (IntegrationType AWS_PROXY, IntegrationSubtype EventBridge-PutEvents, CredentialsArn set), CreateRoute, CreateStage/auto-deploy, CreateEventBus, CreateQueue, SetQueueAttributes, PutRule and PutTargets (FailedEntryCount 0); the HTTP POST to the stage URL also completes. The terminal read — ReceiveMessage on the queue behind the custom-bus rule — never returns a message, and the near-miss queue cannot be shown empty for the right reason. Expected: one message on the matching rule's queue carrying the mapped Source/DetailType/Detail, none on the case-differing near-miss rule's queue, and nothing at all when the integration's role lacks events:PutEvents.

## Steps to Reproduce
1. CreateEventBus(Name=b); CreateQueue(q); SetQueueAttributes(q, Policy admitting the rule ARN); PutRule(Name=r, EventBusName=b, EventPattern='{"source":["app.orders"],"detail-type":["OrderPlaced"]}'); PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<q arn>}]); plus a near-miss rule with detail-type "orderplaced" and its own queue.
2. CreateApi(ProtocolType='HTTP'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationSubtype='EventBridge-PutEvents', CredentialsArn=<role with events:PutEvents>, RequestParameters={'Source':'app.orders','DetailType':'OrderPlaced','Detail':'$request.body','EventBusName':b}); CreateRoute to that integration; deploy a stage.
3. POST the route -> the matching queue never receives a message.
4. Repeat with a CredentialsArn role that lacks events:PutEvents -> indistinguishable from step 3.
