Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 00a4e45f686aa5b52058d128f020a4039d01d62a

# Bootstrap the EntityResolution emulator

_Filed: 2026-10-22T00:06:38Z_
_Operation: service-com-amazonaws-entityresolution-awsveniceservice | Class: stub_

> _Note: this issue was identified against the code base at commit `00a4e45f686aa5b52058d128f020a4039d01d62a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 38 operations in `services/service-com-amazonaws-entityresolution-awsveniceservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow can run. Implement a FIRST, SMALL stateful slice: the schema-mapping + matching-workflow lifecycle, which is EntityResolution's primary resource pair (protocol `restJson1`, sigv4 name `entityresolution`).

### Happy path to implement (in order)

1. **CreateSchemaMapping** — `POST /schemas`. Input: `schemaName` (required, len 1..255, `^[a-zA-Z_0-9-]*$`), `description`, `mappedInputFields` (required, list<SchemaInputAttribute>, len 2..60), `tags`. Must persist a new schema mapping and return `schemaName`, a generated `schemaArn` matching `arn:aws:entityresolution:<region>:<account>:schemamapping/<schemaName>`, `description`, `mappedInputFields` — echoed from what was STORED, not from the request object. Re-creating an existing `schemaName` must fail with `ConflictException` (HTTP 400).

2. **CreateMatchingWorkflow** — `POST /matchingworkflows`. Input: `workflowName` (required), `description`, `inputSourceConfig` (required, list<InputSource>, len 1..20), `outputSourceConfig` (required, list<OutputSource>, len 1..1), `resolutionTechniques` (required), `incrementalRunConfig`, `roleArn` (required), `tags`. Must persist the workflow with `createdAt`/`updatedAt` timestamps and return `workflowName`, generated `workflowArn` (`...:matchingworkflow/<workflowName>`), and every config field read back from store. Duplicate `workflowName` → `ConflictException`. A second create must also flip the referenced schema mapping's `hasWorkflows` to true.

