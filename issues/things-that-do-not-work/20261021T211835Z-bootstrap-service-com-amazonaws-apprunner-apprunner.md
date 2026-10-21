Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the AppRunner emulator

_Filed: 2026-10-21T21:18:35Z_
_Operation: service-com-amazonaws-apprunner-apprunner | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 37 handlers under `services/service-com-amazonaws-apprunner-apprunner/src/ops/` is an unmodified scaffold carrying `//! SCAFFOLDED, NOT IMPLEMENTED:` and answering HTTP 501, so no client workflow works at all. Implement a FIRST, SMALL stateful slice: the App Runner *service* resource lifecycle.

Happy path (in order), protocol `awsJson1_0`, `POST /` with `X-Amz-Target: AppRunner.<Operation>`:

1. **CreateService** — in: `ServiceName` (required, len 4..40, pattern `^[A-Za-z0-9][A-Za-z0-9-_]{3,39}$`), `SourceConfiguration` (required), optional `InstanceConfiguration`, `HealthCheckConfiguration`, `NetworkConfiguration`, `ObservabilityConfiguration`, `EncryptionConfiguration`, `AutoScalingConfigurationArn`, `Tags`. Out: `Service` (required) + `OperationId` (required, UUID len 36). Must mint and PERSIST a new service record: `ServiceId` (32-char UUID-form id), `ServiceArn` = `arn:aws:apprunner:<region>:<account>:service/<ServiceName>/<ServiceId>`, `ServiceUrl` = `<ServiceId>.<region>.awsapprunner.com`, `CreatedAt`/`UpdatedAt` timestamps, `Status` (`OPERATION_IN_PROGRESS` then `RUNNING`), and echo back the stored (not merely echoed-from-request) `SourceConfiguration`, `InstanceConfiguration` with applied defaults, `AutoScalingConfigurationSummary`, `NetworkConfiguration`. Also record a CREATE_SERVICE operation whose `Id` is the returned `OperationId`.
2. **DescribeService** — in: `ServiceArn` (required). Out: `Service` (required). Must look the service up by ARN in the store and return the SAME record that CreateService persisted; unknown ARN -> `ResourceNotFoundException` (HTTP 400).
3. **ListServices** — in: `NextToken`, `MaxResults` (1..20). Out: `ServiceSummaryList` (required) + `NextToken`. Must enumerate the persisted services as `ServiceSummary{ServiceName, ServiceId, ServiceArn, ServiceUrl, CreatedAt, UpdatedAt, Status}`, honour `MaxResults` and return/consume a real `NextToken` (deleted services excluded from the live listing once reaped, per DeleteService below).
4. **UpdateService** — in: `ServiceArn` (required) plus any of `SourceConfiguration`, `InstanceConfiguration`, `AutoScalingConfigurationArn`, `HealthCheckConfiguration`, `NetworkConfiguration`, `ObservabilityConfiguration`. Out: `Service` (required) + `OperationId` (required). Must MERGE the supplied fields into the stored record, bump `UpdatedAt`, and a subsequent DescribeService must show the updated values. Unknown ARN -> `ResourceNotFoundException`.
5. **DeleteService** — in: `ServiceArn` (required). Out: `Service` (required) + `OperationId` (required). Must transition the stored record to `DELETED` with `DeletedAt` set and remove it from `ListServices`; a second DeleteService on the same ARN -> `ResourceNotFoundException` (or `InvalidStateException` while a delete is in flight).
6. **ListOperations** — in: `ServiceArn` (required), `MaxResults` (1..20), `NextToken`. Out: `OperationSummaryList` + `NextToken`. Must return the real operation log recorded by steps 1/4/5 as `OperationSummary{Id, Type (CREATE_SERVICE|UPDATE_SERVICE|DELETE_SERVICE|...), Status, TargetArn, StartedAt, EndedAt, UpdatedAt}` — the `OperationId` values previously returned must appear here.

NEVER STUBS: every one of these six must compute its response from persisted state. Hardcoded `Service` bodies, echoing the request back without storing it, or minting an `OperationId`/ARN that no later call can resolve are all failures of this issue. Validation of the modeled `length`/`pattern` constraints on `ServiceName` and the ARN members must reject bad input with `InvalidRequestException` (HTTP 400).

