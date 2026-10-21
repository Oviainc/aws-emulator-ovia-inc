Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Application Signals emulator

_Filed: 2026-10-21T21:18:35Z_
_Operation: service-com-amazonaws-applicationsignals-applicationsignals | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 30 operations in `services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the crate holds no state. The service is therefore unusable by any client.

First slice = the Service Level Objective (SLO) lifecycle, which is the primary resource of Application Signals (protocol `restJson1`, signing name `application-signals`). Implement these FIVE operations statefully, in this order:

1. **CreateServiceLevelObjective** — `POST /slo`. Input: `Name` (required, pattern `^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$`), `Description`, `SliConfig` | `RequestBasedSliConfig` (mutually exclusive per the model docs), `Goal`, `Tags`, `BurnRateConfigurations`, `CreateRecommendedSlo` (default false), `AutoInvestigationEnabled`. Must mint and persist a new SLO and return `Slo` (`ServiceLevelObjective`) with server-computed `Arn` (`arn:aws:application-signals:<region>:<account>:slo/<Name>`), `Name`, `Description`, `CreatedTime`, `LastUpdatedTime`, `Sli`/`RequestBasedSli` derived from the supplied config, `EvaluationType` (`PERIOD_BASED` when `SliConfig` was given, `REQUEST_BASED` when `RequestBasedSliConfig` was given), `Goal` (applying the documented defaults: rolling 7-day interval, `WarningThreshold` 50.0 when omitted), `MetricSourceType`, `AutoInvestigationEnabled`. A second create with the same `Name` must fail with `ConflictException` (HTTP 409).
2. **GetServiceLevelObjective** — `GET /slo/{Id}`. `Id` accepts either the SLO name or its ARN (the model's pattern is a name-or-ARN alternation). Must return the EXACT persisted `ServiceLevelObjective` written by the create, not a re-echo of the request; unknown id -> `ResourceNotFoundException` (HTTP 404).
3. **ListServiceLevelObjectives** — `POST /slos`. Must return one `ServiceLevelObjectiveSummary` per persisted SLO in `SloSummaries` (`Arn`, `Name`, `KeyAttributes`, `OperationName`, `DependencyConfig`, `CreatedTime`, `EvaluationType`, `MetricSourceType`), honouring `MaxResults` (1..50) and round-tripping `NextToken` so paging over >MaxResults SLOs terminates and visits each exactly once. `KeyAttributes`/`OperationName`/`MetricSourceTypes` filters must actually filter the stored set.
4. **UpdateServiceLevelObjective** — `PATCH /slo/{Id}`. Must mutate the stored record for the supplied `Description`, `SliConfig`/`RequestBasedSliConfig`, `Goal`, `BurnRateConfigurations`, `AutoInvestigationEnabled`, leave omitted members untouched, bump `LastUpdatedTime`, and return the updated `ServiceLevelObjective`. A following Get must show the update. Unknown id -> `ResourceNotFoundException`.
5. **DeleteServiceLevelObjective** — `DELETE /slo/{Id}`. Must remove the record (empty 200 body); a subsequent Get/Update must answer `ResourceNotFoundException` (HTTP 404) and the SLO must be gone from `ListServiceLevelObjectives`. Deleting an unknown id -> `ResourceNotFoundException`.

Every one of the five must compute its response from persisted state. No canned responses, no echoing the input back, no random ARN that is never stored. `Arn`, `CreatedTime`, `LastUpdatedTime` and `EvaluationType` are server-derived and must be stable across reads.

## State Model

In `crate::state` (reached via `crate::state::service_state(&*self.engine)` and its lock guard):

- `ApplicationSignalsState { slos: BTreeMap<String /* SLO name */, Slo>, tags: HashMap<String /* resource ARN */, BTreeMap<String,String>> }` — `BTreeMap` keyed by name gives deterministic list ordering and name-uniqueness for the ConflictException check.
- `Slo { arn: String, name: String, description: Option<String>, created_time: SystemTime, last_updated_time: SystemTime, sli: Option<ServiceLevelIndicator>, request_based_sli: Option<RequestBasedServiceLevelIndicator>, evaluation_type: EvaluationType, goal: Goal, burn_rate_configurations: Vec<BurnRateConfiguration>, metric_source_type: Option<MetricSourceType>, auto_investigation_enabled: Option<bool>, key_attributes: BTreeMap<String,String>, operation_name: Option<String>, dependency_config: Option<DependencyConfig>, tags: Vec<Tag> }` — one struct that serves both `ServiceLevelObjective` (Get/Create/Update) and `ServiceLevelObjectiveSummary` (List) projections.
- ARN resolution helper: `fn resolve(&self, id: &str) -> Option<&Slo>` accepting a bare name or a full `slo/<name>` ARN, used by Get/Update/Delete so all three agree on identity.
- Pagination: `NextToken` = opaque (e.g. base64) encoding of the last-returned SLO name, resolved against the `BTreeMap` cursor on the next call.

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): instrumentation configurations, grouping configuration, discovery, audit findings, service/dependency/operation/state listings, exclusion windows, budget reports, tagging operations.

Done = a client script that creates an SLO, gets it, lists it, updates it, gets it again seeing the update, deletes it and observes a 404 passes end-to-end with correct bodies — not merely 2xx.

## Evidence

`services/service-com-amazonaws-applicationsignals-applicationsignals/OPERATIONS.md` lists all 30 operations with routes/members: CreateServiceLevelObjective (`POST /slo`, output `Slo` required, errors ConflictException 409 / ValidationException 400), GetServiceLevelObjective (`GET /slo/{Id}`, @readonly, ResourceNotFoundException 404), UpdateServiceLevelObjective (`PATCH /slo/{Id}`), DeleteServiceLevelObjective (`DELETE /slo/{Id}`, @idempotent), ListServiceLevelObjectives (`POST /slos`, @paginated inputToken=NextToken outputToken=NextToken items=SloSummaries, MaxResults range 1..50). `smithy_query(shape="ServiceLevelObjective,ServiceLevelObjectiveSummary,Goal")` confirms the required server-computed members `Arn` (pattern `^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/...$`), `Name`, `CreatedTime`, `LastUpdatedTime`, `Goal`, plus `EvaluationType`/`MetricSourceType`, and that `Goal.WarningThreshold` defaults to 50.0 and `Goal.Interval` defaults to a rolling 7-day interval when omitted. `src/ops/create_service_level_objective.rs:4` carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and line 222 returns `Scaffolded::not_implemented("CreateServiceLevelObjective")`; the same shape holds for all 30 files in `src/ops/`. No tests assert any SLO behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `application-signals` client at it.
2. `aws application-signals create-service-level-objective --name demo-slo --description 'bootstrap demo' --sli-config '{...}' --goal '{"AttainmentGoal":99.9}'` -> expect HTTP 200 with `Slo.Arn` ending in `slo/demo-slo`, `Slo.CreatedTime`/`Slo.LastUpdatedTime` set, `Slo.EvaluationType=PERIOD_BASED`, `Slo.Goal.WarningThreshold=50.0`; actual: HTTP 501 NotImplemented.
3. Repeat the same create -> expect HTTP 409 ConflictException; actual: 501.
4. `aws application-signals get-service-level-objective --id demo-slo` -> expect the identical persisted SLO (same Arn and CreatedTime); actual: 501.
5. `aws application-signals list-service-level-objectives --max-results 1` -> expect `SloSummaries` containing demo-slo and a `NextToken` only when more SLOs remain; actual: 501.
6. `aws application-signals update-service-level-objective --id demo-slo --description updated` then get again -> expect `Description=updated` and a bumped `LastUpdatedTime` with `CreatedTime` unchanged; actual: 501.
7. `aws application-signals delete-service-level-objective --id demo-slo`, then get it -> expect HTTP 404 ResourceNotFoundException and absence from list-service-level-objectives; actual: 501.
