Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters
Issue-Key: appsync__cloudwatch|CreateApiKey|NotFoundException
Issue-Commit: 8b3fb3b9641f9f0b47110ea9811423e23ffd107b

# AppSync CreateApiKey refuses an Event API's apiId with 404 NotFoundException "API not found.", so no Event API can be given an API key

> _Note: this issue was identified against the code base at commit `8b3fb3b9641f9f0b47110ea9811423e23ffd107b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiKey

## Description
Cause 1 of 1: an Api created by CreateApi (an AppSync Events API) is invisible to the API-key operations, so CreateApiKey answers NotFoundException for an apiId that exists.
AWS: Smithy CreateApiKey is POST /v1/apis/{apiId}/apikeys with required httpLabel `apiId` and optional `description`/`expires`, success code 200 returning `apiKey`; its modeled errors are ApiKeyLimitExceededException(400), ApiKeyValidityOutOfBoundsException(400), BadRequestException(400), LimitExceededException(429), NotFoundException(404), UnauthorizedException(401), InternalFailureException(500). The apiId path label is the same identifier space for both resource families: an Event API whose eventConfig declares an API_KEY auth provider is given its key by calling CreateApiKey with the apiId that CreateApi returned, and that key is then presented as x-api-key when connecting to the Api's realtime/HTTP event endpoints. Recorded behavior (the check's values were measured against real AWS us-west-2): CreateApi -> 200 with api.apiId and api.dns, then CreateApiKey(apiId=<that apiId>) -> 200 with apiKey.id, which authenticates the subsequent Event-API connect/subscribe/publish. NotFoundException/404 on CreateApiKey is the answer for an apiId that does not exist — the same code and message the emulator correctly returns from GetApi after DeleteApi ("API not found.").
Emulator: CreateApi now succeeds (the scaffold-stub issue 20261024T032443Z-appsync-createapi-stub.md is marked fixed), but the immediately following CreateApiKey(apiId=<the just-created Event API>) is refused with error code `NotFoundException` (HTTP 404) and message "API not found." Hypothesis: the api-key handler resolves apiId only against the GraphqlApi registry, so Apis created through the v2 CreateApi path are not found there; the check never reaches CreateChannelNamespace, the realtime connection, or any metric read.
Why: an AppSync Events API using the API_KEY auth provider cannot be authenticated at all against this emulator — there is no way to obtain the key its connectionAuthModes/defaultPublishAuthModes require — so the whole Event-API data plane (connect, subscribe, publish) and every AWS/AppSync Event-API metric is unreachable. A client gets a 404 for a resource it just created one call earlier, which reads as a nonexistent Api rather than an emulator gap.
Fix: the CreateApiKey handler's apiId resolution in the appsync service crate (ops/create_api_key.rs) and the shared Api/GraphqlApi lookup in the crate's state, so both resource families resolve; the sibling key operations that take the same apiId label (ListApiKeys, UpdateApiKey, DeleteApiKey) must resolve Event APIs the same way.
Related: open issue 20261024T165547Z-appsync-createapi-returns-notimplemented-so-event-api-cloudwatch.md names this same check but its recorded cause (CreateApi answering NotImplemented) no longer reproduces and its replay key (appsync__cloudwatch|CreateApi|NotImplemented) does not cover this one — the failure has moved one call later, to CreateApiKey. The Event-API CloudWatch metric-emission half of that issue remains unverifiable until this refusal is lifted.
Traces: appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters.

## Evidence
appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters (verdict: error):
  expected: create_api(name=<unique>, eventConfig={authProviders:[{authType:API_KEY}], connectionAuthModes:[{authType:API_KEY}], defaultPublishAuthModes:[{authType:API_KEY}], defaultSubscribeAuthModes:[{authType:API_KEY}]}) -> 200; then create_api_key(apiId=<api.apiId>, description=...) -> HTTP 200 with apiKey.id usable as x-api-key against the Api's event endpoints.
  actual:   botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the CreateApiKey operation: API not found.
            (raised at the create_api_key call, two statements after the successful create_api)
  Smithy: CreateApiKey POST /v1/apis/{apiId}/apikeys, success code 200 with `apiKey`; NotFoundException(404) is the answer for an apiId that does not exist, and the apiId here was returned by CreateApi in the preceding call.
  Note: CreateApi itself now returns 200 — the previously filed cause (CreateApi -> NotImplemented) no longer reproduces; the refusal has moved to CreateApiKey.

## Steps to Reproduce
1. Boot the emulator and point an AppSync client at it.
2. Call appsync CreateApi with a valid name and an eventConfig whose authProviders/connectionAuthModes/defaultPublishAuthModes/defaultSubscribeAuthModes are all [{authType: API_KEY}]. Observe HTTP 200 and note api.apiId.
3. Call appsync CreateApiKey(apiId=<that apiId>, description="anything"). Observe error code NotFoundException (HTTP 404, message "API not found.") where AWS returns 200 with apiKey.id.
4. For contrast, call CreateGraphqlApi and CreateApiKey against that apiId — it succeeds, showing the key path resolves only the GraphqlApi registry.
5. Also check ListApiKeys/UpdateApiKey/DeleteApiKey with the Event API's apiId for the same refusal.
6. Replay appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters.
