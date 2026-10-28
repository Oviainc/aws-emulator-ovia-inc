Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# A template whose actions' startAfter form a dependency CYCLE is accepted and started, although the crate states FIS refuses such a graph and its own run planner only defends against it

_Filed: 2026-10-28T01:39:39Z_
_Operation: CreateExperimentTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`actions_refusal` is this crate's one admissibility sequence for an experiment template's actions (both `CreateExperimentTemplate` and `UpdateExperimentTemplate` judge their record through it). The only rule it applies to `startAfter` is that each named action is one the SAME template declares. It never checks that the resulting dependency graph is acyclic, so a template with `actions.a.startAfter = ["b"]` and `actions.b.startAfter = ["a"]` passes the whole sequence, is persisted, is reported by `GetExperimentTemplate`, and is started by `StartExperiment`.

The crate itself says that is not a template the service should hold. `critical_path`'s own doc calls a cycle something "which FIS refuses but which a record could still carry", and the run planner only DEFENDS against it ("broken at the back edge rather than recursing forever"); the crate's test suite calls the same shape "a `startAfter` cycle no FIS template could carry" and "not a graph FIS would accept". The admissibility sequence's own stated purpose is that "a template a call accepts is one the service would run" — a cycle is precisely a template the service could not run, because every action in it must wait for another action in it to complete first.

Two observable consequences: (1) `CreateExperimentTemplate`/`UpdateExperimentTemplate` answer 200 for a template that describes an impossible run instead of the `ValidationException` the sequence exists to produce; (2) `StartExperiment` then mints an experiment whose run span is computed off a graph with a back edge broken arbitrarily — in the two-action example above the run is planned as if `b` ran before `a` (a 2-minute chain) purely because of which key the walk reached first, so the reported `startTime`/`endTime` span describes a schedule no caller asked for.

This is not a request for a new feature: the check belongs in the sequence the crate already has, beside the `startAfter`-names-a-declared-action rule it already applies two lines away, and the defensive cycle break in `critical_path` (which the test pins) stays exactly as it is.

## Evidence

The crate's own statement that FIS refuses such a graph, in the run planner that merely defends against it — `services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:4141-4144`:
```
/// entries) — linear in the request, never in its square. The number of actions is capped
/// at [`MAX_ACTIONS_PER_TEMPLATE`] when the template is created, which also bounds the
/// recursion depth here; a `startAfter` cycle, which FIS refuses but which a record could
/// still carry, is broken at the back edge rather than recursing forever.
```
and the break itself — `src/state.rs:4177-4179`:
```
    if walking.contains(&name) {
        return Duration::ZERO;
    }
```

The ONLY `startAfter` rule the admissibility sequence applies — `src/state.rs:3660-3670`:
```
    for (name, action) in entries {
        if action
            .start_after
            .iter()
            .flatten()
            .any(|earlier| !actions.contains_key(earlier))
        {
            return Some(invalid_input(format!(
                "actions.{name}.startAfter must name actions the template declares."
            )));
        }
```
and the doc above it, which states the member's meaning without the acyclicity it implies — `src/state.rs:3636-3637`:
```
/// * `startAfter` is "the name of the action that must be completed before the current
///   action starts", so it names an action the same template declares;
```

The stated purpose of the sequence that lets the cycle through — `src/state.rs:3948-3954`:
```
    // An action FIS publishes no action for, one missing a parameter its published contract
    // requires, one naming a target the template does not declare and a wait beyond the
    // documented bound are each refused HERE rather than persisted: a template a call
    // accepts is one the service would run.
    if let Some(refusal) = actions_refusal(&template.actions, &template.targets) {
        return Some(TemplateRefusal::Invalid(refusal));
    }
```
reached from the handler at `src/ops/create_experiment_template.rs:200` (`if let Some(refusal) = crate::state::template_refusal(&record)`) and from `src/ops/update_experiment_template.rs:211` for the update candidate.

The crate's tests say the same thing about the shape, while only pinning that the PLAN is bounded (not that a template may carry a cycle) — `tests/all/control_plane_rules.rs:799-801`:
```
    // The rows differ along the shape of the action graph (one action, several in
    // parallel, several chained through `startAfter`, no actions at all, a `startAfter`
    // cycle no FIS template could carry), along WHICH actions carry the duration (a wait,
```
and `tests/all/control_plane_rules.rs:888-897`:
```
        (
            // A cycle is not a graph FIS would accept; what matters is that the plan is
            // BOUNDED rather than endless, which this row asserts by answering at all.
            "a startAfter cycle",
            vec![
                ("a", "aws:fis:wait", Some("PT1M"), vec!["b"]),
                ("b", "aws:fis:wait", Some("PT1M"), vec!["a"]),
            ],
            0,
        ),
```
That row calls `run_plan` directly with a hand-built action map, so refusing the cycle at create/update time leaves it passing unchanged. The sequence's own refusal table, `tests/all/template_admissibility_sequence.rs:177-187`, covers only "a startAfter naming an action the template does not declare" — no row covers a cycle, so nothing pins its acceptance.

## Steps to Reproduce

1. POST /experimentTemplates with a valid description, roleArn, one stop condition, and actions = {"a": {"actionId": "aws:fis:wait", "parameters": {"duration": "PT1M"}, "startAfter": ["b"]}, "b": {"actionId": "aws:fis:wait", "parameters": {"duration": "PT1M"}, "startAfter": ["a"]}}.
2. Observe HTTP 200: the template is created and GET /experimentTemplates/{id} reports both actions with their mutual startAfter, although neither action can ever start.
3. POST /experiments with that template id — observe HTTP 200 and an experiment that proceeds initiating -> running -> completed, its run span planned off the cycle with one back edge silently dropped (src/state.rs:4177-4179).
4. Compare with the sibling rule two lines away: the same create with actions = {"a": {... "startAfter": ["missing"]}} is refused 400 ValidationException "actions.a.startAfter must name actions the template declares." (src/state.rs:3667-3669), the refusal the crate's own text says a cycle should also earn (src/state.rs:4143, tests/all/control_plane_rules.rs:889).
5. Repeat via PATCH /experimentTemplates/{id} with the same cyclic actions map — the update judges the candidate through the same sequence (src/ops/update_experiment_template.rs:211) and likewise answers 200.
