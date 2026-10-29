Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-cache-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-cache-lifecycle appsync-cache-behavior-reconfiguration appsync-api-cache-update-flush-delete-chain
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|FlushApiCache|4xx->4xx|$.message
Issue-Key: appsync|UpdateApiCache|4xx->4xx|$.message
Issue-Key: appsync|CreateApiCache|2xx->2xx|$.apiCache.atRestEncryptionEnabled
Issue-Key: appsync|CreateApiCache|2xx->2xx|$.apiCache.transitEncryptionEnabled
Issue-Key: appsync|GetApiCache|2xx->2xx|$.apiCache.atRestEncryptionEnabled
Issue-Key: appsync|GetApiCache|2xx->2xx|$.apiCache.transitEncryptionEnabled
Issue-Key: appsync|CreateApiCache|4xx->4xx|$.message
Issue-Key: appsync|UpdateApiCache|4xx->2xx|$.message
Issue-Key: appsync|UpdateApiCache|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|UpdateApiCache|4xx->2xx|status
Issue-Key: appsync|GetApiCache|2xx->2xx|$.apiCache.apiCachingBehavior
Issue-Key: appsync|GetApiCache|2xx->2xx|$.apiCache.ttl
Issue-Key: appsync|UpdateApiCache|2xx->2xx|$.apiCache.atRestEncryptionEnabled
Issue-Key: appsync|UpdateApiCache|2xx->2xx|$.apiCache.status
Issue-Key: appsync|UpdateApiCache|2xx->2xx|$.apiCache.transitEncryptionEnabled
Issue-Key: appsync|GetApiCache|4xx->4xx|$.message
Issue-Key: appsync|GetApiCache|2xx->2xx|$.apiCache.healthMetricsConfig
Issue-Key: appsync|GetApiCache|2xx->2xx|$.apiCache.type
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# AppSync ApiCache: refusal wording, encryption flags and applied update state all diverge (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
FlushApiCache

## Description
The AppSync ApiCache surface diverges in three related ways: every per-operation NotFoundException/BadRequestException sentence is replaced by one generic emulator-authored sentence, the two encryption flags are never persisted or echoed, and an UpdateApiCache is not applied to the stored cache (nor refused while the cache is not yet AVAILABLE). 10 net-new causes, all in the ApiCache handler/state.

Cause 1 of 10 — FlushApiCache on an API with no cache answers the wrong message.
AWS: 404 NotFoundException with the fixed sentence "Unable to flush the cache as it doesn't exist, please create the cache first." (recorded behavior; Smithy: FlushApiCache errors include NotFoundException httpError 404). The sentence is entirely fixed text — it carries no request or state values.
Emulator: 404 with "Unable to get the cache as it doesn't exist, please create the cache first." — the GetApiCache sentence, i.e. one shared not-found string for every cache operation.
Why: a client that branches on the refusal text (or a test asserting it) cannot tell a failed flush from a failed read.
Fix: the FlushApiCache handler's not-found refusal must carry its own operation-specific sentence rather than delegating to the shared get-cache message.
Traces: appsync-api-cache-lifecycle#task#3.

Cause 2 of 10 — UpdateApiCache on an API with no cache answers the wrong message.
AWS: 404 NotFoundException, fixed text "Unable to update the cache as it doesn't exist, please create the cache first." (recorded behavior).
Emulator: 404 with "Unable to get the cache as it doesn't exist, please create the cache first.".
Why: same as cause 1, for the update path.
Fix: UpdateApiCache handler's not-found refusal.
Traces: appsync-api-cache-lifecycle#task#4, appsync-cache-behavior-reconfiguration#task#7.

Cause 3 of 10 — CreateApiCache does not persist/echo transitEncryptionEnabled and atRestEncryptionEnabled.
AWS: the request's transitEncryptionEnabled/atRestEncryptionEnabled are stored and returned in apiCache. Smithy: ApiCache.transitEncryptionEnabled and ApiCache.atRestEncryptionEnabled are booleans with default false and are documented "You cannot update this setting after creation" — so the create-time value is the value every later read must report.
Emulator: returns false for both even when the request set them true.
Why: a client that enabled encryption at creation is told encryption is off, and since the member cannot be updated it has no way to correct it.
Fix: CreateApiCache validation/state — carry both request members into the stored ApiCache, and the serializer must echo the stored values.
Traces: appsync-api-cache-lifecycle#task#5, appsync-cache-behavior-reconfiguration#setup#3.

