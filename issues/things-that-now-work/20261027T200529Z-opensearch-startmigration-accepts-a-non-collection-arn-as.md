Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.e2e-migration-lifecycle-domain-state-consequence opensearch.concurrent-start-migration-independent-domains
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.e2e-migration-lifecycle-domain-state-consequence|_error#9|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: opensearch.concurrent-start-migration-independent-domains|_outcome#12|AssertionError|raise AssertionError(f"real AWS rejects this StartMigration, but it SUCCEEDED with {result!r}")
Issue-Commit: 4f795fc4410834b03aa4d835ffe99ef72963a92d

# opensearch StartMigration accepts a non-collection ARN as migrationOptions.source.datasourceArn and mints a PENDING migration instead of rejecting it with ValidationException

> _Note: this issue was identified against the code base at commit `4f795fc4410834b03aa4d835ffe99ef72963a92d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMigration

## Description
One net-new root cause.

Cause 1 of 1: StartMigration does not enforce the OpenSearch Serverless collection-ARN constraint on `migrationOptions.source.datasourceArn`. Any syntactically well-formed ARN — including an `arn:aws:es:<region>:<account>:domain/<name>` domain ARN — is accepted, a migration record is minted, and the call answers HTTP 200 with a `migrationId`/`status: PENDING`.
Failing check ids: opensearch.e2e-migration-lifecycle-domain-state-consequence, opensearch.concurrent-start-migration-independent-domains.

AWS: A saved-object migration source must be an OpenSearch Serverless (aoss) collection ARN. The Smithy model does not express this: `MigrationSource.datasourceArn` targets `com.amazonaws.opensearch#ARN`, which is `@length(min: 20, max: 2048)` with `pattern ".*"`, so the shape constraint is length-only. The stricter rule is enforced server-side and is recorded behavior only. `StartMigration` DOES model `ValidationException` (HTTP 400), and the recording shows AWS refusing the call with that error before any migration record is created:
  status 400, code `ValidationException`, message
  `1 validation error detected: Value '<datasourceArn>' at 'migrationOptions.source.datasourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z\-]*:aoss:[a-z0-9\-]+:[0-9]{12}:collection/[a-z0-9]+`
  Message template — fixed text: `1 validation error detected: Value '`, `' at 'migrationOptions.source.datasourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z\-]*:aoss:[a-z0-9\-]+:[0-9]{12}:collection/[a-z0-9]+`. Variable part: the single-quoted value is the rejected `migrationOptions.source.datasourceArn` copied verbatim from the request (the member path and the pattern are fixed text, not per-run values). The refusal is per-request validation, so it is returned independently for each concurrent request and leaves `ListMigrations(applicationId=...)` empty.

Emulator: the call succeeds. Replay shows HTTP 200 with body `{"migrationId": "mig-...", "status": "PENDING"}` for a `datasourceArn` of the form `arn:aws:es:<region>:<account>:domain/<name>`. Under two concurrent StartMigration calls on two distinct applications, each holding an ATTACHED data source, both are accepted the same way. The handler's only check on this member is a generic "is this an ARN at all" test (`is_arn` / `malformed_arn`), which a domain ARN passes, so no collection-specific refusal exists and a migration record is persisted.

Why: a client that relies on AWS refusing a non-collection migration source gets a 200 and a migration id for a migration that can never run. Negative-path code that expects `ValidationException` (and expects the member path and pattern in the message so it can report which field was wrong) never sees it, so operator mistakes — pointing a migration at an `es` domain instead of an `aoss` collection — surface as a phantom PENDING job rather than an up-front error. State is also corrupted relative to AWS: `ListMigrations` for the application reports a migration record that AWS would never have created, so idempotency/quota accounting and any "no migration after a rejected start" invariant diverge.

Fix: the operation's validation, in StartMigration's handler — the `datasourceArn` check must reject any value not matching the aoss collection-ARN pattern with a modeled `ValidationException` carrying the message template above, and must do so BEFORE any migration record is minted or inserted into state, so a refused start leaves no migration. The existing generic ARN-shape refusal stays as the length/shape guard; the collection-ARN rule is an additional, narrower one. Note the only open issue naming this operation is 20261027T004336Z-opensearch-startmigration-stub.md (an unimplemented-stub item) and 20261027T004336Z-opensearch-ten-net-new-control-plane-causes-getmigration.md; neither covers this validation gap — StartMigration now answers 200, it is no longer a stub.

