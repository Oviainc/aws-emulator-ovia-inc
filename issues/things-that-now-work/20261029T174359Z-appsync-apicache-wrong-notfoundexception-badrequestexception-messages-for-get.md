Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.api-cache-flush-and-update-lifecycle appsync.api-cache-lifecycle-transitions appsync.api-cache-update-persists appsync.op-contract-delete-api-cache appsync.flush-api-cache-deleted-cache-rejected appsync.flush-api-cache-no-cache-configured-rejected appsync.flush-api-cache-no-cache-rejected appsync.flush-api-cache-nonexistent-cache-bad-request appsync.concurrent-api-cache-create-rejected appsync.concurrent-update-api-cache
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.api-cache-flush-and-update-lifecycle|run#28|AssertionError|assert message == (
Issue-Key: appsync.api-cache-lifecycle-transitions|run#22|AssertionError|assert gone == (
Issue-Key: appsync.api-cache-update-persists|run#24|AssertionError|assert gone == (
Issue-Key: appsync.op-contract-delete-api-cache|run#16|AssertionError|assert message == "Unable to get the cache as it has been deleted, please re-create the cache.", (
Issue-Key: appsync.flush-api-cache-deleted-cache-rejected|run#5|AssertionError|assert delete_message == "Unable to delete the cache as it doesn't exist, please create the cache first.", (
Issue-Key: appsync.flush-api-cache-no-cache-configured-rejected|run#5|AssertionError|assert message == "Unable to flush the cache as it doesn't exist, please create the cache first.", (
Issue-Key: appsync.flush-api-cache-no-cache-rejected|run#8|AssertionError|assert message == "Unable to flush the cache as it doesn't exist, please create the cache first.", (
Issue-Key: appsync.flush-api-cache-nonexistent-cache-bad-request|run#5|AssertionError|assert message == "Unable to flush the cache as it doesn't exist, please create the cache first.", (
Issue-Key: appsync.concurrent-api-cache-create-rejected|run#13|AssertionError|assert failures[0][2] == "The API has already enabled caching.", (
Issue-Key: appsync.concurrent-update-api-cache|_assert_delete_race#18|AssertionError|assert successes >= 1, f"expected at least 1 {op_name} racer to win, got {successes} (results={results!r})"
Issue-Commit: 910698ab7b22be6d08fbb15c4abd76821047ddd2

# AppSync ApiCache: wrong NotFoundException/BadRequestException messages for get/delete/flush of a missing cache, and DeleteApiCache is refused after an UpdateApiCache race