Cause 4 of 10 — GetApiCache reports both encryption flags as false.
AWS: GetApiCache returns the flags as created.
Emulator: always false.
Why: reads disagree with what was requested at creation.
Fix: same stored-state defect as cause 3, observed through the GetApiCache projection.
Traces: appsync-api-cache-lifecycle#task#6, appsync-api-cache-lifecycle#task#9, appsync-api-cache-update-flush-delete-chain#task#6.

Cause 5 of 10 — CreateApiCache on an API that already has a cache uses emulator-authored wording.
AWS: 400 BadRequestException with the fixed sentence "The API has already enabled caching." (recorded behavior).
Emulator: 400 with "The API already has a cache. Delete the existing cache before creating another one.".
Why: the status and class are right but the message is not AWS's, so text assertions and user-facing surfacing diverge.
Fix: CreateApiCache handler's already-exists refusal message.
Traces: appsync-api-cache-lifecycle#task#7.

Cause 6 of 10 — UpdateApiCache is not refused while the cache is not yet AVAILABLE.
AWS: 400 BadRequestException with the fixed sentence "The cache is not yet available. Please wait until it becomes available." when the cache status is still CREATING/MODIFYING (Smithy: ApiCacheStatus values AVAILABLE, CREATING, DELETING, MODIFYING, FAILED; UpdateApiCache errors include BadRequestException httpError 400). The sentence is entirely fixed text.
Emulator: 200 with no error body and no x-amzn-errortype header — the update is accepted.
Why: a client that relies on AWS serialising cache mutations silently gets an update applied in a state AWS rejects, so its retry/wait loop never runs and the resulting cache configuration is undefined.
Fix: add the status gate to the UpdateApiCache handler, ahead of applying the change.
Traces: appsync-api-cache-update-flush-delete-chain#task#3, appsync-cache-behavior-reconfiguration#task#4.

Cause 7 of 10 — an accepted UpdateApiCache is never applied: GetApiCache still reports the pre-update ttl and apiCachingBehavior.
AWS: after UpdateApiCache, GetApiCache reports the updated ttl and apiCachingBehavior (Smithy: ApiCache.ttl long, ApiCache.apiCachingBehavior enum ApiCachingBehavior).
Emulator: reports the values from the create call instead of the updated ones (recorded step: expected ttl 300 / FULL_REQUEST_CACHING, got 600 / PER_RESOLVER_CACHING — the create-time values).
Why: the whole update path is a no-op as far as subsequent reads are concerned, so a client cannot reconfigure a cache.
Fix: UpdateApiCache must write the mutated members into cache state.
Traces: appsync-api-cache-update-flush-delete-chain#task#4.

Cause 8 of 10 — UpdateApiCache's own reply reports status MODIFYING where AWS reports AVAILABLE.
AWS: the UpdateApiCache response carries apiCache.status AVAILABLE for the recorded flow (Smithy: ApiCacheStatus).
Emulator: MODIFYING.
Why: a client that polls until status != MODIFYING waits on a transition the emulator never completes.
Fix: the ApiCache state machine in the UpdateApiCache handler.
Traces: appsync-api-cache-update-flush-delete-chain#task#5.

Cause 9 of 10 — GetApiCache after DeleteApiCache does not use the deleted-cache sentence.
AWS: 404 NotFoundException with the fixed text "Unable to get the cache as it has been deleted, please re-create the cache." — AWS distinguishes never-created from deleted (recorded behavior).
Emulator: "Unable to get the cache as it doesn't exist, please create the cache first.".
Why: the deleted-vs-absent distinction AWS makes is lost.
Fix: ApiCache state must retain a deleted marker and GetApiCache must select the deleted-cache sentence from it.
Traces: appsync-api-cache-update-flush-delete-chain#task#9.

