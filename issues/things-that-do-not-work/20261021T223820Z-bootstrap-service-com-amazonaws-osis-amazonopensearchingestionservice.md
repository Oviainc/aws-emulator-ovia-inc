Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 543596051071af06ccfbb58d47ce453a1db8e676

# Bootstrap the AmazonOpenSearchIngestionService (osis) emulator

_Filed: 2026-10-21T22:38:20Z_
_Operation: service-com-amazonaws-osis-amazonopensearchingestionservice | Class: missing_

> _Note: this issue was identified against the code base at commit `543596051071af06ccfbb58d47ce453a1db8e676`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 22 operations in `services/service-com-amazonaws-osis-amazonopensearchingestionservice/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and the service holds no state. Nothing works end to end.

This issue scopes the FIRST buildable slice: the pipeline lifecycle, the primary resource of OSIS. Implement exactly these six operations statefully (restJson1, routes as modeled under `/2022-01-01/osis/...`):

1. **CreatePipeline** — `POST /2022-01-01/osis/createPipeline`. Required input: `PipelineName` (3..28, `^[a-z][a-z0-9\-]+$`), `MinUnits` (>=1), `MaxUnits` (>=1), `PipelineConfigurationBody` (1..100000). Optional: `LogPublishingOptions`, `VpcOptions`, `BufferOptions`, `EncryptionAtRestOptions`, `Tags`, `PipelineRoleArn`. Must persist a new pipeline record and return `Pipeline` with the stored `PipelineName`, a generated `PipelineArn` of the form `arn:aws:osis:<region>:<account>:pipeline/<PipelineName>`, the stored `MinUnits`/`MaxUnits`/`PipelineConfigurationBody`/`Tags`/`PipelineRoleArn`, `Status` = `CREATING` (then settling to `ACTIVE`), `CreatedAt`/`LastUpdatedAt` timestamps, and `IngestEndpointUrls` derived from the pipeline name. A second CreatePipeline with the same name must fail `ResourceAlreadyExistsException` (HTTP 409); a name violating the length/pattern constraint must fail `ValidationException` (HTTP 400).
2. **GetPipeline** — `GET /2022-01-01/osis/getPipeline/{PipelineName}`. Must return the `Pipeline` exactly as persisted by Create/Update (not an echo of the request), and `ResourceNotFoundException` (HTTP 404) for an unknown name.
3. **ListPipelines** — `GET /2022-01-01/osis/listPipelines`. Must return one `PipelineSummary` per persisted pipeline (`PipelineName`, `PipelineArn`, `Status`, `MinUnits`, `MaxUnits`, `CreatedAt`, `LastUpdatedAt`, `Tags`) and honour the `@paginated` contract: slice by `MaxResults` (1..100) and return/accept `NextToken`, returning `InvalidPaginationTokenException` (HTTP 400) for a token it did not issue.
4. **StopPipeline** — `PUT /2022-01-01/osis/stopPipeline/{PipelineName}`. Must transition a persisted `ACTIVE` pipeline to `STOPPING`/`STOPPED`, make that status visible to a later GetPipeline/ListPipelines, return the updated `Pipeline`, and return `ConflictException` (HTTP 409) when the pipeline is not in a stoppable state, `ResourceNotFoundException` for unknown names.
5. **StartPipeline** — `PUT /2022-01-01/osis/startPipeline/{PipelineName}`. Inverse of Stop: `STOPPED` -> `STARTING`/`ACTIVE`, persisted and visible to later reads; `ConflictException` when already active, `ResourceNotFoundException` for unknown names.
6. **DeletePipeline** — `DELETE /2022-01-01/osis/deletePipeline/{PipelineName}`. Must remove the record so a subsequent GetPipeline returns `ResourceNotFoundException` (404) and ListPipelines no longer includes it; a second delete of the same name returns `ResourceNotFoundException`.

NEVER STUBS: every one of these six must compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)`). Hardcoded `Pipeline` bodies, echoing the input back without storing it, or generating an ARN without a stored record are failures of this issue. Create followed by Get/List must return what was created; Stop/Start must change what Get returns; Delete must make it disappear.

