Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the InternetMonitor emulator

_Filed: 2026-10-21T21:08:28Z_
_Operation: service-com-amazonaws-internetmonitor-internetmonitor20210603 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-internetmonitor-internetmonitor20210603/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST stateful slice covering the monitor lifecycle — the service's primary resource — so a realistic client script works end-to-end. Protocol is `restJson1`, signing name `internetmonitor`.

### Happy path (implement in this order)

1. **CreateMonitor** — `POST /v20210603/Monitors`. Input: `MonitorName` (required, len 1..255, `^[a-zA-Z0-9_.-]+$`), `Resources` (list of ARNs), `Tags`, `MaxCityNetworksToMonitor` (1..500000), `TrafficPercentageToMonitor` (1..100), `InternetMeasurementsLogDelivery`, `HealthEventsConfig`, `ClientToken` (@idempotencyToken). Must persist a new monitor record and return the generated `Arn` (`arn:aws:internetmonitor:<region>:<account>:monitor/<MonitorName>`, len 20..512, `^arn:.*`) plus `Status` (`PENDING` on create, settling to `ACTIVE`). Re-creating an existing `MonitorName` must return `ConflictException` (409); a repeat with the same `ClientToken` must be idempotent rather than conflict.
2. **GetMonitor** — `GET /v20210603/Monitors/{MonitorName}`. Must read the stored record and return `MonitorName`, `MonitorArn`, `Resources`, `Status`, `CreatedAt`, `ModifiedAt`, `ProcessingStatus`, `Tags`, `MaxCityNetworksToMonitor`, `TrafficPercentageToMonitor`, `InternetMeasurementsLogDelivery`, `HealthEventsConfig` — exactly the values supplied at create/update time. Unknown name → `ValidationException`/`ResourceNotFoundException` per the model (this op models `ValidationException` 400 only; use it rather than a 200 with empty fields).
3. **ListMonitors** — `GET /v20210603/Monitors`. Must return one `Monitor` summary (`MonitorName`, `MonitorArn`, `Status`, `ProcessingStatus`) per persisted monitor, honouring `MaxResults` (1..25) and round-tripping an opaque `NextToken`, and filtering by `MonitorStatus` when given. A monitor created in step 1 must appear here; one deleted in step 5 must not.
4. **UpdateMonitor** — `PATCH /v20210603/Monitors/{MonitorName}`. Must mutate the stored record: `ResourcesToAdd`/`ResourcesToRemove` applied as a set union/difference on `Resources`, `Status`, `MaxCityNetworksToMonitor`, `TrafficPercentageToMonitor`, `InternetMeasurementsLogDelivery`, `HealthEventsConfig` overwritten when present, and `ModifiedAt` bumped. Returns `MonitorArn` + `Status`. Unknown name → `ResourceNotFoundException` (404). A subsequent GetMonitor must observe the change.
5. **DeleteMonitor** — `DELETE /v20210603/Monitors/{MonitorName}`. Must remove the record (and its tags) from state; empty body. A following GetMonitor must fail and ListMonitors must not include it.
6. **TagResource / ListTagsForResource** (optional sixth, cheap once monitors are stored) — `POST /tags/{ResourceArn}` (204) merges `Tags` onto the monitor identified by its ARN; `GET /tags/{ResourceArn}` returns exactly the merged map; unknown ARN → `NotFoundException` (404).

Every one of these must compute its response from persisted state. No canned bodies, no echoing the input back, no returning a random ARN without storing anything — a create followed by a get/list must return what was created, and a delete must be observable.

## State Model

In `crate::state` (the store reached via `crate::state::service_state(&*self.engine)`):

- `monitors: BTreeMap<String /* MonitorName */, Monitor>`
- `Monitor { name: String, arn: String, resources: BTreeSet<String>, status: MonitorConfigState /* PENDING|ACTIVE|INACTIVE|ERROR */, processing_status: Option<HealthEventsProcessingStatus> /* default COLLECTING_DATA */, processing_status_info: Option<String>, created_at: SystemTime, modified_at: SystemTime, tags: BTreeMap<String,String>, max_city_networks_to_monitor: Option<i32>, traffic_percentage_to_monitor: Option<i32>, internet_measurements_log_delivery: Option<InternetMeasurementsLogDelivery>, health_events_config: Option<HealthEventsConfig> }`
- `client_tokens: HashMap<String /* ClientToken */, String /* MonitorName */>` for CreateMonitor/UpdateMonitor idempotency.
- `tags_by_arn: ` either a derived lookup `arn -> MonitorName` or resolve the ARN by scanning `monitors`, so Tag/Untag/ListTags act on the same `Monitor.tags` map GetMonitor returns.

Out of scope for this first slice (stay 501, filed separately later): StartQuery, GetQueryStatus, GetQueryResults, StopQuery, ListHealthEvents, GetHealthEvent, ListInternetEvents, GetInternetEvent, UntagResource.

## Evidence

`services/service-com-amazonaws-internetmonitor-internetmonitor20210603/OPERATIONS.md` lists all 16 operations with routes, input/output members and errors used above (CreateMonitor lines 65-71, GetMonitor 73-79, UpdateMonitor 81-87, DeleteMonitor 89-95, ListMonitors 97-103, Tag/ListTags 25-39). `src/ops/create_monitor.rs` is representative of all 16 handler files: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header (line 4) and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateMonitor"))` (line 210), i.e. HTTP 501 with zero persisted state. Field names, constraints, status enums and error/status codes are taken from the Smithy model as rendered in OPERATIONS.md.

## Steps to Reproduce

1. aws internetmonitor create-monitor --monitor-name demo-monitor --resources arn:aws:ec2:us-east-1:123456789012:vpc/vpc-abc123 --max-city-networks-to-monitor 100 --tags Env=dev --endpoint-url <emulator>  # expect 200 with Arn + Status, currently 501
2. aws internetmonitor get-monitor --monitor-name demo-monitor  # expect the same Arn, Resources=[vpc-abc123], MaxCityNetworksToMonitor=100, Tags={Env:dev}, CreatedAt/ModifiedAt set
3. aws internetmonitor list-monitors  # expect demo-monitor present with its ARN and Status
4. aws internetmonitor update-monitor --monitor-name demo-monitor --resources-to-add arn:aws:ec2:us-east-1:123456789012:vpc/vpc-def456 --traffic-percentage-to-monitor 50  # expect 200, then get-monitor shows both VPCs, TrafficPercentageToMonitor=50 and a bumped ModifiedAt
5. aws internetmonitor tag-resource --resource-arn <Arn> --tags Team=net && aws internetmonitor list-tags-for-resource --resource-arn <Arn>  # expect {Env:dev, Team:net}
6. aws internetmonitor delete-monitor --monitor-name demo-monitor  # expect success, then get-monitor fails as not found and list-monitors no longer includes it
7. aws internetmonitor create-monitor --monitor-name demo-monitor ... twice  # second call expects ConflictException (409)