Cause 10 of 10 — after a cache reconfiguration, GetApiCache reports stale ttl/apiCachingBehavior/type and an unrequested healthMetricsConfig.
AWS: GetApiCache reports the reconfigured ttl, apiCachingBehavior and type, and omits healthMetricsConfig when it was not configured (Smithy: ApiCache.type enum ApiCacheType, ApiCache.healthMetricsConfig enum CacheHealthMetricsConfig, both optional).
Emulator: serves the earlier configuration and emits healthMetricsConfig ENABLED where AWS omits the member entirely.
Why: a reconfiguration is invisible to reads and an absent member appears populated, so clients see configuration that was never requested.
Fix: ApiCache state update plus the GetApiCache serializer's optional-member handling.
Traces: appsync-cache-behavior-reconfiguration#task#6.

Related open issues that do NOT cover these: "AppSync ApiCache: wrong NotFoundException/BadRequestException messages for get/delete/flush of a missing cache..." (20261029T174359Z) and "AppSync: FlushApiCache never succeeds..." (20261025T011758Z) — neither carries these replay keys, and neither covers the encryption flags or the un-applied update.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline.

1) appsync-api-cache-lifecycle#task#3 FlushApiCache 404: $.message expected "Unable to flush the cache as it doesn't exist, please create the cache first.", got "Unable to get the cache as it doesn't exist, please create the cache first."
2) appsync-api-cache-lifecycle#task#4 UpdateApiCache 404: expected "Unable to update the cache as it doesn't exist, please create the cache first.", got the get-cache sentence.
3) appsync-api-cache-lifecycle#task#5 CreateApiCache 200: $.apiCache.transitEncryptionEnabled expected True got False; $.apiCache.atRestEncryptionEnabled expected True got False.
4) appsync-api-cache-lifecycle#task#6 GetApiCache 200: same two flags expected True got False.
5) appsync-api-cache-lifecycle#task#7 CreateApiCache 400: expected 'The API has already enabled caching.', got 'The API already has a cache. Delete the existing cache before creating another one.'
6) appsync-api-cache-update-flush-delete-chain#task#3 UpdateApiCache: status expected 400 got 200; $.message expected 'The cache is not yet available. Please wait until it becomes available.' got None; $headers.x-amzn-errortype missing.
7) appsync-api-cache-update-flush-delete-chain#task#4 GetApiCache 200: $.apiCache.ttl expected 300 got 600; $.apiCache.apiCachingBehavior expected 'FULL_REQUEST_CACHING' got 'PER_RESOLVER_CACHING'.
8) appsync-api-cache-update-flush-delete-chain#task#5 UpdateApiCache 200: $.apiCache.status expected 'AVAILABLE' got 'MODIFYING'.
9) appsync-api-cache-update-flush-delete-chain#task#9 GetApiCache 404: expected 'Unable to get the cache as it has been deleted, please re-create the cache.', got the doesn't-exist sentence.
10) appsync-cache-behavior-reconfiguration#task#6 GetApiCache 200: ttl expected 60 got 3600; apiCachingBehavior expected 'PER_RESOLVER_CACHING' got 'FULL_REQUEST_CACHING'; type expected 'SMALL' got 'LARGE'; healthMetricsConfig expected absent got 'ENABLED'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Inspect fixtures appsync-api-cache-lifecycle, appsync-api-cache-update-flush-delete-chain and appsync-cache-behavior-reconfiguration; the steps listed under Traces mismatch.
4. By hand: CreateGraphqlApi, then FlushApiCache/UpdateApiCache before any cache exists (causes 1-2); CreateApiCache with transitEncryptionEnabled=true and atRestEncryptionEnabled=true then GetApiCache (causes 3-4); CreateApiCache twice (cause 5); UpdateApiCache while status is not AVAILABLE (cause 6); UpdateApiCache ttl/apiCachingBehavior then GetApiCache (causes 7-8, 10); DeleteApiCache then GetApiCache (cause 9).
