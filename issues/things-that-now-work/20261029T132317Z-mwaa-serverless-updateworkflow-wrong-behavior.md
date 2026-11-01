Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a1e71609e4dd8609a7a875072f755c7dbeedcd71

# A punctuation-only cron field is accepted, so a definition declaring no cron schedule is reported with a ScheduleConfiguration and given an automated trigger

_Filed: 2026-10-29T13:23:17Z_
_Operation: UpdateWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a1e71609e4dd8609a7a875072f755c7dbeedcd71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The definition reader decides whether a declared `schedule` value is a cron expression with `cron_expression()` -> `is_cron_field()`. `is_cron_field` splits the field on the cron punctuation set (`*`, `?`, `,`, `-`, `/`, `#`) and accepts a token when `token.is_empty()`. Because every one of those characters is a separator, a field made ONLY of punctuation splits into nothing but empty tokens and is therefore accepted, although it carries none of the four things the function's own doc says a field may be left with (numbers, wildcards, the day-of-month qualifiers, the month/day-of-week names).

Consequence: a definition declaring `schedule: "- - - - -"` (equally `", , , , ,"`, `"/ / / / /"`, `"# # # # #"`, `"*/ * * * *"`) is five "valid" fields, so `cron_expression` returns the value verbatim and `declared_schedule` reports `ScheduleConfiguration { CronExpression: "- - - - -" }`. That snapshot is frozen on the version, which makes `Workflow::is_scheduled()` true, so `promote_newest_version` -> `configure_trigger` arms the automated trigger and `fire_due_scheduled_runs` starts minting `RunType=SCHEDULED` runs every `SCHEDULED_RUN_EVERY` for a workflow whose definition declares no schedule at all. `GetWorkflow` and `ListWorkflowVersions` additionally report a `CronExpression` that is not a cron expression.

This contradicts the reader's own stated rule, and it is the exact outcome the crate's own tests pin for the sibling case (prose with five words reports no schedule and produces no automated run). The handler cited is `UpdateWorkflow`, whose own module doc makes the claim this breaks ("an update onto a definition that declares a schedule, or off one, starts or stops the runs the service produces on its own"); `CreateWorkflow` reaches the same reader through the same call.

## Evidence

The rule the reader states for itself — `services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state/workflow_definition.rs:241`:
```
/// `None` when the definition declares no schedule, or declares one that is not a cron
/// expression at all: `ScheduleConfiguration$CronExpression` is a cron expression in
/// standard cron syntax, so a value that is not one is not reported as one.
```
and `workflow_definition.rs:269`:
```
/// Whether `field` is written as ONE cron field: taking its structure apart
/// ([`CRON_FIELD_PUNCTUATION`]) leaves only numbers, wildcards, the day-of-month
/// qualifiers ([`CRON_FIELD_QUALIFIERS`]) and the month and day-of-week names
/// ([`CRON_FIELD_NAMES`]) a cron field is allowed to carry.
```

The code under it — `workflow_definition.rs:273`:
```
fn is_cron_field(field: &str) -> bool {
    field
        .split(|character| CRON_FIELD_PUNCTUATION.contains(&character))
        .all(|token| {
            token.is_empty()
                || token.chars().all(|character| character.is_ascii_digit())
                || CRON_FIELD_QUALIFIERS
                    .iter()
                    .chain(CRON_FIELD_NAMES)
                    .any(|name| token.eq_ignore_ascii_case(name))
        })
}
```
Every separator is in the split set (`workflow_definition.rs:128`: `const CRON_FIELD_PUNCTUATION: &[char] = &['*', '?', ',', '-', '/', '#'];`), so `"-"` splits into `["", ""]`, both tokens are empty, and the field passes. `cron_expression` then returns the value as written (`workflow_definition.rs:265`):
```
    (CRON_FIELD_COUNTS.contains(&fields.len()) && fields.iter().copied().all(is_cron_field))
        .then(|| declared.to_string())
```

What the accepted value then drives — `src/state.rs:752`:
```
    pub fn is_scheduled(&self) -> bool {
        self.current_snapshot()
            .is_some_and(|snapshot| snapshot.schedule_configuration.is_some())
    }
```
and `src/state.rs:768`:
```
    pub fn configure_trigger(&mut self, now: SystemTime) {
        self.next_scheduled_run_at = self.is_scheduled().then(|| now + SCHEDULED_RUN_EVERY);
    }
```
reached from `src/ops/update_workflow.rs:227` (`snapshot_definition(...)`) and `:346` (`workflow.promote_newest_version(now)`), and from `src/ops/create_workflow.rs:274`/`:445`.

The sibling case the crate already pins the opposite way — `tests/all/workflow_definition_snapshot.rs:348`:
```
        case: "prose with as many words as a cron expression has fields",
        ...
        definition: "schedule: run it every other day\n",
        cron: None,
```
and `tests/all/definition_declared_schedule_trigger.rs:99`:
```
        case: "a schedule value that is not a cron expression",
        ...
        definition: Some("schedule: whenever you like\ntasks: []\n"),
        cron: None,
```
with `definition_declared_schedule_trigger.rs:199` tying the two together: `let automated = row.cron.is_some();`. No existing test covers a field made only of punctuation, so nothing pins the current acceptance.

## Steps to Reproduce

1. Stage a workflow definition object in the emulated S3 whose content is `schedule: "- - - - -"\ntasks:\n  - id: extract\n`.
2. Call `CreateWorkflow` (or `UpdateWorkflow` onto that object) naming that bucket/key in `DefinitionS3Location`.
3. Call `GetWorkflow` for the workflow: `ScheduleConfiguration.CronExpression` is reported as `"- - - - -"`, although that value is not a cron expression in standard cron syntax — contrast `schedule: run it every other day`, which `tests/all/workflow_definition_snapshot.rs:348` pins as reporting no `ScheduleConfiguration` at all.
4. Wait past `SCHEDULED_RUN_EVERY` and call `ListWorkflowRuns` for the workflow: runs with `RunType=SCHEDULED` have been minted by the automated trigger, although the definition declares no schedule the service could configure a trigger from.
5. Call `ListWorkflowVersions`: the version summary also carries the same non-cron `ScheduleConfiguration.CronExpression`.
