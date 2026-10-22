Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the observabilityadmin emulator

_Filed: 2026-10-21T22:28:17Z_
_Operation: service-com-amazonaws-observabilityadmin-observabilityadmin | Class: missing_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 39 handlers under `services/service-com-amazonaws-observabilityadmin-observabilityadmin/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow can run against this service.

The first slice should make the **telemetry rule** lifecycle (the service's primary resource) work end-to-end, statefully, over `restJson1` (SDK id `ObservabilityAdmin`, sigv4 name `observabilityadmin`). Scope — exactly these 5 operations:

1. **CreateTelemetryRule** — `POST /CreateTelemetryRule`. Input `RuleName` (required, len 1..100, pattern `^[0-9A-Za-z-_.#/]+$`), `Rule` (TelemetryRule, required — members `TelemetryType` (required), `ResourceType`, `TelemetrySourceTypes`, `DestinationConfiguration`, `Scope`, `SelectionCriteria`, `AllowFieldUpdates`, `Regions`, `AllRegions`), optional `Tags`. Must mint and store a rule ARN of the form `arn:aws:observabilityadmin:<region>:<account-id>:telemetry-rule/<RuleName>`, persist the whole `Rule` body plus `Tags` plus creation/update timestamps, and return the minted `RuleArn`. A second create with the same `RuleName` must fail `ConflictException` (409), not overwrite. Missing/ill-formed `RuleName` or a `Rule` with no `TelemetryType` must fail `ValidationException` (400).
2. **GetTelemetryRule** — `POST /GetTelemetryRule`. Input `RuleIdentifier` (required) — must resolve BOTH a bare rule name and the full rule ARN. Output must be read back from the store: `RuleName`, `RuleArn`, `CreatedTimeStamp` (long, epoch millis), `LastUpdateTimeStamp` (long), `TelemetryRule` (byte-for-byte what was created, including `TelemetryType`/`ResourceType`/`DestinationConfiguration`), `HomeRegion`, `IsReplicated`, `RegionStatuses`. Unknown identifier -> `ResourceNotFoundException` (404).
3. **ListTelemetryRules** — `POST /ListTelemetryRules`. Returns `TelemetryRuleSummaries` built from the store, one `TelemetryRuleSummary{RuleName, RuleArn, CreatedTimeStamp, LastUpdateTimeStamp, ResourceType, TelemetryType, TelemetrySourceTypes}` per stored rule. Must honour `RuleNamePrefix` filtering, `MaxResults` (range 1..100) truncation, and emit/accept an opaque `NextToken` so pagination terminates (the op is `@paginated(inputToken=NextToken, outputToken=NextToken, items=TelemetryRuleSummaries)`).
4. **UpdateTelemetryRule** — `POST /UpdateTelemetryRule`. Input `RuleIdentifier` (required) + `Rule` (required) replaces the stored `Rule`, bumps `LastUpdateTimeStamp`, leaves `CreatedTimeStamp` and `Tags` untouched, and returns the existing `RuleArn`. Unknown identifier -> `ResourceNotFoundException` (404). A subsequent GetTelemetryRule must observe the new `Rule`.
5. **DeleteTelemetryRule** — `POST /DeleteTelemetryRule`. Input `RuleIdentifier` (required). Removes the rule (and its tags) from the store; empty body on success. Unknown identifier -> `ResourceNotFoundException` (404), and a GetTelemetryRule after delete must 404 while ListTelemetryRules no longer lists it.

Every one of these MUST compute its response from persisted state held in the shared engine store reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the request back, no minting an ARN without storing the rule behind it. A create followed by a get/list must return what was created; a delete must be observable by the next reader.

## State Model

In `src/state.rs` (`ComAmazonawsObservabilityadminObservabilityadminState`):

- `telemetry_rules: BTreeMap<String /* RuleName */, StoredTelemetryRule>` — BTreeMap so `ListTelemetryRules` has a stable, name-ordered page sequence for `NextToken`.
- `StoredTelemetryRule {
    rule_name: String,
    rule_arn: String,              // arn:aws:observabilityadmin:<region>:<account>:telemetry-rule/<name>
    rule: TelemetryRule,           // the modeled struct, stored verbatim
    created_timestamp_ms: i64,     // -> CreatedTimeStamp (Long)
    last_update_timestamp_ms: i64, // -> LastUpdateTimeStamp (Long)
    home_region: String,
    is_replicated: bool,           // true when rule.Regions is non-empty or rule.AllRegions
    region_statuses: Vec<RegionStatus>,
    tags: BTreeMap<String, String>,
  }`
- `arn_index: BTreeMap<String /* rule_arn */, String /* rule_name */>` — so `RuleIdentifier` can be resolved as either a name or an ARN by every read/update/delete.

All other operations (the ForOrganization variants, telemetry pipelines, dataset/S3-table integrations, resource-telemetry listing, evaluation/enrichment start-stop, and Tag/Untag/ListTagsForResource) stay at 501 for this slice and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-observabilityadmin-observabilityadmin/OPERATIONS.md` lines 46-51 (CreateTelemetryRule: route, required `RuleName`/`Rule`, `RuleArn` output, ConflictException 409 / ValidationException 400), 82-87 (DeleteTelemetryRule: `RuleIdentifier`, empty output, ResourceNotFoundException 404), 144-150 (GetTelemetryRule: `@readonly`, full output member list incl. `CreatedTimeStamp`/`LastUpdateTimeStamp`/`TelemetryRule`/`HomeRegion`/`IsReplicated`/`RegionStatuses`), 208-214 (ListTelemetryRules: `@paginated(inputToken=NextToken, outputToken=NextToken, items=TelemetryRuleSummaries)`, `RuleNamePrefix`, `MaxResults` 1..100), 302-307 (UpdateTelemetryRule: `RuleIdentifier`+`Rule`, `RuleArn` output, 404/409). Wire protocol `restJson1` from OPERATIONS.md lines 8-13 and README.md. `smithy_query shape=TelemetryRule,TelemetryRuleSummary` confirms `TelemetryType` is the only `@required` member of `TelemetryRule` and gives `TelemetryRuleSummary`'s exact member set. Stub census: `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 39 operation files, including `create_telemetry_rule.rs`, `get_telemetry_rule.rs`, `list_telemetry_rules.rs`, `update_telemetry_rule.rs`, `delete_telemetry_rule.rs`. No tests assert any of this behavior (only the generated `tests/state_roundtrip.rs` example exists).

## Steps to Reproduce

1. POST /CreateTelemetryRule with {"RuleName":"vpc-flow-logs","Rule":{"ResourceType":"AWS::EC2::VPC","TelemetryType":"Logs","TelemetrySourceTypes":["VPC_FLOW_LOGS"],"DestinationConfiguration":{...}},"Tags":{"team":"obs"}} — expect 200 with a `RuleArn` ending in `telemetry-rule/vpc-flow-logs`; currently 501 NotImplemented.
2. POST /GetTelemetryRule with {"RuleIdentifier":"vpc-flow-logs"} — expect the stored rule back: `RuleName`, the same `RuleArn`, non-zero `CreatedTimeStamp`/`LastUpdateTimeStamp`, and a `TelemetryRule` identical to the one created.
3. POST /GetTelemetryRule with {"RuleIdentifier":"<the RuleArn returned by step 1>"} — expect the same payload (identifier resolves by ARN as well as by name).
4. POST /CreateTelemetryRule again with RuleName "vpc-flow-logs" — expect 409 ConflictException and no mutation of the stored rule.
5. POST /ListTelemetryRules with {"RuleNamePrefix":"vpc-","MaxResults":1} — expect exactly one `TelemetryRuleSummary` for the rule, with `ResourceType`/`TelemetryType`/timestamps populated from state; paging with the returned `NextToken` must eventually come back empty with no token.
6. POST /UpdateTelemetryRule with {"RuleIdentifier":"vpc-flow-logs","Rule":{...TelemetryType:"Logs", changed DestinationConfiguration...}} then POST /GetTelemetryRule — expect the new `Rule`, an advanced `LastUpdateTimeStamp`, and an unchanged `CreatedTimeStamp`.
7. POST /DeleteTelemetryRule with {"RuleIdentifier":"vpc-flow-logs"} — expect success; then POST /GetTelemetryRule for the same identifier expects 404 ResourceNotFoundException, POST /ListTelemetryRules returns an empty `TelemetryRuleSummaries`, and a second DeleteTelemetryRule returns 404.
8. POST /CreateTelemetryRule with {"RuleName":"bad name!","Rule":{"TelemetryType":"Logs"}} — expect 400 ValidationException (pattern violation), and nothing added to the store.
