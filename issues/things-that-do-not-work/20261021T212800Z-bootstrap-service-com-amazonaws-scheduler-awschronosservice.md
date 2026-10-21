Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the EventBridge Scheduler (scheduler) emulator

_Filed: 2026-10-21T21:28:00Z_
_Operation: service-com-amazonaws-scheduler-awschronosservice | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 12 operations in `services/service-com-amazonaws-scheduler-awschronosservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding zero state. No client workflow works at all.

Implement a FIRST, SMALL, stateful slice: the schedule lifecycle of the primary resource (`Schedule`), plus the group it lives in. Scope = these 6 operations, in this order (restJson1, sigv4 signing name `scheduler`):

1. **CreateScheduleGroup** — `POST /schedule-groups/{Name}`. Input: `Name` (required, @httpLabel, 1..64, `^[0-9a-zA-Z-_.]+$`), `Tags`, `ClientToken`. Output: `ScheduleGroupArn` (required) — must be a real generated ARN of the form `arn:aws:scheduler:<region>:<account>:schedule-group/<Name>`, stored. Creating a group whose name already exists must raise `ConflictException` (409).
2. **CreateSchedule** — `POST /schedules/{Name}`. Input: `Name` (required), `GroupName` (defaults to the group named `default`, which must exist implicitly), `ScheduleExpression` (required), `Target` (required: `Arn`, `RoleArn` both required), `FlexibleTimeWindow` (required: `Mode` required, optional `MaximumWindowInMinutes` 1..1440), plus `StartDate`, `EndDate`, `Description`, `ScheduleExpressionTimezone`, `State` (ENABLED|DISABLED, default ENABLED), `KmsKeyArn`, `ActionAfterCompletion`. Output: `ScheduleArn` (required) = `arn:aws:scheduler:<region>:<account>:schedule/<GroupName>/<Name>`. Must persist the whole schedule, set `CreationDate`/`LastModificationDate`, return `ConflictException` (409) on duplicate (group, name), and `ResourceNotFoundException` (404) when `GroupName` names a group that was never created.
3. **GetSchedule** — `GET /schedules/{Name}?groupName=`. Must return the persisted record field-for-field: `Arn`, `Name`, `GroupName`, `ScheduleExpression`, `ScheduleExpressionTimezone`, `StartDate`, `EndDate`, `Description`, `State`, `KmsKeyArn`, `Target`, `FlexibleTimeWindow`, `ActionAfterCompletion`, `CreationDate`, `LastModificationDate`. Unknown name/group => `ResourceNotFoundException` (404).
4. **UpdateSchedule** — `PUT /schedules/{Name}`. Full-replace semantics (AWS requires `ScheduleExpression`, `Target`, `FlexibleTimeWindow` on every update, and fields omitted from the request are CLEARED, not merged). Must bump `LastModificationDate`, preserve `CreationDate`, return the same `ScheduleArn`, and 404 if the schedule does not exist. A following GetSchedule must show the new values.
5. **ListSchedules** — `GET /schedules`. Must enumerate persisted schedules as `ScheduleSummary` (`Arn`, `Name`, `GroupName`, `State`, `CreationDate`, `LastModificationDate`, `Target` summary) and honour the filters `ScheduleGroup` (GroupName), `NamePrefix`, `State`, plus `MaxResults` (1..100) with an opaque `NextToken` that resumes where the previous page ended. `Schedules` is required and must be present (empty list, not absent, when nothing matches).
6. **DeleteSchedule** — `DELETE /schedules/{Name}?groupName=&clientToken=`. Must remove the record from state; a subsequent GetSchedule must return `ResourceNotFoundException` (404), and ListSchedules must no longer include it. Deleting an unknown schedule => 404.

Every one of these must compute its response from persisted state. Hardcoded/canned outputs, echoing the request back, or minting an ARN without storing the resource are failures of this issue: a CreateSchedule followed by GetSchedule in a separate request must return exactly what was created, and after DeleteSchedule the resource must be observably gone.

## State Model

A single struct in `src/state.rs` reachable via `crate::state::service_state(&*self.engine)` (the shared engine — never construct a new engine in a handler):

```
SchedulerState {
    // group name -> group; pre-seeded with the implicit "default" group
    groups: BTreeMap<String, ScheduleGroup>,
    // (group_name, schedule_name) -> schedule
    schedules: BTreeMap<(String, String), Schedule>,
}

ScheduleGroup {
    name: String,
    arn: String,                 // arn:aws:scheduler:<region>:<acct>:schedule-group/<name>
    state: String,               // ACTIVE | DELETING
    creation_date: SystemTime,
    last_modification_date: SystemTime,
    tags: Vec<Tag>,              // key/value, used later by Tag/Untag/ListTagsForResource
}