## State Model

In `src/state.rs`, on the service state struct:

- `pipelines: BTreeMap<String /* PipelineName */, Pipeline>` — ordered by name so ListPipelines paginates deterministically.
- `Pipeline { pipeline_name: String, pipeline_arn: String, min_units: i32, max_units: i32, pipeline_configuration_body: String, status: PipelineStatus /* CREATING | ACTIVE | STOPPING | STOPPED | STARTING | UPDATING | DELETING | CREATE_FAILED ... */, status_reason: Option<String>, created_at: SystemTime, last_updated_at: SystemTime, ingest_endpoint_urls: Vec<String>, log_publishing_options: Option<LogPublishingOptions>, vpc_endpoints: Vec<VpcEndpoint>, buffer_options: Option<BufferOptions>, encryption_at_rest_options: Option<EncryptionAtRestOptions>, pipeline_role_arn: Option<String>, tags: Vec<Tag> }`
- `list_tokens: BTreeMap<String /* NextToken */, usize /* offset into the ordered pipeline keys */>` — so ListPipelines can issue and validate its own pagination tokens.

ARN construction should use the request's region/account from the operation context so `PipelineArn` is stable and reusable as the `Arn`/`ResourceArn` key by the later tagging and resource-policy operations.

The remaining 16 operations (CreatePipelineEndpoint, DeletePipelineEndpoint, ListPipelineEndpoints, ListPipelineEndpointConnections, RevokePipelineEndpointConnections, Put/Get/DeleteResourcePolicy, Tag/Untag/ListTagsForResource, UpdatePipeline, ValidatePipeline, GetPipelineBlueprint, ListPipelineBlueprints, GetPipelineChangeProgress) stay at 501 and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-osis-amazonopensearchingestionservice/OPERATIONS.md` lists all 22 operations with their routes, required inputs, outputs and modeled errors (CreatePipeline route/inputs lines 26-31; DeletePipeline 40-45; GetPipeline 63-68; ListPipelines with @paginated 114-120; StartPipeline 145-150; StopPipeline 152-157). The `Pipeline` and `PipelineSummary` output shapes (fields cited above) come from the Smithy model at `smithy-model/com-amazonaws-osis-amazonopensearchingestionservice.json`. A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` under `src/ops/` matches all 22 handler files, confirming nothing is implemented and no test asserts any behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `osis` client at it.
2. `aws osis create-pipeline --pipeline-name my-pipeline --min-units 1 --max-units 2 --pipeline-configuration-body "$(cat pipeline.yaml)"` — expect HTTP 200 with a `Pipeline` containing PipelineName=my-pipeline, a pipeline ARN, MinUnits/MaxUnits as sent, Status CREATING/ACTIVE and CreatedAt; actual: HTTP 501 NotImplemented.
3. `aws osis get-pipeline --pipeline-name my-pipeline` — expect the same persisted Pipeline (same ARN, same configuration body); actual: 501.
4. `aws osis list-pipelines --max-results 1` — expect a PipelineSummary for my-pipeline and a NextToken only when more pipelines exist; actual: 501.
5. `aws osis stop-pipeline --pipeline-name my-pipeline` then `get-pipeline` — expect Status STOPPING/STOPPED to be visible on the read; actual: 501.
6. `aws osis start-pipeline --pipeline-name my-pipeline` then `get-pipeline` — expect Status back to STARTING/ACTIVE; actual: 501.
7. `aws osis delete-pipeline --pipeline-name my-pipeline`, then `get-pipeline` — expect ResourceNotFoundException (HTTP 404) and the pipeline absent from list-pipelines; actual: 501.
8. `aws osis create-pipeline --pipeline-name my-pipeline ...` twice — expect the second call to fail ResourceAlreadyExistsException (HTTP 409); actual: 501.
