Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the service-com-amazonaws-appflow-sandstoneconfigurationservicelambda emulator

## Resolution

Implemented: the flow lifecycle slice this report prescribes — `CreateFlow`,
`DescribeFlow`, `ListFlows`, `StartFlow`, `DescribeFlowExecutionRecords` and
`DeleteFlow` — now computes every response from the flow store held by the shared
engine (`crate::state::service_state`), with minted ARNs, stored `executionId`s, real
pagination cursors and the modelled `ConflictException` / `ResourceNotFoundException` /
`ValidationException` / `ServiceQuotaExceededException` answers. Covered end to end by
`tests/all/flow_lifecycle.rs`. The 19 operations the report puts out of scope are
untouched and still answer HTTP 501 until their own slices land.

_Filed: 2026-10-21T21:18:40Z_
_Operation: service-com-amazonaws-appflow-sandstoneconfigurationservicelambda | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 handlers under `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST SMALL SLICE: the stateful flow lifecycle of AppFlow's primary resource (a Flow), wired through the shared engine store (`crate::state::service_state(&*self.engine)`), on the `restJson1` protocol (SDK id `Appflow`, signing name `appflow`).

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateFlow** — `POST /create-flow`. Input: `flowName` (required, pattern `^[a-zA-Z0-9][\w!@#.-]+$`), `description`, `kmsArn`, `triggerConfig` (required), `sourceFlowConfig` (required), `destinationFlowConfigList` (required), `tasks` (required), `tags`, `metadataCatalogConfig`, `clientToken`. Store the whole flow definition under `flowName`; mint `flowArn` = `arn:aws:appflow:<region>:<account>:flow/<flowName>`; return `flowArn` + `flowStatus` (`Draft` for an OnDemand trigger, `Active` for Scheduled/Event). Re-creating an existing `flowName` must fail `ConflictException` (409).
2. **DescribeFlow** — `POST /describe-flow`. Input: `flowName` (required). Must return the PERSISTED record, not an echo: `flowArn`, `flowName`, `description`, `kmsArn`, `flowStatus`, `sourceFlowConfig`, `destinationFlowConfigList`, `triggerConfig`, `tasks`, `tags`, `metadataCatalogConfig`, `createdAt`, `lastUpdatedAt`, `createdBy`, `lastUpdatedBy`, and `lastRunExecutionDetails` once a run has happened. Unknown `flowName` ⇒ `ResourceNotFoundException` (404).
3. **ListFlows** — `POST /list-flows`. Input: `maxResults` (1..100), `nextToken`. Return `flows` as `FlowDefinition` entries built from the stored flows (`flowArn`, `flowName`, `description`, `flowStatus`, `sourceConnectorType`/`destinationConnectorType` derived from the stored configs, `triggerType`, `createdAt`, `lastUpdatedAt`, `tags`), honouring `maxResults` and emitting/consuming a real opaque `nextToken` cursor.
4. **StartFlow** — `POST /start-flow`. Input: `flowName` (required), `clientToken`. Must append a new execution record to the stored flow, returning `flowArn`, `flowStatus` and a freshly generated `executionId` that is retrievable afterwards. Unknown flow ⇒ 404; starting an already-running scheduled flow ⇒ `ConflictException` (409).
5. **DescribeFlowExecutionRecords** — `POST /describe-flow-execution-records`. Input: `flowName` (required), `maxResults`, `nextToken`. Return the `flowExecutions` (`ExecutionRecord`) list recorded by StartFlow — the `executionId` returned in step 4 MUST appear, with `executionStatus`, `startedAt`/`lastUpdatedAt` and `executionResult` — plus `nextToken` paging. Unknown flow ⇒ 404.
6. **DeleteFlow** — `POST /delete-flow`. Input: `flowName` (required), `forceDelete` (default false). Remove the flow from the store; a subsequent DescribeFlow must return `ResourceNotFoundException` (404) and ListFlows must no longer include it. Deleting an unknown flow ⇒ 404; deleting a running flow with `forceDelete=false` ⇒ `ConflictException` (409).

NEVER STUBS: each of these six must compute its response from the persisted store. Hardcoded ARNs, echoed inputs, or a random `executionId` that is never stored are failures — DescribeFlow must return exactly what CreateFlow stored, and DescribeFlowExecutionRecords must return exactly the executions StartFlow created.

## State Model

In `src/state.rs`, on the service state struct held by the shared engine:

- `flows: BTreeMap<String /* flowName */, Flow>`
- `Flow { flow_arn: String, flow_name: String, description: Option<String>, kms_arn: Option<String>, flow_status: FlowStatus, flow_status_message: Option<String>, trigger_config: TriggerConfig, source_flow_config: SourceFlowConfig, destination_flow_config_list: Vec<DestinationFlowConfig>, tasks: Vec<Task>, tags: BTreeMap<String,String>, metadata_catalog_config: Option<MetadataCatalogConfig>, created_at: SystemTime, last_updated_at: SystemTime, created_by: String, last_updated_by: String, schema_version: i64, executions: Vec<ExecutionRecord>, last_run_execution_details: Option<ExecutionDetails> }`
- `ExecutionRecord { execution_id: String, execution_status: ExecutionStatus, execution_result: Option<ExecutionResult>, started_at: SystemTime, last_updated_at: SystemTime, data_pull_start_time: Option<SystemTime>, data_pull_end_time: Option<SystemTime> }`
- `client_tokens: BTreeMap<String /* clientToken */, String /* flowName|executionId */>` so repeated CreateFlow/StartFlow with the same `@idempotencyToken` return the first result instead of a conflict.
- Reserve `connector_profiles: BTreeMap<String, ConnectorProfile>` and `registered_connectors: BTreeMap<String /* connectorLabel */, ConnectorDetail>` for the next slice (CreateConnectorProfile / DescribeConnectorProfiles / RegisterConnector), but leave those operations stubbed for now.

Out of scope for this first slice (stay 501, filed separately later): CreateConnectorProfile, UpdateConnectorProfile, DeleteConnectorProfile, DescribeConnectorProfiles, RegisterConnector, UnregisterConnector, UpdateConnectorRegistration, DescribeConnector, DescribeConnectors, ListConnectors, ListConnectorEntities, DescribeConnectorEntity, ResetConnectorMetadataCache, StopFlow, UpdateFlow, CancelFlowExecutions, TagResource, UntagResource, ListTagsForResource.

## Evidence

OPERATIONS.md (crate root) documents all 25 operations, their routes and members: CreateFlow `POST /create-flow` → `flowArn`/`flowStatus`; DescribeFlow `POST /describe-flow` → full persisted flow fields with `ResourceNotFoundException (HTTP 404)`; ListFlows `POST /list-flows` @paginated(inputToken=nextToken, outputToken=nextToken) → `flows` (FlowDefinition); StartFlow `POST /start-flow` → `flowArn`/`flowStatus`/`executionId`; DescribeFlowExecutionRecords `POST /describe-flow-execution-records` @paginated → `flowExecutions` (ExecutionRecord); DeleteFlow `POST /delete-flow` with `forceDelete` (default false) and `ConflictException (HTTP 409)` / `ResourceNotFoundException (HTTP 404)`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 25 handler files (create_flow.rs, describe_flow.rs, list_flows.rs, start_flow.rs, describe_flow_execution_records.rs, delete_flow.rs, …), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` ⇒ HTTP 501. README.md confirms the scaffold answers 501 until `handle` is written and that state must be reached via `self.engine`. No integration tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AppFlow client at it (restJson1, signing name `appflow`).
2. CreateFlow with flowName=`demo-flow`, an S3 sourceFlowConfig, one S3 destinationFlowConfig, one Map task and triggerConfig={triggerType: OnDemand} — expect 200 with a `flowArn` ending in `demo-flow` and a `flowStatus`; today 501 NotImplemented.
3. DescribeFlow flowName=`demo-flow` — expect 200 whose `tasks`, `sourceFlowConfig`, `destinationFlowConfigList`, `triggerConfig` and `tags` equal what was created, plus `createdAt`/`lastUpdatedAt`.
4. ListFlows with maxResults=1 — expect `demo-flow` in `flows` with its derived connector types and a usable `nextToken` when more flows exist.
5. StartFlow flowName=`demo-flow` — expect 200 with a non-empty `executionId`.
6. DescribeFlowExecutionRecords flowName=`demo-flow` — expect the `executionId` from the previous step present in `flowExecutions` with an `executionStatus`.
7. DeleteFlow flowName=`demo-flow` — expect 200; then DescribeFlow flowName=`demo-flow` must return ResourceNotFoundException (404) and ListFlows must not list it.
8. DescribeFlow on a never-created name — expect ResourceNotFoundException (404), not 501 and not an empty 200.
