Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the CloudWatch Events (EventBridge) emulator

_Filed: 2026-10-21T22:08:29Z_
_Operation: service-com-amazonaws-cloudwatchevents-awsevents | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 51 operations in `services/service-com-amazonaws-cloudwatchevents-awsevents/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented` while persisting nothing. The service has no working emulator at all.

Implement a FIRST SLICE of six operations that form the service's core rule-lifecycle workflow on the default event bus, all backed by real shared state (`crate::state::service_state(&*self.engine)`), on the `awsJson1_1` wire protocol (`POST /`, `X-Amz-Target: AWSEvents.<Operation>`).

### Happy path (in order)

1. **PutRule** — input `Name` (required, 1..64, `^[\.\-_A-Za-z0-9]+$`), `EventPattern`, `ScheduleExpression`, `State` (`ENABLED`|`DISABLED`, default `ENABLED` when omitted), `Description`, `RoleArn`, `Tags`, `EventBusName` (default `default`). Create OR update (idempotent upsert) the rule in state and return `RuleArn` = `arn:aws:events:<region>:<account>:rule/<Name>` (and `rule/<bus>/<Name>` for a non-default bus). A second PutRule with the same name must overwrite the stored fields, not create a duplicate. Reject an input that gives neither `EventPattern` nor `ScheduleExpression` with `ValidationException`/`InvalidEventPatternException` per the model errors, and a syntactically invalid `EventPattern` JSON with `InvalidEventPatternException` (HTTP 400).
2. **PutTargets** — input `Rule` (required), `EventBusName`, `Targets` (required, 1..100, each `Target{Id (required), Arn (required), RoleArn, Input, InputPath, InputTransformer, SqsParameters, ...}`). Append/replace targets by `Id` on the stored rule. Output `FailedEntryCount` (required, 0 on success) and `FailedEntries` (empty list on success). An unknown `Rule` must yield `ResourceNotFoundException` (HTTP 400), not a success.
3. **DescribeRule** — input `Name` (required), `EventBusName`. Must return the values that PutRule persisted: `Name`, `Arn`, `EventPattern`, `ScheduleExpression`, `State`, `Description`, `RoleArn`, `EventBusName`, `CreatedBy`. Unknown name → `ResourceNotFoundException`.
4. **ListTargetsByRule** — input `Rule` (required), `EventBusName`, `Limit` (1..100), `NextToken`. Return the exact `Targets` stored by PutTargets, in insertion order, honoring `Limit` and emitting/accepting `NextToken` when truncated. Unknown rule → `ResourceNotFoundException`.
5. **PutEvents** — input `Entries` (required, 1..10, each `PutEventsRequestEntry{Source, DetailType, Detail, Resources, Time, EventBusName, TraceHeader}`). For each entry allocate a unique `EventId` (UUID) and return it in `Entries[i].EventId`; `FailedEntryCount` must be the real count of failed entries (0 on the happy path). Entries must be recorded against the named bus (default `default`) and matched against stored enabled rules' event patterns (at minimum `source` / `detail-type` exact and list matching) so a later read/assertion can see which rules matched — no canned `FailedEntryCount: 0` with a random ID and no storage.
6. **DeleteRule** — input `Name` (required), `EventBusName`, `Force`. Remove the rule from state; a subsequent DescribeRule must return `ResourceNotFoundException` (HTTP 400). Deleting a rule that still has targets must fail the way the real service does (`ValidationException` "Rule can't be deleted since it has targets") unless the targets were removed first; deleting an unknown rule → `ResourceNotFoundException`.

Also implement **ListRules** (input `NamePrefix`, `EventBusName`, `Limit` 1..100, `NextToken`; output `Rules` as `list<Rule>` + `NextToken`) if it is cheap — it is the natural verification read for the loop and must list exactly the rules created by PutRule, filtered by `NamePrefix` and bounded by `Limit`.

NEVER STUBS: each of these must compute its response from the persisted state. Hardcoded ARNs, echoed inputs, empty lists from a non-empty store, or a random EventId without storage are failures of this issue.

## State Model

Store one struct in the service state (reached through `crate::state::service_state`, mutated under `state.lock()`):

