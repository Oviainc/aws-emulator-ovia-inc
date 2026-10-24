Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# UpdateExperimentTemplate accepts a template FIS refuses: actions/stopConditions emptied, and a logConfiguration naming no destination

_Filed: 2026-10-24T15:29:01Z_
_Operation: UpdateExperimentTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An update is judged by exactly the refusals the create applies (`role_arn_refusal`, `stop_conditions_refusal`, `template_experiment_options_refusal`, `actions_quota_refusal`, `actions_refusal`), and none of them enforces the shape rules FIS applies to the RESULTING template:

1. `actions` replaced by an EMPTY map (`{"actions": {}}`) is accepted, leaving a stored template that declares no action at all. FIS requires an experiment template to declare at least one action (`actions` is @required on the create and an experiment with no action cannot run); such a request is refused with ValidationException (400). The emulator answers 200, GetExperimentTemplate reports `actions: {}`, and StartExperiment then happily runs the actionless template to `completed`.
2. `stopConditions` replaced by an EMPTY list (`{"stopConditions": []}`) is accepted, leaving a template with no stop condition — FIS requires at least one entry (the explicit `{"source": "none"}` is how "no alarm" is expressed, which is why the member is @required on the create). `state.rs::stop_conditions_refusal` iterates the list and returns None for an empty one.
3. `logConfiguration` is accepted when it names NEITHER destination. `ExperimentTemplateRecord::updated_log_configuration` only requires a `logSchemaVersion` (taken from the stored configuration when the request omits it) and then stores `cloudWatchLogsConfiguration: None, s3Configuration: None`, so the template is reported back carrying a logConfiguration with a schema version and no destination — a representation FIS does not have, since a log configuration must name a CloudWatch Logs group or an S3 bucket.

Expected: each of these requests is refused with ValidationException (HTTP 400) and the stored template is left untouched. Actual: HTTP 200 with the degenerate template persisted and readable through GetExperimentTemplate.

## Evidence

Smithy model: UpdateExperimentTemplate declares ValidationException (HTTP 400); CreateExperimentTemplateRequest marks `actions` and `stopConditions` @required while the lists/maps carry no @length, so the "at least one" rules are server-side; CreateExperimentTemplateLogConfigurationInput makes both destinations optional and only logSchemaVersion required, so "at least one destination" is likewise a server-side rule (FIS User Guide: experiment logging is configured to CloudWatch Logs or Amazon S3).
Handler/state: src/ops/update_experiment_template.rs lines ~170-221 (the five refusals consulted, then the candidate is committed); src/state.rs `ExperimentTemplateRecord::apply_update` (replaces `actions`/`stopConditions` whole with whatever the request carries), `stop_conditions_refusal` (loops the slice — an empty slice refuses nothing), `actions_refusal` (loops the map — an empty map refuses nothing), `updated_log_configuration` (checks only logSchemaVersion).
Tests: tests/all/update_experiment_template.rs pins a "targets emptied" row at HTTP 200 and its refusal table covers only stop-condition source/value, the experimentOptions enum, the wait duration bounds and the actions quota — no row empties `actions` or `stopConditions`, and the "no stored log configuration" test asserts only that a MISSING logSchemaVersion is refused, never that a destination is required.

## Steps to Reproduce

1. Boot the emulator and point a botocore `fis` client at it; create a template with one aws:fis:wait action, stopConditions=[{'source':'none'}] and a logConfiguration naming a CloudWatch Logs group.
2. Call update_experiment_template(id=<it>, actions={}) — observe HTTP 200 and get_experiment_template reporting `actions: {}`; then start_experiment from that template and observe it runs.
3. Call update_experiment_template(id=<it>, stopConditions=[]) — observe HTTP 200 and a stored template with no stop condition.
4. Call update_experiment_template(id=<it>, logConfiguration={'logSchemaVersion': 2}) — observe HTTP 200 and a stored logConfiguration carrying neither cloudWatchLogsConfiguration nor s3Configuration.
