Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.get-migration-cross-account-access-denied-not-not-found
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.get-migration-cross-account-access-denied-not-not-found|run#8|AssertionError|assert malformed_code == "ValidationException", (
Issue-Commit: 64ba80be2f956a355eead737f51c939a3b404043

# opensearch: GetMigration with an empty migrationId answers 501 NotImplemented instead of the modeled 400 ValidationException for the missing applicationId

> _Note: this issue was identified against the code base at commit `64ba80be2f956a355eead737f51c939a3b404043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMigration

## Description
One net-new root cause in the OpenSearch Service control plane, grounded in the one surviving behavioral check of the `opensearch` service in this run (every other opensearch and opensearch-cross-service failure was marked ALREADY FILED or OWNED and was skipped).

=== Cause 1 of 1: an empty required httpLabel on GetMigration collapses to 501 NotImplemented instead of a modeled ValidationException ===

AWS: Smithy: `GetMigration` is `GET /2021-01-01/opensearch/app-migrations/{migrationId}`, `migrationId` is a required `@httpLabel` String, and the operation models ValidationException (HTTP 400), ResourceNotFoundException (409), AccessDeniedException (403), DisabledOperationException (409), InternalException (500). An empty `migrationId` renders the request URI as the collection path `/2021-01-01/opensearch/app-migrations/`, which is the `ListMigrations` route (Smithy: `ListMigrations` is `GET /2021-01-01/opensearch/app-migrations` with a REQUIRED `applicationId` @httpQuery, pattern `^[a-z0-9]{3,30}$`) with no `applicationId` supplied. The recorded response is therefore the standard AWS missing-required-member refusal: HTTP 400, error code `ValidationException`, message `1 validation error detected: Value null at 'applicationId' failed to satisfy constraint: Member must not be null`. In that message the fixed text is `1 validation error detected: Value null at '`, `' failed to satisfy constraint: Member must not be null`, and the only variable part is the member name the route requires and the request did not carry (`applicationId` here); the error-count prefix `1 validation error detected` scales with the number of violations. The well-formed-but-nonexistent id path is NOT part of this cause and already behaves: a UUID-shaped unknown id is refused 409 `ResourceNotFoundException` with message `Application <leading-segment-of-the-id> not found`, which the check asserts first and which passes.

Emulator: the same call is answered with error code `NotImplemented` (the scaffold's 501 channel) rather than any modeled error. Replay: `AssertionError: expected ValidationException for an empty migrationId, got 'NotImplemented'`. Because GetMigration itself is implemented (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/get_migration.rs resolves the id and answers the not-found refusal correctly), the 501 comes from the request walk ahead of the handler: the empty label leaves the URI matching no bound operation route, and the fallthrough answers NotImplemented.

Why: a client that sends an empty/unset id — the common shape of an uninitialised variable or a propagated empty string — is told the API does not exist instead of being told its input is invalid. 501 is not a modeled error for this operation, so an SDK or client branching on `ValidationException` versus `ResourceNotFoundException` sees neither, and a retry policy that treats 5xx as transient retries a permanently malformed request forever. It also makes the two genuinely different failure classes (malformed input, unknown resource) indistinguishable from an unavailable service.

Fix: the service's REST routing and input-binding layer for this crate — the request walk that binds `@httpLabel` members and the route table it matches against (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/get_migration.rs plus the path that dispatches `/2021-01-01/opensearch/app-migrations[/]`, i.e. the generated router and its NotImplemented fallthrough in crate::scaffolded) — must not fall through to NotImplemented when a required label is empty: it must resolve the trailing-slash collection path to the migrations collection route and raise the modeled ValidationException for the missing required `applicationId`, with AWS's `N validation error(s) detected: Value null at '<member>' failed to satisfy constraint: Member must not be null` wording.

Note: the open issue 20261027T004336Z-opensearch-ten-net-new-control-plane-causes-getmigration.md describes GetMigration/ListMigrations answering 501 for their authorization and feature-gating paths, but it carries neither this check's replay key nor the empty-label/missing-`applicationId` validation rule, and GetMigration's served paths have since been implemented; this cause is the remaining 501 on the empty-label route.

Traces: opensearch.get-migration-cross-account-access-denied-not-not-found.

## Evidence
Replay output (suite simparity-behavioral-aws-v1), check opensearch.get-migration-cross-account-access-denied-not-not-found [mismatch]:

  File "check_opensearch_get_migration_cross_account_access_denied_not_not_found.py", line 43, in run
    assert malformed_code == "ValidationException", (
  AssertionError: expected ValidationException for an empty migrationId, got 'NotImplemented'

Control leg that PASSES in the same check (so the client, credentials and the operation's served path are fine): `get_migration(migrationId=<random uuid4>)` -> 409 ResourceNotFoundException, message `Application <first dash-segment of the uuid> not found`.

Failing leg: `get_migration(migrationId="")`
  Expected (AWS): HTTP 400, code `ValidationException`, message `1 validation error detected: Value null at 'applicationId' failed to satisfy constraint: Member must not be null` — fixed text `1 validation error detected: Value null at '` + `' failed to satisfy constraint: Member must not be null`, variable part the missing required member name (`applicationId`, the required @httpQuery of the collection route the empty label resolves to).
  Actual (emulator): error code `NotImplemented` (the scaffold 501 channel), an error GetMigration does not model.

The check additionally asserts the two codes must differ from each other; with both the not-found and the malformed-input legs required, the emulator currently emits ResourceNotFoundException for the first and NotImplemented for the second, so the distinction exists but the malformed-input class is reported with an unmodeled 501.

## Steps to Reproduce
1. Boot the emulator and point an opensearch client at it.
2. Positive control: `aws opensearch get-migration --migration-id <random uuid4>` -> AWS and emulator both answer 409 ResourceNotFoundException with `Application <leading segment> not found`.
3. Call GetMigration with an EMPTY migrationId, e.g. botocore `client.get_migration(migrationId="")`, which puts the request on `GET /2021-01-01/opensearch/app-migrations/`.
   - AWS: HTTP 400, `ValidationException`, message `1 validation error detected: Value null at 'applicationId' failed to satisfy constraint: Member must not be null`.
   - Emulator: error code `NotImplemented` (501).
4. Re-replay check opensearch.get-migration-cross-account-access-denied-not-not-found to confirm the fix; the check asserts both the code and the exact message, and that the malformed-input code differs from the not-found code.
