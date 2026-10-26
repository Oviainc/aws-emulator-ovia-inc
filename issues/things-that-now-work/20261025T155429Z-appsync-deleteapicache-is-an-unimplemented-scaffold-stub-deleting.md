Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.api-cache-lifecycle-state-transitions
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.api-cache-lifecycle-state-transitions|run#14|AssertionError|assert code == "BadRequestException", (
Issue-Commit: 3e2c83ced69a5d134aa927e429a7d731168ed0a2

# AppSync DeleteApiCache is an unimplemented scaffold stub: deleting a CREATING api cache answers 501 NotImplemented instead of 400 BadRequestException (1 cause)

> _Note: this issue was identified against the code base at commit `3e2c83ced69a5d134aa927e429a7d731168ed0a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApiCache

## Description
1 net-new root cause in this issue; it must be closed before submitting.

=== Cause 1 of 1: DeleteApiCache answers 501 NotImplemented, so the api-cache delete precondition is never enforced ===

AWS: Smithy (appsync, com.amazonaws.appsync#DeleteApiCache) — DELETE /v1/apis/{apiId}/ApiCaches, input `apiId` (String, required, httpLabel), EMPTY output shape, HTTP success code 200; modeled errors BadRequestException|400, ConcurrentModificationException|409, InternalFailureException|500, NotFoundException|404, UnauthorizedException|401. Model doc: "Deletes an ApiCache object"; BadRequestException is documented as "The request is not well formed. For example, a value is invalid or a required field is missing." Recorded behavior for the state-machine precondition: an api cache may only be deleted once it has reached AVAILABLE — DeleteApiCache against a cache still in status CREATING is refused with HTTP 400 `BadRequestException` and the message `Cannot delete cache as it is either not available or its creation failed.` That message is wholly fixed text for this error class (it interpolates no request or state value), and after the refusal GetApiCache must still report the cache (status CREATING or AVAILABLE) — the refused delete must not remove it. Freshly created caches are expected to be AVAILABLE only on a service that provisions instantly; the contract under test is the CREATING-branch refusal.

Emulator: DeleteApiCache is still the generated scaffold — services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_api_cache.rs carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteApiCache"))`, so every call answers HTTP 501 with error code `NotImplemented` and message `operation DeleteApiCache is scaffolded but not implemented in this emulator`. The replay reaches that call after the earlier steps of the check all PASS (GetApiCache before any cache correctly 404s NotFoundException with AWS's sentence, CreateApiCache succeeds, and GetApiCache echoes ttl=300 / type=SMALL / apiCachingBehavior=FULL_REQUEST_CACHING at status CREATING), and fails with `AssertionError: expected BadRequestException deleting a CREATING cache, got NotImplemented`. Because the check captures the ClientError and asserts on its code, the suite scored this as a mismatch rather than an unimplemented-op error, so no auto-filed stub issue covers it; the open issue 20261025T011758Z-appsync-flushapicache-never-succeeds-both-concurrency-checks-updatedatasource.md covers FlushApiCache (a different operation) and does not mention DeleteApiCache.

Why: a client can never delete an AppSync api cache at all — not from AVAILABLE (the legal path, which should return an empty 200) and not from CREATING (which should be a modeled 400). It gets an unmodeled 501/NotImplemented that no AWS SDK retry or error-branching path expects, so cache teardown and the whole create→delete→recreate lifecycle are unusable, and any client that relies on the delete-while-provisioning guard (e.g. tooling that distinguishes "too early, retry" from "gone") cannot observe it.

Fix: the DeleteApiCache handler plus the api-cache state component — the handler must read the cache for `apiId`, return 404 NotFoundException when none exists, refuse 400 BadRequestException with AWS's fixed sentence while the cache is not AVAILABLE (leaving the cache in place), and otherwise remove the cache and return the empty 200 body; the SCAFFOLDED marker line must be deleted once implemented.

Traces: appsync.api-cache-lifecycle-state-transitions.

## Evidence
Check `appsync.api-cache-lifecycle-state-transitions` [mismatch]:

Expected: `DeleteApiCache(apiId=<api>)` against a cache in status CREATING is refused with HTTP 400, Error.Code `BadRequestException`, Error.Message `Cannot delete cache as it is either not available or its creation failed.` (fixed text, no request/state substitution), and a following GetApiCache still returns the cache with status in {CREATING, AVAILABLE}.

Actual: HTTP 501, Error.Code `NotImplemented`, Error.Message `operation DeleteApiCache is scaffolded but not implemented in this emulator`. Replay failure: `check_appsync_api_cache_lifecycle_state_transitions.py`, line 65 — `AssertionError: expected BadRequestException deleting a CREATING cache, got NotImplemented`.

Source evidence: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_api_cache.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED`) and line 157 (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteApiCache"))`).

Preceding steps of the same check passed, so this is the only divergence in the api-cache lifecycle path: GetApiCache on a cacheless api returned 404 NotFoundException with `Unable to get the cache as it doesn't exist, please create the cache first.`; CreateApiCache(ttl=300, apiCachingBehavior=FULL_REQUEST_CACHING, type=SMALL) succeeded; GetApiCache returned status CREATING with ttl/type/apiCachingBehavior echoed.

## Steps to Reproduce
1. CreateGraphqlApi(name=<unique>, authenticationType='API_KEY') and note apiId.
2. GetApiCache(apiId=<apiId>) — expect 404 NotFoundException, message `Unable to get the cache as it doesn't exist, please create the cache first.` (passes today).
3. CreateApiCache(apiId=<apiId>, ttl=300, apiCachingBehavior='FULL_REQUEST_CACHING', type='SMALL') (passes today).
4. GetApiCache(apiId=<apiId>) — status is CREATING with ttl=300, type='SMALL', apiCachingBehavior='FULL_REQUEST_CACHING' (passes today).
5. While status is CREATING, call DeleteApiCache(apiId=<apiId>) — MUST be refused with HTTP 400 BadRequestException and message `Cannot delete cache as it is either not available or its creation failed.`; today it answers HTTP 501 NotImplemented.
6. GetApiCache(apiId=<apiId>) again — the cache must still be present (status CREATING or AVAILABLE), i.e. the refused delete did not remove it.
7. Once the cache is AVAILABLE, DeleteApiCache(apiId=<apiId>) must succeed with an empty 200 body, a following GetApiCache must 404 NotFoundException, and FlushApiCache on the deleted cache must 404.