## State Model

In `src/state.rs` (`ComAmazonawsApprunnerApprunnerState`, reached via `crate::state::service_state(&*self.engine)`):

- `services: BTreeMap<String /*ServiceArn*/, ServiceRecord>` where
  `ServiceRecord { service_name: String, service_id: String, service_arn: String, service_url: String, created_at: SystemTime, updated_at: SystemTime, deleted_at: Option<SystemTime>, status: ServiceStatus, source_configuration: SourceConfiguration, instance_configuration: InstanceConfiguration, encryption_configuration: Option<EncryptionConfiguration>, health_check_configuration: HealthCheckConfiguration, auto_scaling_configuration_summary: AutoScalingConfigurationSummary, network_configuration: NetworkConfiguration, observability_configuration: Option<ServiceObservabilityConfiguration>, tags: BTreeMap<String,String> }`
- `operations: BTreeMap<String /*ServiceArn*/, Vec<OperationRecord>>` where
  `OperationRecord { id: Uuid-string, op_type: OperationType, status: OperationStatus, target_arn: String, started_at: SystemTime, ended_at: Option<SystemTime>, updated_at: SystemTime }`
- `name_index: BTreeMap<String /*ServiceName*/, String /*ServiceArn*/>` so duplicate-name creates can be detected and `ListServices` can be stably ordered.

Out of scope for this first slice (stay 501, filed separately later): connections, auto-scaling/observability configurations, VPC connectors and ingress connections, custom domains, Pause/Resume/StartDeployment, Tag/Untag/ListTagsForResource.

## Evidence

`services/service-com-amazonaws-apprunner-apprunner/OPERATIONS.md` lists all 37 operations with their input/output members and errors (CreateService l.51-55, DeleteService l.87-91, DescribeService l.124-128, ListOperations l.169-174, ListServices l.176-181, UpdateService l.246-250) and the wire protocol (`awsJson1_0`, `X-Amz-Target: AppRunner.<Op>`, sigv4 name `apprunner`). `grep -n '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/*.rs` matches all 37 handler files, each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. The `Service`, `ServiceSummary` and `OperationSummary` shape definitions (smithy_query) ground the required record fields: `ServiceName`, `ServiceId`, `ServiceArn`, `ServiceUrl`, `CreatedAt`, `UpdatedAt`, `DeletedAt`, `Status`, `SourceConfiguration`, `InstanceConfiguration`, `AutoScalingConfigurationSummary`, `NetworkConfiguration`; and `OperationSummary{Id,Type,Status,TargetArn,StartedAt,EndedAt,UpdatedAt}`. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI AppRunner client at it.
2. CreateService with ServiceName="my-web-svc" and a SourceConfiguration (ImageRepository with ImageIdentifier + ImageRepositoryType=ECR_PUBLIC) — expect 200 with Service.ServiceArn, Service.ServiceId, Service.ServiceUrl, Service.Status and a UUID OperationId (today: HTTP 501 NotImplemented).
3. DescribeService with the ServiceArn returned above — expect the identical persisted Service record (same ServiceId/ServiceUrl/SourceConfiguration/CreatedAt).
4. ListServices — expect exactly one ServiceSummary whose ServiceArn/ServiceName/Status match the created service; repeat with MaxResults=1 and follow NextToken after creating a second service.
5. UpdateService on that ARN with a changed InstanceConfiguration (e.g. Cpu="2 vCPU") — expect 200 + OperationId, then DescribeService must report the new InstanceConfiguration and a bumped UpdatedAt.
6. ListOperations for that ServiceArn — expect OperationSummary entries whose Ids include the OperationIds returned by CreateService and UpdateService, with Type CREATE_SERVICE / UPDATE_SERVICE.
7. DeleteService on that ARN — expect 200 + OperationId; ListServices no longer lists it and DescribeService on the ARN eventually returns ResourceNotFoundException (HTTP 400).
8. DescribeService with a well-formed but unknown ARN — expect ResourceNotFoundException (HTTP 400), not 501 and not a fabricated Service.
9. CreateService with ServiceName="ab" (shorter than the modeled min length 4) — expect InvalidRequestException (HTTP 400).
