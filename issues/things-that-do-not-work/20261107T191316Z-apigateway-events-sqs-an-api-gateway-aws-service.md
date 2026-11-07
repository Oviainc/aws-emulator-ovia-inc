Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__events__sqs.xsvc-apigateway-1-near-miss-detail-type-is-not-delivered-to-rest-stage apigateway__events__sqs.xsvc-apigateway-1-rule-delivers-transformed-detail-with-path-and-header-to-rest-stage apigateway__events__sqs.xsvc-apigateway-1-target-role-without-execute-api-invoke-is-denied
Issue-Key: apigateway__events__sqs.xsvc-apigateway-1-near-miss-detail-type-is-not-delivered-to-rest-stage|_poll_signed_post#7|AssertionError|raise AssertionError(f"POST {url} never returned {expected}; last={last!r}")
Issue-Key: apigateway__events__sqs.xsvc-apigateway-1-rule-delivers-transformed-detail-with-path-and-header-to-rest-stage|_poll_signed_post#7|AssertionError|raise AssertionError(f"POST {url} never returned {expected}; last={last!r}")
Issue-Key: apigateway__events__sqs.xsvc-apigateway-1-target-role-without-execute-api-invoke-is-denied|_poll_signed_post#8|AssertionError|raise AssertionError(f"POST {url} never returned {expected}; last={last!r}")
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigateway__events__sqs: an API Gateway AWS-service integration to SQS answers 500 Internal server error, so no EventBridge→REST-stage→SQS check can establish its precondition

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane, IntegrationType AWS → SQS SendMessage)

## Description
Cause 1 of 1: a REST method whose integration is `type=AWS` targeting SQS (`arn:aws:apigateway:<region>:sqs:path/<account>/<queue>`) with a `credentials` role never performs the SendMessage; the stage answers 500.

AWS: Smithy: PutIntegration.type -> IntegrationType enum includes `AWS`, documented as integrating a method "with any Amazon Web Services service endpoints"; `credentials` names the role API Gateway assumes, `requestParameters` may set `integration.request.header.Content-Type` to `'application/x-www-form-urlencoded'`, and `requestTemplates` renders the SQS query-protocol body. With a role granted sqs:SendMessage on the queue, API Gateway signs and issues SendMessage and serves the mapped integration response — 200 in these recordings. With a role lacking sqs:SendMessage, API Gateway surfaces the service's authorization error (an AccessDenied-class failure), not a bare gateway 500.

Emulator: every POST to the deployed stage is answered HTTP 500 with the DEFAULT_5XX gateway body `{"message":"Internal server error"}` for the whole poll window, on the granted-role path as well. The inner SQS action is never performed, so nothing is ever enqueued.

Why: a client that fronts SQS (the canonical API-Gateway service-proxy pattern) gets a blanket 500 for a correct configuration; and because the ungranted-role negative yields the same 500, an "authorization denied" result is indistinguishable from "the integration is not implemented". All three checks here fail on the direct-POST precondition, so the EventBridge rule → REST stage delivery, the detail-transformation path and the target-role denial are all unobservable.

Fix: the execute-api data-plane dispatcher for IntegrationType AWS — assume the `credentials` role, render the request template, sign and issue the inner SQS call, then map the service response (and surface the service's authorization error rather than DEFAULT_5XX). Related but NOT covering these checks: open issues 20261030T120000Z-apigateway-aws-service-integration-to-another-services-action-is-not-performed.md and 20261023T210711Z-apigateway-sqs-both-api-gateway-to-sqs-checks.md.

Traces: the three failing checks in trace_ids.

## Evidence
Configuration under test: `put_integration(type="AWS", integrationHttpMethod="POST", uri="arn:aws:apigateway:<region>:sqs:path/<account>/<queue>", credentials=<role arn>, requestParameters={"integration.request.header.Content-Type": "'application/x-www-form-urlencoded'"}, requestTemplates={"application/json": <SQS SendMessage form template>}, passthroughBehavior="NEVER")`.
Expected: HTTP 200 from the stage and the message visible in the queue.
Actual, all three checks: `AssertionError: POST https://<host>/_aws/execute-api/<apiId>/v1/orders/<id> never returned 200; last=(500, '{"message":"Internal server error"}')`.
The 500 status and the `{"message":"Internal server error"}` body are AWS's DEFAULT_5XX gateway response; the divergence is that they are served where AWS serves the mapped 200. Variable parts of the URL: host/port, api id, path id.

## Steps to Reproduce
1. CreateQueue; create an IAM role trusted by apigateway.amazonaws.com and granted sqs:SendMessage on that queue.
2. CreateRestApi; CreateResource /orders/{id}; PutMethod(POST, authorizationType=NONE, requestParameters={"method.request.path.id": true}).
3. PutIntegration(type="AWS", integrationHttpMethod="POST", uri="arn:aws:apigateway:<region>:sqs:path/<account>/<queue>", credentials=<role arn>, requestParameters={"integration.request.header.Content-Type": "'application/x-www-form-urlencoded'"}, requestTemplates={"application/json": "Action=SendMessage&MessageBody=..."}, passthroughBehavior="NEVER"); PutMethodResponse(200); PutIntegrationResponse(200, selectionPattern="").
4. CreateDeployment(stageName="v1"); POST the stage URL -> 500 `{"message":"Internal server error"}` instead of 200, and ReceiveMessage on the queue returns nothing.
