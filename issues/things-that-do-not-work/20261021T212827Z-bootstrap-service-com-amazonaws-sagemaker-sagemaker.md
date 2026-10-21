Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the SageMaker emulator

_Filed: 2026-10-21T21:28:27Z_
_Operation: service-com-amazonaws-sagemaker-sagemaker | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 409 operations in `service-com-amazonaws-sagemaker-sagemaker` is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, and the service holds no state. Implement a FIRST, SMALL, stateful slice covering the core inference-deployment lifecycle (model → endpoint config → endpoint), the workflow every SageMaker hosting client performs.

Wire protocol (per OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: SageMaker.<OperationName>`, sigv4 signing name `sagemaker`.

### Happy path to implement (in order)

1. **CreateModel** — in: `ModelName` (required), `PrimaryContainer`/`Containers`, `ExecutionRoleArn`, `VpcConfig`, `EnableNetworkIsolation`, `Tags`. Out: `ModelArn` (required) — mint `arn:aws:sagemaker:<region>:<account>:model/<ModelName>`. Persist the whole input plus `CreationTime`. Re-creating an existing name must fail (`ResourceInUse`-style failure as modeled for this family; at minimum it must not silently overwrite and report success).
2. **DescribeModel** — in: `ModelName` (required). Out must be reconstructed from stored state: `ModelName`, `ModelArn`, `PrimaryContainer`, `Containers`, `InferenceExecutionConfig`, `ExecutionRoleArn`, `VpcConfig`, `EnableNetworkIsolation`, `CreationTime` (required). Unknown name → the service's not-found failure (HTTP 400 `ValidationException`-style `ResourceNotFound`), never a 200 with empty fields.
3. **CreateEndpointConfig** — in: `EndpointConfigName` (required), `ProductionVariants` (required, 1..10, each referencing a `ModelName`), `KmsKeyId`, `DataCaptureConfig`, `AsyncInferenceConfig`, `Tags`. Out: `EndpointConfigArn` (required). Store variants verbatim and record `CreationTime`; a variant naming a model that does not exist in state must be rejected, not accepted.
4. **CreateEndpoint** — in: `EndpointName` (required), `EndpointConfigName` (required), `Tags`. Out: `EndpointArn` (required). Must look the config up in state (unknown config → failure), store the endpoint with `EndpointStatus = Creating`, `CreationTime`, `LastModifiedTime`.
5. **DescribeEndpoint** — in: `EndpointName` (required). Out from state: `EndpointName`, `EndpointArn`, `EndpointConfigName`, `EndpointStatus` (required), `ProductionVariants` as `ProductionVariantSummary` derived from the stored config's variants (VariantName, CurrentWeight/DesiredWeight, CurrentInstanceCount/DesiredInstanceCount), `CreationTime`, `LastModifiedTime` (both required). Status must advance `Creating` → `InService` (immediately on the first describe, or after the stored creation timestamp elapses) so a client waiter terminates.
6. **ListModels** — `@paginated(inputToken=NextToken, outputToken=NextToken, items=Models)`. Honor `MaxResults` (1..100), `NextToken`, `NameContains`, `CreationTimeBefore`/`CreationTimeAfter`, and `SortBy` (CreationTime|Name) + `SortOrder` (Ascending|Descending). `Models` (required) entries come from stored models (ModelName, ModelArn, CreationTime). Emit `NextToken` only when a further page exists.
7. **DeleteModel / DeleteEndpoint** — in: the respective name (required). Out: no members. Must actually remove the entry so a subsequent Describe fails with not-found and ListModels no longer shows it.

Every one of these must compute its response from persisted state. No canned, hardcoded or echoed responses: ARNs must be minted deterministically from the stored name and stored, timestamps must be stored at create time and replayed on describe/list, and a random ID without storage is a failure. All other SageMaker operations stay at 501 and will be filed as individual gap issues later.

## State Model

In `ComAmazonawsSagemakerSagemakerState` (`src/state.rs`), reached ONLY via `crate::state::service_state(&*self.engine)`:

- `models: BTreeMap<String /*ModelName*/, Model>` where `Model { name: String, arn: String, primary_container: Option<ContainerDefinition>, containers: Vec<ContainerDefinition>, inference_execution_config: Option<InferenceExecutionConfig>, execution_role_arn: Option<String>, vpc_config: Option<VpcConfig>, enable_network_isolation: Option<bool>, tags: Vec<Tag>, creation_time: SystemTime }`
- `endpoint_configs: BTreeMap<String /*EndpointConfigName*/, EndpointConfig>` where `EndpointConfig { name: String, arn: String, production_variants: Vec<ProductionVariant>, shadow_production_variants: Vec<ProductionVariant>, data_capture_config: Option<DataCaptureConfig>, async_inference_config: Option<AsyncInferenceConfig>, kms_key_id: Option<String>, execution_role_arn: Option<String>, vpc_config: Option<VpcConfig>, enable_network_isolation: Option<bool>, tags: Vec<Tag>, creation_time: SystemTime }`
- `endpoints: BTreeMap<String /*EndpointName*/, Endpoint>` where `Endpoint { name: String, arn: String, config_name: String, status: EndpointStatus, failure_reason: Option<String>, production_variants: Vec<ProductionVariantSummary>, creation_time: SystemTime, last_modified_time: SystemTime, tags: Vec<Tag> }`
- `account_id: String` and `region: String` (or shared engine config) used to mint `arn:aws:sagemaker:<region>:<account>:{model,endpoint-config,endpoint}/<name>`.

BTreeMap keying gives ListModels a stable order to sort/paginate over; pagination tokens encode the last-returned key (or an offset) so a token resumes exactly where the prior page stopped.

## Evidence

OPERATIONS.md (crate root) — `CreateModel` in: `ModelName` required, out: `ModelArn` required (lines 334-338); `DescribeModel` out requires `ModelName`, `CreationTime`, `ModelArn` (1183-1187); `CreateEndpointConfig` in: `EndpointConfigName` + `ProductionVariants` (1..10) required, out `EndpointConfigArn` required (225-229); `CreateEndpoint` in: `EndpointName` + `EndpointConfigName` required, out `EndpointArn` required (219-223); `DescribeEndpoint` out requires `EndpointStatus`/`CreationTime`/`LastModifiedTime` and carries `ProductionVariants` summaries (1055-1059); `ListModels` is `@paginated(inputToken=NextToken, outputToken=NextToken, items=Models)` with `MaxResults` 1..100, `NameContains`, `SortBy`, `SortOrder` (1852-1857); `DeleteModel` (749-753) and `DeleteEndpoint` (646-650) take the name and return no members. README.md documents the scaffold contract: handlers answer 501 until the `SCAFFOLDED, NOT IMPLEMENTED` marker is deleted. Verified stub: `src/ops/create_model.rs` line 4 carries the marker and line 192 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateModel"))`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI SageMaker client at it.
2. `CreateModel --model-name demo-model --primary-container Image=123456789012.dkr.ecr.us-east-1.amazonaws.com/demo:latest --execution-role-arn arn:aws:iam::123456789012:role/SMRole` → expect 200 with a `ModelArn` ending in `model/demo-model` (today: HTTP 501 NotImplemented).
3. `DescribeModel --model-name demo-model` → expect the stored PrimaryContainer, ExecutionRoleArn, ModelArn and a CreationTime matching the create call.
4. `CreateEndpointConfig --endpoint-config-name demo-cfg --production-variants VariantName=AllTraffic,ModelName=demo-model,InitialInstanceCount=1,InstanceType=ml.m5.large` → expect 200 with `EndpointConfigArn`.
5. `CreateEndpoint --endpoint-name demo-ep --endpoint-config-name demo-cfg` → expect 200 with `EndpointArn`.
6. `DescribeEndpoint --endpoint-name demo-ep` → expect `EndpointConfigName=demo-cfg`, a `ProductionVariants` summary with `VariantName=AllTraffic`, and `EndpointStatus` reaching `InService` (so `aws sagemaker wait endpoint-in-service` terminates).
7. `ListModels --max-results 1` with two models created → expect exactly one model plus a `NextToken`; passing that token returns the second and no token. `ListModels --name-contains demo` returns only matching models.
8. `DeleteEndpoint --endpoint-name demo-ep` then `DescribeEndpoint --endpoint-name demo-ep` → expect a not-found failure, not a 200.
9. `DeleteModel --model-name demo-model` then `DescribeModel --model-name demo-model` → expect a not-found failure, and `ListModels` no longer lists it.
10. `CreateEndpoint --endpoint-name other-ep --endpoint-config-name missing-cfg` → expect a failure referencing the missing config, not a success.
