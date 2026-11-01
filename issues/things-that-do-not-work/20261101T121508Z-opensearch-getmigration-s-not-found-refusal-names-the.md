Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.e2e-migration-lifecycle-domain-state-consequence
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.e2e-migration-lifecycle-domain-state-consequence|run#17|AssertionError|assert got_get == ("ResourceNotFoundException", "Application mig not found"), (
Issue-Commit: 1089a3896fb85320e89317bd5e056d5ee3057dc5

# opensearch GetMigration's not-found refusal names the migration instead of the application: emulator-authored "Migration not found: <id>" where AWS answers "Application <app-id> not found"

> _Note: this issue was identified against the code base at commit `1089a3896fb85320e89317bd5e056d5ee3057dc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMigration

## Description
One net-new root cause in the OpenSearch Service control plane, grounded in the single surviving failing check of the `opensearch` service (every other opensearch failure in this run was marked ALREADY FILED or OWNED by an open issue and was skipped).

=== Cause 1 of 1: GetMigration's ResourceNotFoundException for an unknown migrationId carries emulator-authored wording and names the wrong resource ===

AWS: Smithy: `GetMigration` is `GET /2021-01-01/opensearch/app-migrations/{migrationId}` with required `migrationId` (httpLabel, String) and models AccessDeniedException (HTTP 403), DisabledOperationException (409), InternalException (500), ResourceNotFoundException (409) and ValidationException (400). The model documents ResourceNotFoundException only as "An exception for accessing or deleting a resource that doesn't exist" and states nothing about its message, so the message is recorded behavior only. The recording shows that a well-formed `migrationId` for which no migration job exists is refused with ResourceNotFoundException whose message names the APPLICATION, not the migration: its fixed text is `Application ` and ` not found`, and its one variable part is the application identifier the service resolves out of the supplied `migrationId` — in the recording the segment of the migrationId before its first `-` (migrationId `mig-<suffix>` produced `Application mig not found`). Hypothesis: AWS's migrationId encodes its owning applicationId as that leading segment, so resolving an unknown job fails at the application lookup and is reported as the missing application. The code (ResourceNotFoundException) and status (409) that the recording shows are the requirement regardless; only the message diverges here.

Emulator: answers ResourceNotFoundException with the emulator's own wording `Migration not found: <migrationId>` — it echoes the whole requested migrationId and names the migration as the absent resource. In the replay the bogus id `mig-<suffix>` produced `('ResourceNotFoundException', 'Migration not found: mig-<suffix>')` where AWS produces `('ResourceNotFoundException', 'Application mig not found')`. The handler builds this message unconditionally from the request's `migration_id` (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/get_migration.rs line 171-177, via `crate::state::migration_not_found`), with no application resolution step at all.

Why: a client relying on AWS's behavior is told the wrong thing about which resource is missing. AWS's refusal says the APPLICATION the id belongs to could not be resolved, which is the signal a caller uses to decide whether to re-create/await the application versus re-poll a job id; the emulator's wording says the job is absent while implying the application was found. Any client, runbook or test that matches the documented refusal text — or that parses the resource name out of it for triage/alerting — diverges, and a caller cannot reproduce AWS's diagnosis from the emulator.

Fix: the not-found path of the GetMigration handler and the shared message helper it calls — services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/get_migration.rs (the `ok_or_else` on `settled_migration_mut`) and `migration_not_found` in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/state.rs. The handler must resolve the owning application for the requested migrationId first and raise ResourceNotFoundException (HTTP 409) worded `Application <application-id> not found`, with the application id taken from the request's migrationId (and from the emulator's own migration-id minting scheme, so a minted id and a refused id agree), never the full migrationId. Note: the open issue 20261027T004336Z-opensearch-ten-net-new-control-plane-causes-getmigration.md (cause 1) covers GetMigration answering 501 NotImplemented and its AccessDenied-before-not-found ordering; the operation is now served and refuses correctly-coded-but-wrongly-worded, and that issue carries neither this replay key nor this message requirement. No fixed issue claims this operation.

Traces: opensearch.e2e-migration-lifecycle-domain-state-consequence.

## Evidence
Replay output (suite simparity-behavioral-aws-v1), check opensearch.e2e-migration-lifecycle-domain-state-consequence [mismatch]:

  File "check_opensearch_e2e_migration_lifecycle_domain_state_consequence.py", line 68, in run
    assert got_get == ("ResourceNotFoundException", "Application mig not found"), (
  AssertionError: unexpected refusal for a bogus migrationId: ('ResourceNotFoundException', 'Migration not found: mig-x561766f8eee0')

Expected (AWS): HTTP 409, Error.Code `ResourceNotFoundException`, Error.Message `Application <application-id> not found` — fixed text `Application ` + ` not found`, variable part the application id resolved from the request's migrationId (the segment before its first `-`; `mig` for a `mig-<suffix>` id).
Actual (emulator): Error.Code `ResourceNotFoundException` with Error.Message `Migration not found: <migrationId>` — the full requested id echoed and the migration named as the missing resource. (`mig-x561766f8eee0` is the per-run generated id of this recording and is not part of the requirement.)

The check's earlier legs pass against the emulator: StartMigration with a domain-shaped ARN as `migrationOptions.source.datasourceArn` is already refused with the modeled ValidationException and its pattern message, ListMigrations is empty afterwards, and AttachDataSource accepts the same ARN — so this check fails on the GetMigration refusal message alone, not on a chain of earlier defects.

## Steps to Reproduce
1. Boot the emulator and point an `opensearch` client at it.
2. CreateApplication(name=<unique>) and poll GetApplication until `status == "ACTIVE"`.
3. Call GetMigration(migrationId="mig-<random-suffix>") — an id of the documented shape for which no migration job was ever started.
4. AWS: 409 ResourceNotFoundException with message `Application mig not found` (fixed text `Application ` / ` not found`; the variable part is the application id resolved from the migrationId, i.e. the segment before the first `-`).
   Emulator: 409 ResourceNotFoundException with message `Migration not found: mig-<random-suffix>`.
5. Replay check opensearch.e2e-migration-lifecycle-domain-state-consequence to confirm the fix.
