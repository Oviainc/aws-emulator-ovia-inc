Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__events__sqs.xsvc-apigateway-1-near-miss-detail-type-is-not-delivered-to-rest-stage
Issue-Key: apigateway__events__sqs.xsvc-apigateway-1-near-miss-detail-type-is-not-delivered-to-rest-stage|_poll_signed_post#7|Timeout|time.sleep(2)
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# apigateway__events__sqs: an EventBridge rule targeting a REST stage never delivers, so the case-sensitive detail-type near-miss contract is ungradeable

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause.

Cause 1 of 1: an EventBridge rule whose target is an API Gateway REST stage (`arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/POST/<resource>/*`) never produces a request at the stage, so no event ever reaches the SQS sink behind the stage and the whole near-miss differential cannot be observed.
Failing check: apigateway__events__sqs.xsvc-apigateway-1-near-miss-detail-type-is-not-delivered-to-rest-stage

AWS: EventBridge matches `detail-type` case-sensitively. For a rule with pattern {"source":[S],"detail-type":["OrderPlaced"]} and an execute-api target carrying a RoleArn granting `execute-api:Invoke` plus HttpParameters.PathParameterValues, a PutEvents entry with DetailType "OrderPlaced" is signed and POSTed to the stage resource, while an entry in the SAME PutEvents batch whose DetailType differs only in letter case ("orderPlaced") is NOT delivered; republishing with the exact detail-type does deliver. Smithy (eventbridge): PutEvents returns FailedEntryCount/Entries with an EventId per accepted entry, and PutTargets.Targets[].HttpParameters is the modeled carrier of PathParameterValues/HeaderParameters/QueryStringParameters for API-destination and execute-api targets; the delivery semantics themselves are recorded behavior only.
Emulator: the check never gets past its own positive control: the signed POST warm-up plus sink collection, then the matched-entry delivery, produce nothing within the harness budget and the check ends `Check timed out after 60.0s`. No request arrives at the REST stage for either the matching entry or the control published after it, so the negative (near-miss withheld) is indistinguishable from "nothing is ever delivered".
Why: a client that wires an EventBridge rule to a REST stage sees its API never invoked. Case-sensitivity of detail-type — the thing that decides whether a near-miss event hits a production endpoint — cannot be relied on at all, and a negative-delivery test passes vacuously on the emulator while failing on AWS.
Fix: the EventBridge target-delivery path for execute-api target ARNs — target resolution/dispatch in the events crate (recognising an `arn:aws:execute-api:...` target, assuming the target RoleArn, rendering HttpParameters into the request path, and issuing the signed request to the API Gateway stage), together with the rule matcher's case-sensitive detail-type comparison. Open issues 20261029T064745Z-apigateway-events-sqs-an-eventbridge-rule-targeting-a.md and 20261023T210711Z-apigateway-events-sqs-all-four-eventbridge-to-rest.md own the sibling checks in this group; neither carries this check's replay key, so this cause is filed separately and should be closed together with them.
Traces: apigateway__events__sqs.xsvc-apigateway-1-near-miss-detail-type-is-not-delivered-to-rest-stage.

## Evidence
Expected: the matching PutEvents entry (DetailType "OrderPlaced") is POSTed to the REST stage and its marker appears on the SQS sink; the near-miss entry (DetailType "orderPlaced", same batch) produces no request; a control published afterwards is delivered; republishing the near-miss marker with the exact detail-type delivers it.
Actual: `Check timed out after 60.0s` — nothing is delivered on the positive path either (the warm-up signed POST + sink collect and then the matched delivery never converge inside the budget), so no verdict about the near-miss can be formed.

## Steps to Reproduce
1. Create an SQS queue and a REST API whose POST /orders/{p} integration writes the request body to that queue; deploy a stage and confirm a signed POST to the stage lands a message (positive control).
2. Create a custom event bus and a rule with EventPattern {"source":["sim2real.orders"],"detail-type":["OrderPlaced"]}.
3. PutTargets with Arn=arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/POST/orders/*, a RoleArn trusting events.amazonaws.com and allowing execute-api:Invoke on that ARN, and HttpParameters.PathParameterValues=["$.detail.id"].
4. PutEvents one entry with DetailType="OrderPlaced" and one with DetailType="orderPlaced", same Source and bus.
5. Poll the queue: on AWS only the exact-case entry arrives. On the emulator neither arrives and the check times out.