3. **GetMatchingWorkflow** — `GET /matchingworkflows/{workflowName}`. Must return the workflow from the store: `workflowName`, `workflowArn`, `description`, `inputSourceConfig`, `outputSourceConfig`, `resolutionTechniques`, `createdAt`, `updatedAt`, `incrementalRunConfig`, `roleArn`, `tags` — byte-for-byte what CreateMatchingWorkflow stored. Unknown name → `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.

4. **ListMatchingWorkflows** — `GET /matchingworkflows`. Returns `workflowSummaries` (list<MatchingWorkflowSummary>) built by iterating the store, plus `nextToken`. Honour `maxResults` and `nextToken` (`@paginated` inputToken=nextToken/outputToken=nextToken/items=workflowSummaries): page size must be respected and the token must resume where the previous page ended; omit `nextToken` on the final page.

5. **GetSchemaMapping** — `GET /schemas/{schemaName}`. Returns `schemaName`, `schemaArn`, `description`, `mappedInputFields`, `createdAt`, `updatedAt`, `tags`, and `hasWorkflows` computed from whether any stored workflow references the schema (not hardcoded false). Unknown name → `ResourceNotFoundException`.

6. **DeleteMatchingWorkflow** / **DeleteSchemaMapping** — `DELETE /matchingworkflows/{workflowName}` and `DELETE /schemas/{schemaName}`, both `@idempotent`. Must remove the entry from the store and return the required `message` field. After deletion, GetMatchingWorkflow/GetSchemaMapping must return `ResourceNotFoundException` and the resource must be gone from the corresponding List. Deleting a schema mapping still referenced by a workflow must fail with `ConflictException` (HTTP 400). Being `@idempotent`, deleting an already-absent resource must not 500.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the input straight back, no generating an ARN/ID without storing the record behind it. A create followed by a get must return what was created; a delete followed by a get must 404.

## State Model

In `src/state.rs`, on `ComAmazonawsEntityresolutionAwsveniceserviceState` (reached via `crate::state::service_state(&*self.engine)`, locked through the guard):

- `schema_mappings: BTreeMap<String /*schemaName*/, SchemaMapping>` where `SchemaMapping { schema_name: String, schema_arn: String, description: Option<String>, mapped_input_fields: Vec<SchemaInputAttribute>, created_at: Timestamp, updated_at: Timestamp, tags: BTreeMap<String,String> }`. `hasWorkflows` is DERIVED at read time from `matching_workflows`, not stored.
- `matching_workflows: BTreeMap<String /*workflowName*/, MatchingWorkflow>` where `MatchingWorkflow { workflow_name: String, workflow_arn: String, description: Option<String>, input_source_config: Vec<InputSource>, output_source_config: Vec<OutputSource>, resolution_techniques: ResolutionTechniques, incremental_run_config: Option<IncrementalRunConfig>, role_arn: String, created_at: Timestamp, updated_at: Timestamp, tags: BTreeMap<String,String> }`.
- Use `BTreeMap` so List ordering is stable and opaque `nextToken` can encode the last-returned key.
- Account id and region for ARN construction come from the request context, so ARNs are consistent across calls for the same resource.

All other operations (ID-mapping workflows, ID namespaces, jobs — StartMatchingJob/GetMatchingJob/ListMatchingJobs, GetMatchId/GenerateMatchId, BatchDeleteUniqueId, provider services, policies, Tag/Untag/ListTagsForResource, the Update* ops) remain 501 for now and are filed as individual gap issues later.

Done = an integration test driving the workflow below passes with correct, stateful bodies — not merely 2xx.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-entityresolution-awsveniceservice/src/ops/` matches all 38 operation files (add_policy_statement.rs … update_schema_mapping.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, so the emulator holds zero state. Contracts cited come from the crate's generated OPERATIONS.md (re-rendered from smithy-model/com-amazonaws-entityresolution-awsveniceservice.json): CreateSchemaMapping `POST /schemas` with required `schemaName`/`mappedInputFields` (len 2..60) and required output `schemaArn` (pattern `^arn:(aws|aws-us-gov|aws-cn):entityresolution:...:(schemamapping/[a-zA-Z_0-9-]{1,255})$`); CreateMatchingWorkflow `POST /matchingworkflows` with required `inputSourceConfig`/`outputSourceConfig`/`resolutionTechniques`/`roleArn` and `ConflictException (HTTP 400)`; GetMatchingWorkflow `GET /matchingworkflows/{workflowName}` `@readonly` with required `createdAt`/`updatedAt` and `ResourceNotFoundException (HTTP 404)`; ListMatchingWorkflows `GET /matchingworkflows` `@paginated(inputToken=nextToken, outputToken=nextToken, items=workflowSummaries)` with `maxResults`; GetSchemaMapping `GET /schemas/{schemaName}` with required `hasWorkflows`; DeleteMatchingWorkflow/DeleteSchemaMapping `@idempotent` DELETE routes with required output `message`. README.md documents the shared-engine state API (`crate::state::service_state`) and warns a fresh `InMemoryEngine` inside a handler silently discards writes. No tests assert any of this — only the scaffold's `tests/state_roundtrip.rs` example exists.

## Steps to Reproduce

1. Start the emulator and point an EntityResolution client (aws CLI / SDK) at it.
2. CreateSchemaMapping with schemaName='customers' and two mappedInputFields (e.g. NAME and EMAIL) — expect 200 with schemaArn ending in 'schemamapping/customers' (today: HTTP 501 NotImplemented).
3. GetSchemaMapping schemaName='customers' — expect the same mappedInputFields back and hasWorkflows=false.
4. CreateMatchingWorkflow with workflowName='wf1', inputSourceConfig referencing the 'customers' schema ARN, one outputSourceConfig, resolutionTechniques and a roleArn — expect 200 with workflowArn ending in 'matchingworkflow/wf1'.
5. GetMatchingWorkflow workflowName='wf1' — expect every field as created, plus createdAt/updatedAt.
6. GetSchemaMapping schemaName='customers' again — expect hasWorkflows=true now that a workflow references it.
7. CreateMatchingWorkflow with workflowName='wf1' again — expect ConflictException (HTTP 400), not a second 200.
8. ListMatchingWorkflows with maxResults=1 after creating 'wf1' and 'wf2' — expect exactly one summary plus a nextToken; pass that nextToken and expect the other workflow and no nextToken.
9. DeleteSchemaMapping schemaName='customers' while 'wf1' exists — expect ConflictException (HTTP 400).
10. DeleteMatchingWorkflow workflowName='wf1' — expect 200 with a message; then GetMatchingWorkflow 'wf1' must return ResourceNotFoundException (HTTP 404) and ListMatchingWorkflows must no longer include it.
11. DeleteSchemaMapping schemaName='customers' now — expect 200; GetSchemaMapping must then 404 and ListSchemaMappings must not include it.