Traces: opensearch.e2e-migration-lifecycle-domain-state-consequence (asserts the exact ValidationException code and message for a domain-shaped ARN; got 200 + mig id), opensearch.concurrent-start-migration-independent-domains (asserts both concurrent starts are refused with ValidationException naming the `datasourceArn`/aoss constraint and that neither application holds a migration record; got 200 + mig id). Handler: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/start_migration.rs lines 203-212 (the `is_arn`/`malformed_arn` guard) and lines 260-284 (record minted and inserted).

## Evidence
Check opensearch.e2e-migration-lifecycle-domain-state-consequence:
  Request: StartMigration(applicationId=<app>, migrationOptions={source:{datasourceArn: "arn:aws:es:<region>:<account>:domain/<name>"}, workspace:{createWorkspace:true, name:<ws>}})
  Expected (AWS, recorded): 400 ValidationException, message `1 validation error detected: Value '<that arn>' at 'migrationOptions.source.datasourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z\-]*:aoss:[a-z0-9\-]+:[0-9]{12}:collection/[a-z0-9]+`; ListMigrations(applicationId) then returns [].
  Actual (emulator): HTTPStatusCode 200, body {'migrationId': 'mig-...', 'status': 'PENDING'}.
  Assertion: `AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'migrationId': 'mig-...', 'status': 'PENDING'}`

Check opensearch.concurrent-start-migration-independent-domains:
  Two applications, each with an ATTACHED data source (attachment status asserted ATTACHED first, so the expected refusal is not a missing-attachment ResourceNotFoundException), each issued StartMigration concurrently with its own domain ARN as source.
  Expected (AWS, recorded): each call independently refused with 400 ValidationException whose message names `datasourceArn` and the `aoss` collection-ARN constraint; ListMigrations for each application returns [].
  Actual (emulator): HTTPStatusCode 200, body {'migrationId': 'mig-...', 'status': 'PENDING'}.
  Assertion: `AssertionError: real AWS rejects this StartMigration, but it SUCCEEDED with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'migrationId': 'mig-...', 'status': 'PENDING'}`

Model evidence (smithy_query operation=StartMigration, closure):
  MigrationSource.datasourceArn -> ARN, required, length {min:20,max:2048}, pattern ".*"  (no collection-ARN pattern in the model; collection rule is recorded behavior only)
  StartMigration errors include ValidationException (errorKind client, httpError 400)

Emulator source evidence: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/start_migration.rs:206 — `if !is_arn(&options.source.datasource_arn) { ... malformed_arn(...) }` is the ONLY validation applied to this member; a domain ARN passes it, then lines 260-284 mint and insert the Migration and answer 200.

## Steps to Reproduce
1. CreateApplication(name=<app-name>) and poll GetApplication until status == ACTIVE; note its id.
2. Call StartMigration with a source ARN that is a well-formed ARN but NOT an aoss collection ARN, e.g.:
   StartMigration(applicationId=<app-id>, migrationOptions={"source": {"datasourceArn": "arn:aws:es:<region>:<account-id>:domain/<some-domain-name>"}, "workspace": {"createWorkspace": true, "name": "<ws-name>"}})
3. Observe HTTP 200 with {"migrationId": "mig-...", "status": "PENDING"}. AWS instead answers HTTP 400 ValidationException with the message `1 validation error detected: Value '<the arn you sent>' at 'migrationOptions.source.datasourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z\-]*:aoss:[a-z0-9\-]+:[0-9]{12}:collection/[a-z0-9]+`.
4. Call ListMigrations(applicationId=<app-id>). The emulator reports the migration record it minted; AWS reports [] because the start was refused before any record was persisted.
5. Concurrency variant: create two applications, attach a data source to each and wait for ATTACHED, then issue the two StartMigration calls above simultaneously (one per application). Both are accepted with 200 in the emulator; AWS refuses each independently with the ValidationException above and leaves both applications with no migration record.
