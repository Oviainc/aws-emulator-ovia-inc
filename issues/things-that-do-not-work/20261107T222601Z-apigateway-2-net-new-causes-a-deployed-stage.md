Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found apigateway.flush-caches-bypass-ttl
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found|_served#19|Timeout|time.sleep(3)
Issue-Key: apigateway.flush-caches-bypass-ttl|_poll_until#24|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s, last value={last!r}")
Issue-Commit: 7f27151474f220c715589d81820913dcd4b7b62b

# apigateway: 2 net-new causes — a deployed stage never serves a MOCK method on a resource carrying both GET and ANY, and a cacheClusterEnabled stage never leaves CREATE_IN_PROGRESS

> _Note: this issue was identified against the code base at commit `7f27151474f220c715589d81820913dcd4b7b62b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Two net-new root causes in the apigateway emulator, both grounded in failing behavioral checks that are not covered by any open issue.

Cause 1 of 2: a deployed REST stage never serves a MOCK method on a resource that carries BOTH an explicit verb and ANY.
Failing check: apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found
  AWS: on a resource with a GET method and an ANY method (Smithy: PutMethod httpMethod, PutIntegration type=MOCK with requestTemplates application/json '{"statusCode": 200}', PutIntegrationResponse responseTemplates), the execute-api data plane routes GET to the explicit GET method and every other verb (POST here) to ANY, and answers 200 with the body produced by that method's integration response template ($context.httpMethod resolved to the request verb). Deleting GET and redeploying makes ANY serve GET as well. The control-plane side of the same contract — GetMethod for a verb that only ANY covers is refused NotFoundException "Invalid Method identifier specified" (Smithy: GetMethod error NotFoundException) — is asserted earlier in the check and did NOT fail, so the control plane holds the methods correctly.
  Emulator: after CreateDeployment to stage 'beta', the stage URL never answers 200 with the integration-response body for either verb. The check re-issues the signed GET for a 180 s window and the emulator never produces the expected response, so the check reaches no verdict at all ("Check timed out after 180.0s"). The explicit-verb-vs-ANY routing precedence and the MOCK response-template rendering are therefore both unobservable.
  Why: a client that deploys a resource with GET plus ANY and calls either verb gets no usable response from the stage; routing precedence between an explicit verb and ANY cannot be relied on, and a request that AWS serves 200 hangs or fails here.
  Fix: the execute-api data-plane route resolver for a deployed stage (explicit-verb-before-ANY match) and the MOCK integration/integration-response renderer it dispatches to.
  Traces: apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found

Cause 2 of 2: a stage created with cacheClusterEnabled=true never leaves CREATE_IN_PROGRESS.
Failing check: apigateway.flush-caches-bypass-ttl
  AWS: CreateStage with cacheClusterEnabled=true and cacheClusterSize="0.5" provisions a response cache cluster; GetStage.cacheClusterStatus (Smithy: Stage.cacheClusterStatus -> enum CacheClusterStatus with values CREATE_IN_PROGRESS, AVAILABLE, DELETE_IN_PROGRESS, NOT_AVAILABLE, FLUSH_IN_PROGRESS) moves out of CREATE_IN_PROGRESS to a terminal value — AVAILABLE on success, NOT_AVAILABLE on failure — and only then is the stage's per-method caching usable. The terminal value is the requirement; how long provisioning takes is not.
  Emulator: GetStage keeps reporting cacheClusterStatus 'CREATE_IN_PROGRESS' and never reaches AVAILABLE or NOT_AVAILABLE, so the assertion fails with "condition not met within 1.0s, last value='CREATE_IN_PROGRESS'". The 1 s poll window is the floor the check falls back to because the earlier steps of the same check — waiting for the backend MOCK stage to become servable through the stage URL — had already consumed its budget, which is the same slow stage-serving path as Cause 1; the diverging value asserted is the status itself, which is still the non-terminal CREATE_IN_PROGRESS. Everything after it in the check (per-method caching, FlushStageCache's 202, FlushStageAuthorizersCache leaving the response cache cluster untouched) is unreachable.
  Why: a client that enables a stage cache and waits for cacheClusterStatus to settle before switching traffic waits forever; nothing can confirm that per-method caching, FlushStageCache or FlushStageAuthorizersCache took effect.
  Fix: the stage state that models the cache cluster lifecycle (CreateStage/UpdateStage transition to AVAILABLE, FLUSH_IN_PROGRESS back to AVAILABLE on FlushStageCache) and the GetStage serializer that reports it.
  Traces: apigateway.flush-caches-bypass-ttl

## Evidence
Cause 1 — apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found
  Expected: GET https://<stage-url>/data-agreements/<id> -> 200 {"route": "explicit", "verb": "GET"}; POST same path -> 200 {"route": "any", "verb": "POST"}; after DeleteMethod(GET) + CreateDeployment, GET -> 200 {"route": "any", "verb": "GET"}.
  Actual: the stage never answers with any of those bodies; the check's bounded re-issue loop exhausts and the harness reports "Error: Check timed out after 180.0s". (The preceding control-plane assertions — resourceMethods == [ANY, GET] and GetMethod POST -> NotFoundException "Invalid Method identifier specified" — did not fail.)

Cause 2 — apigateway.flush-caches-bypass-ttl
  Expected: after CreateStage(cacheClusterEnabled=True, cacheClusterSize='0.5'), GetStage.cacheClusterStatus settles on a terminal value, 'AVAILABLE' for a successful provision.
  Actual: AssertionError: condition not met within 1.0s, last value='CREATE_IN_PROGRESS' (raised from the check's _poll_until over GetStage(...)['cacheClusterStatus']). Status never leaves the non-terminal CREATE_IN_PROGRESS.

## Steps to Reproduce
Cause 1:
1. CreateRestApi (REGIONAL). CreateResource 'data-agreements', then child '{agreementId}'.
2. On the child, PutMethod GET (authorizationType=AWS_IAM) + PutIntegration type=MOCK requestTemplates {'application/json': '{"statusCode": 200}'} + PutMethodResponse 200 + PutIntegrationResponse 200 with responseTemplates {'application/json': '{"route": "explicit", "verb": "$context.httpMethod"}'}. Repeat for httpMethod ANY with route 'any'.
3. CreateDeployment(stageName='beta').
4. SigV4-sign (service execute-api) a GET to https://<api-id>.execute-api.<region>.<host>/beta/data-agreements/dua-1 and retry while non-200. Observe the stage never returns 200 with {"route": "explicit", "verb": "GET"}; same for POST and ANY.

Cause 2:
1. CreateRestApi (REGIONAL), CreateResource 'items', PutMethod GET (authorizationType=NONE), PutIntegration type=HTTP_PROXY to any reachable backend URL.
2. CreateDeployment, then CreateStage(stageName='prod', deploymentId=<id>, cacheClusterEnabled=True, cacheClusterSize='0.5').
3. Poll GetStage(restApiId, stageName='prod')['cacheClusterStatus']. It stays 'CREATE_IN_PROGRESS' and never becomes 'AVAILABLE' (or 'NOT_AVAILABLE').
