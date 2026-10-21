Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Application Discovery Service emulator

_Filed: 2026-10-21T21:18:21Z_
_Operation: service-com-amazonaws-applicationdiscoveryservice-awsposeidonservice-v2015-11-01 | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 28 handlers under `src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state at all. No client workflow works today.

Implement a FIRST SLICE: the application-configuration lifecycle, which is the primary resource of Application Discovery Service (ADS) and the one the Migration Hub console/CLI exercises. Protocol is `awsJson1_1`, routed as `POST /` with `X-Amz-Target: AWSPoseidonService_V2015_11_01.<OperationName>` (see README.md / OPERATIONS.md 'Wire protocol').

Happy path, in order (all fields per OPERATIONS.md):

1. **CreateApplication** — input `name` (required, len 0..127, pattern `^[\s\S]*\S[\s\S]*$`), `description` (len 0..1000), `wave` (len 0..256). Must MINT and PERSIST a new application record and return its generated `configurationId` (e.g. `d-application-<random>`). The id returned must be the key later ops resolve; do not echo the input or return a fixed string.
2. **ListConfigurations** — input `configurationType` (required, one of `APPLICATION|CONNECTION|PROCESS|SERVER`), `filters`, `maxResults`, `nextToken`, `orderBy`. For `configurationType=APPLICATION` it must return one `configurations` map per persisted application, built from stored state with the ADS flat key naming (`application.configurationId`, `application.name`, `application.description`, plus `application.serverCount` when associations exist). An application created in step 1 MUST appear here; a deleted one MUST NOT. Honour `maxResults` by truncating and emitting an opaque `nextToken` that resumes where the page ended (and omitting `nextToken` on the last page).
3. **DescribeConfigurations** — input `configurationIds` (required). Must return, for each requested id that exists, a `configurations` map of that application's stored attributes (same flat keys as above). Unknown ids are simply absent from the result (ADS does not fail the whole call) — the returned values must come from state, never be fabricated.
4. **UpdateApplication** — input `configurationId` (required), `name`, `description`, `wave`. Must mutate the stored record in place (only the members supplied), returning an empty body; a following DescribeConfigurations/ListConfigurations must observe the new values. An unknown `configurationId` must raise a modeled failure (`InvalidParameterValueException`, HTTP 400), not a silent success.
5. **DeleteApplications** — input `configurationIds` (required). Must remove the named applications from state. After this, DescribeConfigurations returns no entry for them and ListConfigurations(APPLICATION) no longer lists them. Repeating the call is idempotent (no error for already-gone ids).
6. **GetDiscoverySummary** — no input. Must COMPUTE `applications` from the number of persisted applications (and `servers`, `serversMappedToApplications`, `serversMappedtoTags` from the respective stores, 0 while empty). This is the proof that nothing is canned: create two applications and the count must read 2; delete one and it must read 1.

NEVER STUBS: each of the six must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (see README 'State'), never from hardcoded values, echoed input, or a random id that is not stored. Validation that is cheap and spec-grounded should be applied: `@required` members present, `name`/`configurationId` length+pattern per the model, and `configurationType` restricted to the `ConfigurationItemType` enum — violations as `InvalidParameterValueException`/`InvalidParameterException` (HTTP 400), not 501 and not 200.

## State Model

In `src/state.rs`, on the service state struct:

- `applications: BTreeMap<String /* configurationId */, Application>` — insertion-ordered-by-key registry of applications.
  - `Application { configuration_id: String, name: String, description: Option<String>, wave: Option<String>, created_time: SystemTime, last_modified_time: SystemTime, associated_configuration_ids: BTreeSet<String> }`
- `next_application_seq: u64` — monotonic counter used to mint `d-application-<hex/seq>` ids so they are unique and reproducible.
- `list_tokens: HashMap<String /* nextToken */, ListCursor>` with `ListCursor { configuration_type: ConfigurationItemType, offset: usize }` — backs ListConfigurations pagination (an opaque token, not a client-supplied offset).
- Reserved for the next slices (left empty by this one, but declared so later ops share the store): `tags: BTreeMap<String /* configurationId */, BTreeSet<(String /* key */, String /* value */)>>`, `agents: BTreeMap<String /* agentId */, AgentInfo>`, `import_tasks: BTreeMap<String /* importTaskId */, ImportTask>`, `export_tasks: BTreeMap<String /* exportId */, ExportInfo>`, `continuous_exports: BTreeMap<String /* exportId */, ContinuousExportDescription>`, `batch_delete_tasks: BTreeMap<String /* taskId */, BatchDeleteConfigurationTask>`.

An `Application` flattens to a ListConfigurations/DescribeConfigurations map through one shared helper so both ops report identical attribute keys and values.

Out of scope for this first slice (they stay 501 and are filed separately later): CreateTags/DeleteTags/DescribeTags, Associate/DisassociateConfigurationItems*, DescribeAgents, Start/StopDataCollectionByAgentIds, all export operations (StartExportTask, DescribeExportTasks, Start/StopContinuousExport, DescribeContinuousExports, DescribeExportConfigurations, ExportConfigurations), import operations (StartImportTask, DescribeImportTasks, BatchDeleteImportData), BatchDeleteAgents, Start/DescribeBatchDeleteConfigurationTask and ListServerNeighbors.

## Evidence

OPERATIONS.md (crate root) documents all 28 operations with their members: CreateApplication (in: name/description/wave; out: configurationId), ListConfigurations (in: configurationType[APPLICATION|CONNECTION|PROCESS|SERVER], filters, maxResults, nextToken, orderBy; out: configurations list<map<string,string>>, nextToken; @paginated items=configurations), DescribeConfigurations (in: configurationIds; out: configurations), UpdateApplication (in: configurationId/name/description/wave; out: no members), DeleteApplications (in: configurationIds; out: no members), GetDiscoverySummary (out: servers/applications/serversMappedToApplications/serversMappedtoTags required longs). Error shapes available per op include InvalidParameterException and InvalidParameterValueException (HTTP 400). README.md confirms awsJson1_1 with X-Amz-Target `AWSPoseidonService_V2015_11_01.<Op>` and documents the scaffold contract. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 28 operation files (create_application.rs:4, list_configurations.rs:4, describe_configurations.rs:4, update_application.rs:4, delete_applications.rs:4, get_discovery_summary.rs:4, ...), proving the whole service answers 501 with zero persisted state. No tests exist beyond the generated tests/state_roundtrip.rs example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `discovery` client at it.
2. CreateApplication with name='migration-wave-1', description='first wave' -> expect 200 with a non-empty, newly minted configurationId (capture it as APP1).
3. CreateApplication with name='migration-wave-2' -> expect 200 with a DIFFERENT configurationId (APP2).
4. ListConfigurations with configurationType=APPLICATION -> expect both APP1 and APP2 present, each map carrying application.configurationId and application.name matching what was created.
5. ListConfigurations with configurationType=APPLICATION, maxResults=1 -> expect exactly 1 configuration plus a nextToken; call again with that nextToken -> expect the OTHER application and no nextToken.
6. DescribeConfigurations with configurationIds=[APP1] -> expect one map whose application.name is 'migration-wave-1' and application.description is 'first wave'.
7. UpdateApplication with configurationId=APP1, name='migration-wave-1b' -> expect 200 empty body; re-run DescribeConfigurations([APP1]) -> expect application.name == 'migration-wave-1b' and description unchanged.
8. GetDiscoverySummary -> expect applications == 2.
9. DeleteApplications with configurationIds=[APP1] -> expect 200; DescribeConfigurations([APP1]) -> expect no entry for APP1; ListConfigurations(APPLICATION) -> expect only APP2; GetDiscoverySummary -> expect applications == 1.
10. UpdateApplication with configurationId='d-application-doesnotexist' -> expect InvalidParameterValueException (HTTP 400), not 200 and not 501.
11. ListConfigurations with configurationType omitted -> expect a 400 validation error naming configurationType.
