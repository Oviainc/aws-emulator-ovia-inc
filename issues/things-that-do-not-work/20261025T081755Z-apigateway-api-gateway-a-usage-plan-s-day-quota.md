Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-usage-plan-daily-quota-exhaustion-rejects-over-http
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-usage-plan-daily-quota-exhaustion-rejects-over-http|run#31|AssertionError|assert rejection is not None, (
Issue-Commit: ffece56e68e804f0450b8a83661b5d243139afbd

# API Gateway: a usage plan's DAY quota is never enforced at the deployed stage — an exhausted key keeps getting 200 (1 cause)

> _Note: this issue was identified against the code base at commit `ffece56e68e804f0450b8a83661b5d243139afbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUsagePlan

## Description
Cause 1 of 1: usage-plan quota accounting exists only as bookkeeping; the execute-api data plane never refuses a request whose key has consumed its quota.

AWS: a usage plan carries QuotaSettings (Smithy: com.amazonaws.apigateway#QuotaSettings with members limit, offset and period, whose documented valid values are "DAY", "WEEK" and "MONTH" — "The target maximum number of requests that can be made in a given time period"). The quota is counted server-side per API key per period, and once the limit is consumed the deployed stage REFUSES further requests carrying that key: recorded behavior is HTTP 429 with x-amzn-ErrorType: LimitExceededException and body message "Limit Exceeded" (message quoted as an example of the recorded text). That refusal is distinct from the throttle 429 ("Too Many Requests") and from the 403 a key outside the plan gets; the check isolates the quota case by setting a rateLimit orders of magnitude above the request rate.

Emulator: with a DAY quota of 2 on the plan, the stage admitted every request and refused none — the recorded status sequence over 8 requests was [200, 200, 200, 200, 200, 200, 200, 200] and no rejection was ever produced.

Why: a client that relies on quota enforcement to see its own exhaustion (SaaS tiering, a test that asserts a key is cut off after N calls, any code branching on 429/LimitExceededException vs 429/Too Many Requests) never sees the refusal here. Quota is a correctness boundary, not a performance hint: code tested against this emulator will ship believing a key is capped when it is not.

Fix: usage-plan quota accounting and enforcement on the execute-api request path — the per-key/per-period counter in the usage-plan state plus the refusal branch in the stage's request-authorization step that must map an exhausted counter to 429 LimitExceededException before the integration is invoked. Enforcement must be keyed per API key and per period, not per connection.

Traces: apigateway.apigw-usage-plan-daily-quota-exhaustion-rejects-over-http.

## Evidence
Check apigateway.apigw-usage-plan-daily-quota-exhaustion-rejects-over-http.
Setup: REST API with a MOCK-integrated GET /q, apiKeyRequired=true, deployed to stage 'test', fresh API key attached to a usage plan whose quota is {limit: 2, period: DAY} and whose rateLimit is far above the request rate.
Expected (AWS): the first 2 requests succeed, subsequent requests with the same key are refused with HTTP 429, x-amzn-ErrorType: LimitExceededException (recorded body message example: "Limit Exceeded").
Actual (emulator): AssertionError: the edge admitted 8 requests and never enforced the DAY quota of 2; status sequence [200, 200, 200, 200, 200, 200, 200, 200].

## Steps to Reproduce
1. CreateRestApi; create child resource 'q'; PutMethod GET with authorizationType=NONE and apiKeyRequired=true; PutIntegration type=MOCK with requestTemplates {"application/json": "{\"statusCode\":200}"}; PutMethodResponse/PutIntegrationResponse 200.
2. CreateDeployment and CreateStage stageName=test.
3. CreateApiKey (fresh value), CreateUsagePlan with quota {limit:2, period:DAY} and a high throttle rateLimit, apiStages = [that api/stage], then CreateUsagePlanKey binding the key.
4. Issue 8 sequential GET requests to the stage invoke URL for /q with header x-api-key: <key value>.
5. Observe: all 8 answer 200. AWS refuses from the 3rd onward with 429 / x-amzn-ErrorType: LimitExceededException.
