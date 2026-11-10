Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only apigateway.apigw-extended-request-id-is-minted-per-api-per-request-not-echoed apigateway.deploy-usage-plan-cfn-quota-enforced apigateway.flush-stage-authorizers-cache-bypasses-ttl apigateway.request-validator-body-validation-skipped-for-unmapped-content-type apigateway.delete-rest-api-racing-base-path-mapping-leaves-orphaned-mapping
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only|_poll_http#37|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-extended-request-id-is-minted-per-api-per-request-not-echoed|_poll_http#21|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.deploy-usage-plan-cfn-quota-enforced|_poll_usage_for_key#23|Timeout|time.sleep(2)
Issue-Key: apigateway.flush-stage-authorizers-cache-bypasses-ttl|_poll_for_fresh_invocation#24|Timeout|time.sleep(1)
Issue-Key: apigateway.request-validator-body-validation-skipped-for-unmapped-content-type|_poll_http#19|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.delete-rest-api-racing-base-path-mapping-leaves-orphaned-mapping|_assert_refusal#9|AssertionError|assert message == bad_request_message, f"unexpected BadRequestException message {message!r}"
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# apigateway: 2 net-new causes — five stage/data-plane behaviour scenarios reach no verdict inside the harness budget, and DeleteRestApi's base-path-mapping refusal states a sentence AWS never emits (the recorded AWS sentence is quoted here)

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRestApi

## Description
Two net-new root causes in the apigateway service, neither marked ALREADY FILED nor OWNED in this replay.

Cause 1 of 2: five REST-API stage/data-plane behaviour scenarios never reach a verdict — every one of them burns its whole run budget without the emulator ever producing the state the scenario polls for.

AWS: each of these five scenarios is a documented REST API behaviour that is observable within seconds of a CreateDeployment/CreateStage (plus, for two of them, a Lambda authorizer and a CloudFormation stack):
  (a) a stage cache is keyed ONLY by the method's cacheKeyParameters — a request that varies an UNDECLARED query parameter hits the existing entry, one that varies the DECLARED parameter misses, and FlushStageCache sends the next request to the integration (Smithy: apigateway FlushStageCache DELETE /restapis/{restApiId}/stages/{stageName}/cache/data, 202; https://docs.aws.amazon.com/apigateway/latest/developerguide/api-gateway-caching.html);
  (b) $context.extendedRequestId is minted fresh by each API on every request and equals that response's x-amz-apigw-id header, while $context.requestId carries the client-supplied value, so the two never coincide and the front and backend APIs of one request report different ids;
  (c) a usage plan's DAY quota is enforced at the invoke URL: the first N keyed requests are served and the next answers 429 with the "Limit Exceeded" body, and GetUsage answers for the plan over the date range (Smithy: GetUsage GET /usageplans/{usagePlanId}/usage, 200, output usagePlanId/startDate/endDate/values);
  (d) FlushStageAuthorizersCache forces the NEXT request on the stage to re-invoke the TOKEN authorizer immediately even though authorizerResultTtlInSeconds is far from expiring (Smithy: FlushStageAuthorizersCache DELETE /restapis/{restApiId}/stages/{stageName}/cache/authorizers, 202);
  (e) a requestModels entry registered only for application/json skips body validation for a request whose Content-Type matches no model (the schema-violating body reaches the integration) while the identical body sent as application/json is refused 400.
Recorded behavior only for the exact bodies and ids; the status/error facts above are the modelled/documented ones.

Emulator: all five checks end with `Error: Check timed out after 60.0s` — no HTTP status, no error code, no response at all. The scenarios are bounded pollers (they re-issue the stage request until it answers, with a ~150 s propagation budget), so a timeout means the emulator never reached the serving state the scenario needs: nothing it was asked for ever became observable.

Why: a client cannot observe stage caching, request-id minting, usage-plan quota enforcement, authorizer-cache flushing or request-validator content-type gating at all on this emulator — every one of those features is unverifiable, and code that waits for the documented effect waits forever instead of getting it (or getting a refusal it can handle). This is the same failure family as the already-filed data-plane issues (20261029T193654Z-apigateway-api-gateway-execute-api-data-plane-10-root.md and 20261109T152222Z-apigateway-stage-throttling-is-never-enforced-deleted-api.md), which do NOT carry these five replay keys.

Fix: the execute-api data-plane path for a deployed stage and the per-stage caches it owns — the stage cache key derivation and its flush, the per-request extendedRequestId/x-amz-apigw-id minting in the response serializer, the usage-plan quota metering on the keyed invoke path, the authorizer-result cache and its flush, and the request-validator content-type gate. In the apigateway service crate: the data-plane stage handler plus the FlushStageCache / FlushStageAuthorizersCache / GetUsage operation handlers and the stage state they read.

Traces: apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only (a), apigateway.apigw-extended-request-id-is-minted-per-api-per-request-not-echoed (b), apigateway.deploy-usage-plan-cfn-quota-enforced (c), apigateway.flush-stage-authorizers-cache-bypasses-ttl (d), apigateway.request-validator-body-validation-skipped-for-unmapped-content-type (e).

Cause 2 of 2: DeleteRestApi's refusal for an API a BasePathMapping still names states a sentence that is not AWS's — and THIS replay exposes AWS's sentence verbatim, which the open issue 20261105T160000Z-apigateway-base-path-mapping-guard-and-import-refusal-sentences.md (cause 1 of 3) explicitly could not read out of its own checks.