Schedule {
    name: String,
    group_name: String,
    arn: String,                 // arn:aws:scheduler:<region>:<acct>:schedule/<group>/<name>
    schedule_expression: String,
    schedule_expression_timezone: Option<String>,
    start_date: Option<SystemTime>,
    end_date: Option<SystemTime>,
    description: Option<String>,
    state: String,               // ENABLED (default) | DISABLED
    kms_key_arn: Option<String>,
    action_after_completion: Option<String>,  // DELETE | NONE
    target: Target,              // Arn + RoleArn required, plus optional DeadLetterConfig,
                                 // RetryPolicy, Input, Ecs/EventBridge/Kinesis/SageMaker/Sqs params
    flexible_time_window: FlexibleTimeWindow, // Mode + optional MaximumWindowInMinutes
    creation_date: SystemTime,
    last_modification_date: SystemTime,
}
```

Pagination tokens for ListSchedules should encode the last returned `(group_name, name)` key so the ordered `BTreeMap` can resume deterministically.

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): `GetScheduleGroup`, `DeleteScheduleGroup`, `ListScheduleGroups`, `TagResource`, `UntagResource`, `ListTagsForResource`, cron/rate expression parsing and actual timed target invocation, `ServiceQuotaExceededException` limits, and ClientToken-based idempotency replay.

Done = a client script that creates a group, creates a schedule in it, gets it back and sees identical fields, updates it and sees the change plus a bumped `LastModificationDate`, lists it with a name prefix and a MaxResults page, deletes it, and then gets a 404 — all with correct response bodies, not just 2xx.

## Evidence

`services/service-com-amazonaws-scheduler-awschronosservice/OPERATIONS.md` lists all 12 operations with their routes, required members and errors: CreateSchedule (`POST /schedules/{Name}`, required `Name`/`ScheduleExpression`/`Target`/`FlexibleTimeWindow`, output required `ScheduleArn`), GetSchedule (`GET /schedules/{Name}`, full field set incl. `CreationDate`/`LastModificationDate`), UpdateSchedule (`PUT /schedules/{Name}`), DeleteSchedule (`DELETE /schedules/{Name}`), ListSchedules (`GET /schedules`, @paginated inputToken=NextToken/outputToken=NextToken/items=Schedules, filters `ScheduleGroup`/`NamePrefix`/`State`/`MaxResults` 1..100, required `Schedules`), CreateScheduleGroup (`POST /schedule-groups/{Name}`, output required `ScheduleGroupArn`). Error codes come from the same file: ResourceNotFoundException 404, ConflictException 409, ValidationException 400. Shape members for `Target` (Arn, RoleArn required), `FlexibleTimeWindow` (Mode required, MaximumWindowInMinutes 1..1440), `ScheduleSummary` and `ScheduleGroupSummary` were read from the Smithy model at `services/service-com-amazonaws-scheduler-awschronosservice/smithy-model/com-amazonaws-scheduler-awschronosservice.json`. `grep -n '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/*.rs` matches all 12 handler files (create_schedule.rs, get_schedule.rs, update_schedule.rs, delete_schedule.rs, list_schedules.rs, create_schedule_group.rs, get_schedule_group.rs, delete_schedule_group.rs, list_schedule_groups.rs, tag_resource.rs, untag_resource.rs, list_tags_for_resource.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI scheduler client at it.
2. `aws scheduler create-schedule-group --name demo-group` — expect 200 with a `ScheduleGroupArn` of the form arn:aws:scheduler:<region>:<account>:schedule-group/demo-group; today returns HTTP 501 NotImplemented.
3. `aws scheduler create-schedule --name demo --group-name demo-group --schedule-expression 'rate(5 minutes)' --flexible-time-window Mode=OFF --target '{"Arn":"arn:aws:sqs:us-east-1:123456789012:q","RoleArn":"arn:aws:iam::123456789012:role/r","Input":"{}"}'` — expect 200 with ScheduleArn arn:aws:scheduler:...:schedule/demo-group/demo.
4. `aws scheduler get-schedule --name demo --group-name demo-group` — expect the persisted schedule: same ScheduleExpression, Target.Arn/RoleArn/Input, FlexibleTimeWindow.Mode=OFF, State=ENABLED, CreationDate and LastModificationDate set.
5. `aws scheduler update-schedule --name demo --group-name demo-group --schedule-expression 'rate(10 minutes)' --flexible-time-window Mode=FLEXIBLE,MaximumWindowInMinutes=15 --target '{"Arn":"arn:aws:sqs:us-east-1:123456789012:q","RoleArn":"arn:aws:iam::123456789012:role/r"}' --state DISABLED`, then get-schedule again — expect rate(10 minutes), Mode=FLEXIBLE/15, State=DISABLED, Input cleared, CreationDate unchanged, LastModificationDate advanced.
6. `aws scheduler list-schedules --schedule-group demo-group --name-prefix de --max-results 1` — expect exactly the demo summary (Arn, Name, GroupName, State, dates, Target) and a NextToken only if more remain.
7. `aws scheduler delete-schedule --name demo --group-name demo-group` — expect 200, then `get-schedule` returns ResourceNotFoundException (HTTP 404) and `list-schedules --schedule-group demo-group` returns an empty `Schedules` list.
8. Re-run create-schedule-group with the same name — expect ConflictException (HTTP 409); create-schedule into a group that was never created — expect ResourceNotFoundException (HTTP 404).