```
EventsState {
    // bus name -> EventBus
    buses: BTreeMap<String, EventBus>,
}

EventBus {
    name: String,            // "default" pre-created at first touch
    arn: String,             // arn:aws:events:<region>:<account>:event-bus/<name>
    policy: Option<String>,
    rules: BTreeMap<String, Rule>,     // rule name -> Rule
    events: Vec<RecordedEvent>,        // what PutEvents accepted
}

Rule {
    name: String,
    arn: String,
    event_pattern: Option<String>,
    schedule_expression: Option<String>,
    state: RuleState,                  // ENABLED | DISABLED, default ENABLED
    description: Option<String>,
    role_arn: Option<String>,
    managed_by: Option<String>,
    event_bus_name: String,
    created_by: Option<String>,
    tags: Vec<Tag>,
    targets: Vec<Target>,              // insertion-ordered, unique by Target.Id
}

RecordedEvent {
    event_id: String,                  // UUID returned by PutEvents
    source: Option<String>,
    detail_type: Option<String>,
    detail: Option<String>,
    resources: Vec<String>,
    time: SystemTime,                  // request Time, else now
    bus_name: String,
    matched_rules: Vec<String>,        // rules whose pattern matched
}
```

Everything outside this slice (event buses CRUD, archives, replays, connections, API destinations, partner event sources, permissions, tagging, TestEventPattern, EnableRule/DisableRule, RemoveTargets, ListRuleNamesByTarget) stays 501 and will be filed as individual gap issues later.

## Evidence

`OPERATIONS.md` (crate root, 51 operations from `smithy-model/com-amazonaws-cloudwatchevents-awsevents.json`): wire protocol `awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<OperationName>`, sigv4 name `events`. PutRule (line 267) — input `Name` required + `EventPattern`/`ScheduleExpression`/`State`/`Description`/`RoleArn`/`Tags`/`EventBusName`, output `RuleArn`, errors include `InvalidEventPatternException (HTTP 400)` and `ResourceNotFoundException (HTTP 400)`. PutTargets (line 273) — `Rule`+`Targets` (1..100) required, output `FailedEntryCount` (required, default 0) + `FailedEntries`. DescribeRule (line 159) — output `Name`/`Arn`/`EventPattern`/`ScheduleExpression`/`State`/`Description`/`RoleArn`/`ManagedBy`/`EventBusName`/`CreatedBy`, error `ResourceNotFoundException`. ListTargetsByRule (line 243) and ListRules (line 231) — `Limit` 1..100 + `NextToken` pagination. PutEvents (line 249) — `Entries` 1..10 required, output `FailedEntryCount` + `Entries` (`PutEventsResultEntry{EventId, ErrorCode, ErrorMessage}` per smithy_query). DeleteRule (line 111) — `Name` required, `Force` default false, error `ResourceNotFoundException`. smithy_query confirms `Target{Id required, Arn required, ...}` and `Rule{Name, Arn, EventPattern, State, Description, ScheduleExpression, RoleArn, ManagedBy, EventBusName}`. All 52 files under `src/ops/` (51 ops + mod.rs) match `^//! SCAFFOLDED, NOT IMPLEMENTED:` at line 4 — including put_rule.rs, put_targets.rs, describe_rule.rs, list_targets_by_rule.rs, put_events.rs, delete_rule.rs — so no handler holds state today. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an EventBridge/CloudWatch Events client at it (endpoint prefix `events`, protocol awsJson1_1).
2. PutRule(Name="demo-rule", EventPattern='{"source":["my.app"]}', State="ENABLED", Description="demo") -> expect 200 with RuleArn ending in `rule/demo-rule`; today: HTTP 501 NotImplemented.
3. DescribeRule(Name="demo-rule") -> expect the same Arn plus the EventPattern, State=ENABLED and Description exactly as submitted.
4. PutTargets(Rule="demo-rule", Targets=[{Id:"t1", Arn:"arn:aws:sqs:us-east-1:123456789012:q1"}]) -> expect FailedEntryCount=0 and empty FailedEntries.
5. ListTargetsByRule(Rule="demo-rule") -> expect exactly one target with Id="t1" and the submitted Arn.
6. ListRules(NamePrefix="demo") -> expect `Rules` to contain demo-rule with its stored Arn/State/EventPattern.
7. PutEvents(Entries=[{Source:"my.app", DetailType:"test", Detail:"{\"k\":1}"}]) -> expect FailedEntryCount=0 and a non-empty unique EventId per entry, recorded in state and matched against demo-rule's pattern.
8. PutTargets again with Id="t1" but a different Arn -> ListTargetsByRule must show ONE target with the updated Arn (replace by Id, not duplicate).
9. DescribeRule(Name="no-such-rule") -> expect ResourceNotFoundException (HTTP 400), not 501 and not a hollow 200.
10. RemoveTargets or targetless rule, then DeleteRule(Name="demo-rule") -> expect 200; a following DescribeRule(Name="demo-rule") must return ResourceNotFoundException and ListRules must no longer include it.
