Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the EventBridge emulator

_Filed: 2026-10-21T23:18:09Z_
_Operation: service-com-amazonaws-eventbridge-awsevents | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 57 handlers under `services/service-com-amazonaws-eventbridge-awsevents/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented` while holding zero state. No client workflow works at all.

Implement a FIRST, SMALL happy-path slice: the rule lifecycle on the default event bus, driven end-to-end by a real event publish. Protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<OperationName>` (README / OPERATIONS.md 'Wire protocol').

In-scope operations, in workflow order (fields from OPERATIONS.md):

1. **PutRule** — in: `Name` (required), `EventPattern`, `ScheduleExpression`, `State` (ENABLED|DISABLED, default ENABLED), `Description`, `RoleArn`, `EventBusName` (default `default`). Out: `RuleArn`, which must be a deterministic `arn:aws:events:<region>:<account>:rule/<bus-if-not-default>/<name>` built from the stored rule, not a random string. Must create-or-update in place: calling PutRule twice with the same Name mutates the stored rule and returns the SAME RuleArn. Reject a request with neither `EventPattern` nor `ScheduleExpression` and reject a syntactically invalid pattern with `InvalidEventPatternException`; `ResourceNotFoundException` when `EventBusName` names a bus that does not exist.

2. **PutTargets** — in: `Rule` (required), `EventBusName`, `Targets` (1..100, required; each `Target` carries at least `Id`, `Arn`, `RoleArn`, `Input`, `InputPath`). Out: `FailedEntryCount` (required) + `FailedEntries`. Targets must be stored against the rule, upserted by `Id` (re-putting the same Id replaces that target, it does not duplicate). `FailedEntryCount` must be 0 on success and computed from actual per-entry outcomes, never hardcoded. `ResourceNotFoundException` when the rule does not exist on that bus.

3. **PutEvents** — in: `Entries` (1..10, required; each entry has `Source`, `DetailType`, `Detail`, `Resources`, `Time`, `EventBusName`). Out: `FailedEntryCount` (required) + `Entries` of `PutEventsResultEntry` with a freshly generated `EventId` per accepted entry (and `ErrorCode`/`ErrorMessage` for rejected ones). Each entry must be routed: evaluate the stored rules' `EventPattern` against the event on the entry's bus and record the match (matched rule names + the event) in state so a test can observe that the event reached the rule's targets. `FailedEntryCount` must reflect real per-entry failures (e.g. a non-existent target bus), not a constant 0.

4. **DescribeRule** — in: `Name` (required), `EventBusName`. Out: `Name`, `Arn`, `EventPattern`, `ScheduleExpression`, `State`, `Description`, `RoleArn`, `EventBusName`, `CreatedBy`, `ManagedBy` — every field read back from the rule stored by PutRule (round-trip fidelity, not an echo of the DescribeRule input). `ResourceNotFoundException` for an unknown rule.

5. **ListTargetsByRule** — in: `Rule` (required), `EventBusName`, `Limit` (1..100), `NextToken`. Out: `Targets`, `NextToken`. Must return exactly the targets stored by PutTargets, honour `Limit` as a page size and return/accept an opaque `NextToken` that resumes where the previous page ended. `ResourceNotFoundException` for an unknown rule.

6. **ListRules** — in: `NamePrefix`, `EventBusName`, `Limit`, `NextToken`. Out: `Rules`, `NextToken`. Must filter by `NamePrefix` and by bus, and paginate as above.

7. **RemoveTargets** / **DeleteRule** — RemoveTargets in: `Rule` (required), `Ids` (1..100, required), `Force`; out `FailedEntryCount` + `FailedEntries` computed from which `Ids` actually existed. DeleteRule in: `Name` (required), `EventBusName`, `Force`; it must delete the rule from state so a following DescribeRule returns `ResourceNotFoundException` and ListRules no longer lists it. Per well-established EventBridge behaviour, DeleteRule on a rule that still has targets must fail (`ValidationException`-style refusal) unless `Force` is true.

Hard requirement: NEVER STUBS. Every one of these must compute its response from persisted state held in the single shared engine store reached via `crate::state::service_state(&*self.engine)`. Hardcoded ARNs, echoed inputs, a random `EventId` that is never recorded, or a constant `FailedEntryCount: 0` are failures, not implementations.

A `default` event bus must exist implicitly at startup so the whole slice works without CreateEventBus (which stays stubbed for now).

## State Model

In `src/state.rs`, on `ComAmazonawsEventbridgeAwseventsState`:

- `buses: BTreeMap<String, EventBus>` — keyed by bus name, pre-seeded with `"default"`.
  - `EventBus { name: String, arn: String, description: Option<String>, created: SystemTime, rules: BTreeMap<String, Rule> }`
- `Rule { name: String, arn: String, event_pattern: Option<String>, schedule_expression: Option<String>, state: RuleState, description: Option<String>, role_arn: Option<String>, managed_by: Option<String>, created_by: Option<String>, event_bus_name: String, targets: Vec<StoredTarget> }` — `targets` keeps insertion order for ListTargetsByRule paging, upsert keyed by `StoredTarget.id`.
- `StoredTarget { id: String, arn: String, role_arn: Option<String>, input: Option<String>, input_path: Option<String>, raw: Target }` — `raw` preserves the full modeled `Target` so ListTargetsByRule can return it verbatim.
- `delivered: Vec<DeliveredEvent>` — the routing log PutEvents appends to, so tests can assert an event reached a rule's targets: `DeliveredEvent { event_id: String, bus: String, source: Option<String>, detail_type: Option<String>, detail: Option<String>, resources: Vec<String>, time: SystemTime, matched_rules: Vec<String>, target_arns: Vec<String> }`.
- `account_id: String` and `region: String` used to mint deterministic rule/target ARNs.

A shared `event_pattern::matches(pattern_json, event_json) -> bool` helper (exact-value lists, `prefix`, `exists`, nested objects) is written once and reused by PutEvents routing; the same helper later backs TestEventPattern.

Everything else (archives, replays, connections, api destinations, endpoints, partner event sources, permissions, tagging, Enable/DisableRule, CreateEventBus/ListEventBuses, TestEventPattern) stays at 501 and will be filed as individual gap issues.

Done = a realistic integration test performing PutRule -> PutTargets -> PutEvents -> DescribeRule -> ListTargetsByRule -> ListRules -> RemoveTargets -> DeleteRule passes with correct, stateful response bodies — not merely 2xx.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-eventbridge-awsevents/src/ops/` matches all 57 operation files (put_rule.rs, put_targets.rs, put_events.rs, describe_rule.rs, list_targets_by_rule.rs, list_rules.rs, remove_targets.rs, delete_rule.rs among them), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. Operation contracts and field names above are taken from `services/service-com-amazonaws-eventbridge-awsevents/OPERATIONS.md`: PutRule (line 291), PutTargets (line 297), PutEvents (line 273), DescribeRule (line 177), ListTargetsByRule (line 267), ListRules (line 255), RemoveTargets (line 309), DeleteRule (line 123); wire protocol `awsJson1_1` + `X-Amz-Target: AWSEvents.<Op>` from OPERATIONS.md lines 8-15. Expected lifecycle/error semantics (create-or-update PutRule, target upsert by Id, ResourceNotFoundException for unknown rule/bus, InvalidEventPatternException for bad patterns, Force required to delete a rule with targets, implicit `default` bus) are well-established EventBridge behaviour and are the errors the model declares for these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `events` client at it.
2. `PutRule --name demo-rule --event-pattern '{"source":["com.demo.app"]}' --state ENABLED` — expect 200 with a `RuleArn` ending in `rule/demo-rule`; today: HTTP 501 NotImplemented.
3. `PutTargets --rule demo-rule --targets 'Id=t1,Arn=arn:aws:sqs:us-east-1:123456789012:demo-queue'` — expect 200 with `FailedEntryCount: 0`.
4. `PutEvents --entries 'Source=com.demo.app,DetailType=demo,Detail={"k":1}'` — expect 200, `FailedEntryCount: 0`, one `Entries[0].EventId`, and the event recorded as matching `demo-rule` and routed to target `t1`.
5. `DescribeRule --name demo-rule` — expect the EventPattern, State, Arn and EventBusName exactly as created.
6. `ListTargetsByRule --rule demo-rule` — expect the single target `t1` with its Arn; with `--limit 1` on a two-target rule expect a usable `NextToken` that returns the second target.
7. `ListRules --name-prefix demo` — expect `demo-rule` listed; `--name-prefix zzz` returns an empty list.
8. `RemoveTargets --rule demo-rule --ids t1` then `DeleteRule --name demo-rule` — expect 200s, then `DescribeRule --name demo-rule` returns `ResourceNotFoundException` and `ListRules` no longer lists it.