AWS: while a base path mapping of a custom domain name references a REST API, DeleteRestApi is refused with HTTP 400 BadRequestException (Smithy: DeleteRestApi DELETE /restapis/{restApiId}, success 202, errors BadRequestException 400 / ConflictException 409 / NotFoundException 404 / TooManyRequestsException 429 / UnauthorizedException 401) and the API survives. The message's FIXED text is `Deleting RestApi ` … ` failed. Please remove all base path mappings related to the RestApi in your domains: `; its VARIABLE parts are the restApiId from the request and the domain name(s) from state, i.e. `Deleting RestApi <restApiId> failed. Please remove all base path mappings related to the RestApi in your domains: <domainName>`. (A concurrent loser may instead answer TooManyRequestsException with the message `Too Many Requests` — that branch is accepted.)

Emulator: refuses with 400 BadRequestException, but the message is `Cannot delete REST API '<restApiId>', is referenced in base path mapping: <domainName>/<basePath>` — different fixed text, and it appends the basePath, which AWS's sentence does not carry (it names the DOMAIN, not domain/basePath).

Why: a client that matches on this refusal's text to tell the operator which record blocks the delete (or that parses the domain out of it to clean up) gets a sentence AWS never emits, and the "please remove all base path mappings … in your domains" instruction is lost.

Fix: the DeleteRestApi handler's referenced-by-base-path-mapping guard message rendering in the apigateway service crate (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_rest_api.rs and the shared base-path-mapping reference guard it calls). Only the message text changes; the guard, the 400 and the BadRequestException code already match.

Traces: apigateway.delete-rest-api-racing-base-path-mapping-leaves-orphaned-mapping.

## Evidence
Cause 1 of 2 (replay, apigateway):
- apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only — `Error: Check timed out after 60.0s`
- apigateway.apigw-extended-request-id-is-minted-per-api-per-request-not-echoed — `Error: Check timed out after 60.0s`
- apigateway.deploy-usage-plan-cfn-quota-enforced — `Error: Check timed out after 60.0s`
- apigateway.flush-stage-authorizers-cache-bypasses-ttl — `Error: Check timed out after 60.0s`
- apigateway.request-validator-body-validation-skipped-for-unmapped-content-type — `Error: Check timed out after 60.0s`
Expected in each case: the scenario's bounded poll converges (a 200 from the deployed stage carrying the rendered body, or the documented 429/400 refusal) well inside its budget. Actual: no response is ever graded.

Cause 2 of 2 (replay, apigateway.delete-rest-api-racing-base-path-mapping-leaves-orphaned-mapping):
```
AssertionError: unexpected BadRequestException message "Cannot delete REST API 'jho0tn34ri', is referenced in base path mapping: sim2real-behav-mapdom-3a9bb08de086.example.com/v1"
```
The check grades the losing racer with `_assert_refusal(delete_result, f"Deleting RestApi {api_id} failed. Please remove all base path mappings related to the RestApi in your domains: {domain}")` — i.e. expected 400 BadRequestException with `Deleting RestApi <restApiId> failed. Please remove all base path mappings related to the RestApi in your domains: <domainName>` (the api id and domain are per-run values filling that template), got the emulator's `Cannot delete REST API '<restApiId>', is referenced in base path mapping: <domainName>/<basePath>`. The sibling sentence for the winning-delete branch, `Invalid REST API identifier specified`, is also graded by this check.

## Steps to Reproduce
Cause 1 of 2 — each scenario, run against the emulator with a bounded poll:
(a) Create two REGIONAL REST APIs (front + MOCK backend minting a new id per hit), declare cacheKeyParameters for one query parameter on the front method, enable the stage cache, deploy; then GET the stage varying the UNDECLARED parameter (must hit the cache), varying the DECLARED one (must miss), and after FlushStageCache (must reach the backend again).
(b) Front + backend REST APIs whose response templates render $context.requestId and $context.extendedRequestId; GET the front stage once with a client-supplied request id and compare the three values plus the x-amz-apigw-id header.
(c) Deploy the usage-plan CloudFormation template (RestApi + Resource + Method ApiKeyRequired + Deployment + Stage + ApiKey + UsagePlan with a DAY quota of 3 + UsagePlanKey); call the invoke URL with x-api-key 4 times (3 served, 4th 429 "Limit Exceeded") and then GetUsage for the plan/date range.
(d) Create a TOKEN authorizer with authorizerResultTtlInSeconds=300 over a Lambda that counts invocations, deploy, GET twice (second must be cached), call FlushStageAuthorizersCache, GET again (must re-invoke).
(e) Register a request validator with a requestModels entry for application/json only; POST a schema-violating body as text/plain (must reach the integration) and the same body as application/json (must be refused 400).
Each of the five never produces a graded response on the emulator; the run ends at the harness budget.

Cause 2 of 2:
1. CreateRestApi; put a method/integration and CreateDeployment with a stage.
2. CreateDomainName, then CreateBasePathMapping(domainName, basePath="v1", restApiId, stage).
3. Call DeleteRestApi(restApiId). It is refused 400 BadRequestException (correct) — compare the message against `Deleting RestApi <restApiId> failed. Please remove all base path mappings related to the RestApi in your domains: <domainName>`; the emulator answers `Cannot delete REST API '<restApiId>', is referenced in base path mapping: <domainName>/<basePath>`.