> _Note: this issue was identified against the code base at commit `910698ab7b22be6d08fbb15c4abd76821047ddd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApiCache

## Description
Five distinct net-new root causes in the AppSync api-cache control plane surfaced by this behavioral replay. 5 causes in this issue; all five must be closed before submitting. (The open issue 20261025T011758Z-appsync-flushapicache-never-succeeds-both-concurrency-checks-updatedatasource.md covers FlushApiCache returning no success at all and UpdateDataSource validation order; none of the five causes below is one of those, and none of these checks carries that issue's replay key.)

=== Cause 1 of 5: GetApiCache after a successful DeleteApiCache reports the never-existed message instead of the deleted message ===
AWS: Smithy (appsync) GetApiCache — GET /v1/apis/{apiId}/ApiCaches, input apiId (required, httpLabel), modeled errors BadRequestException|400, ConcurrentModificationException|409, InternalFailureException|500, NotFoundException|404, UnauthorizedException|401. Recorded behavior: once DeleteApiCache has taken effect, GetApiCache answers 404 NotFoundException with the message 'Unable to get the cache as it has been deleted, please re-create the cache.' — AWS distinguishes a cache that WAS deleted from an api that never had one. Fixed text of that message is the whole sentence; it carries no request- or state-derived variable parts.
Emulator: 404 NotFoundException with the never-existed wording 'Unable to get the cache as it doesn't exist, please create the cache first.' for a cache that was just deleted.
Why: a client (and the AWS console/CLI flows built on it) cannot distinguish 'your cache was torn down, re-create it' from 'you never configured caching on this api', so delete-then-verify tooling reports the wrong remediation and idempotent teardown logic that keys on the deleted wording never fires.
Fix: the GetApiCache handler's not-found branch plus the api-cache state component must retain the deleted-vs-absent distinction for the api and select the message from it.
Traces: appsync.api-cache-flush-and-update-lifecycle, appsync.api-cache-lifecycle-transitions, appsync.api-cache-update-persists, appsync.op-contract-delete-api-cache.

=== Cause 2 of 5: DeleteApiCache on an api with no cache returns GetApiCache's message ===
AWS: Smithy (appsync) DeleteApiCache — DELETE /v1/apis/{apiId}/ApiCaches, empty output, modeled errors BadRequestException|400, ConcurrentModificationException|409, InternalFailureException|500, NotFoundException|404, UnauthorizedException|401. Recorded behavior: deleting a cache that is not there is refused with NotFoundException whose message names the attempted operation: 'Unable to delete the cache as it doesn't exist, please create the cache first.'
Emulator: the refusal carries the GET wording 'Unable to get the cache as it doesn't exist, please create the cache first.' — the operation name in the message is wrong.
Why: the message is the only place the control plane names which cache operation failed; a client logging/branching on it mis-attributes the failure to a read it never made.
Fix: the DeleteApiCache handler must emit its own not-found message rather than delegating to the GetApiCache message builder.
Traces: appsync.flush-api-cache-deleted-cache-rejected.

=== Cause 3 of 5: FlushApiCache on an api with no cache returns GetApiCache's message ===
AWS: Smithy (appsync) FlushApiCache — DELETE /v1/apis/{apiId}/FlushCache, empty output, modeled errors include NotFoundException|404. Recorded behavior: flushing when no cache is configured (or after the cache was deleted) is refused with NotFoundException and the flush-specific message 'Unable to flush the cache as it doesn't exist, please create the cache first.'
Emulator: NotFoundException with 'Unable to get the cache as it doesn't exist, please create the cache first.'
Why: same class as Cause 2 — the refusal names the wrong operation, so cache-invalidation tooling cannot tell a failed flush from a failed read.
Fix: the FlushApiCache handler's not-found branch must build its own message.
Traces: appsync.flush-api-cache-no-cache-configured-rejected, appsync.flush-api-cache-no-cache-rejected, appsync.flush-api-cache-nonexistent-cache-bad-request.

=== Cause 4 of 5: CreateApiCache on an api that already has a cache uses non-AWS wording ===
AWS: Smithy (appsync) CreateApiCache — POST /v1/apis/{apiId}/ApiCaches, required apiId/ttl/apiCachingBehavior/type, modeled errors BadRequestException|400, ConcurrentModificationException|409, NotFoundException|404, UnauthorizedException|401. Recorded behavior: a second CreateApiCache for the same api (including the loser of two simultaneous creates) is refused 400 BadRequestException with the fixed message 'The API has already enabled caching.'
Emulator: 400 BadRequestException with 'The API already has a cache. Delete the existing cache before creating another one.'
Why: the error code is right but the message AWS defines for this class is a requirement; clients and test gates that match AWS's sentence treat the emulator's refusal as an unknown failure.
Fix: the CreateApiCache handler's already-exists branch (message text).
Traces: appsync.concurrent-api-cache-create-rejected.

=== Cause 5 of 5: after concurrent UpdateApiCache calls the cache is left in a state where every DeleteApiCache is refused ===
AWS: recorded behavior only for the race arbitration (the Smithy model lists the error set but not the state machine). Recorded behavior: after racing UpdateApiCache calls on the same api the cache settles back to status AVAILABLE, and of two simultaneous DeleteApiCache calls at least one succeeds with 200 and an empty body (the loser gets a not-found/concurrent-modification refusal). A delete is never refused merely because an update preceded it.
Emulator: BOTH DeleteApiCache racers are refused 400 BadRequestException 'Cannot delete cache as it is either not available or its creation failed.', so no racer wins and the cache can no longer be removed. Hypothesis: the cache is left in a non-AVAILABLE (MODIFYING/FAILED) status after the update race and DeleteApiCache gates on status == AVAILABLE.
Why: the cache becomes undeletable — a client that reconfigures a cache can neither delete it nor re-create it (Cause 4's already-exists refusal), so the api is permanently stuck paying for a cache it cannot manage.
Fix: the api-cache state component's post-update status settlement plus the DeleteApiCache handler's status precondition (a delete must not require AVAILABLE, and the cache must return to AVAILABLE after an update).
Traces: appsync.concurrent-update-api-cache.

## Evidence
Cause 1 — expected: NotFoundException 'Unable to get the cache as it has been deleted, please re-create the cache.' Actual: NotFoundException 'Unable to get the cache as it doesn't exist, please create the cache first.' (check_appsync_api_cache_flush_and_update_lifecycle.py line 81 assert; appsync.api-cache-update-persists reports 'GetApiCache on a deleted cache should 404 with NotFoundException, got (\'NotFoundException\', "Unable to get the cache as it doesn't exist, please create the cache first.")'; appsync.op-contract-delete-api-cache: 'expected the exact NotFoundException message AWS returns, got "Unable to get the cache as it doesn't exist, please create the cache first."').
Cause 2 — expected: "Unable to delete the cache as it doesn't exist, please create the cache first." Actual: 'unexpected delete_api_cache message: "Unable to get the cache as it doesn\'t exist, please create the cache first."'.
Cause 3 — expected: "Unable to flush the cache as it doesn't exist, please create the cache first." Actual (3 checks): 'unexpected FlushApiCache message: "Unable to get the cache as it doesn\'t exist, please create the cache first."'.
Cause 4 — expected BadRequestException message exactly 'The API has already enabled caching.'. Actual: 'The API already has a cache. Delete the existing cache before creating another one.'.
Cause 5 — expected at least one DeleteApiCache racer to return 200. Actual: 'expected at least 1 DeleteApiCache racer to win, got 0 (results=[BadRequestException(... DeleteApiCache ... Cannot delete cache as it is either not available or its creation failed.), BadRequestException(... same ...)])' (check_appsync_concurrent_update_api_cache.py line 104).

## Steps to Reproduce
Causes 1-3: CreateGraphqlApi(authenticationType=API_KEY); CreateApiCache(apiId, ttl=300, apiCachingBehavior=FULL_REQUEST_CACHING, type=SMALL); poll GetApiCache until status=AVAILABLE; UpdateApiCache(ttl=600); FlushApiCache; DeleteApiCache; then (1) poll GetApiCache until it 404s — message must be 'Unable to get the cache as it has been deleted, please re-create the cache.'; (2) call DeleteApiCache again — message must be "Unable to delete the cache as it doesn't exist, please create the cache first."; (3) call FlushApiCache on an api that never had a cache — message must be "Unable to flush the cache as it doesn't exist, please create the cache first."
Cause 4: CreateGraphqlApi; CreateApiCache twice (or two simultaneously) — the second/losing call must be 400 BadRequestException 'The API has already enabled caching.'
Cause 5: CreateGraphqlApi; CreateApiCache and wait for AVAILABLE; fire two UpdateApiCache(ttl=600, apiCachingBehavior=FULL_REQUEST_CACHING, type=SMALL) concurrently; then fire two DeleteApiCache concurrently — at least one must return 200, and GetApiCache must then report the deleted state.
