Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# experimentOptions.actionsMode=skip-all and the template's emptyTargetResolutionMode are echoed back but never affect the run

_Filed: 2026-10-24T01:20:07Z_
_Operation: StartExperiment | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartExperiment validates and persists `experimentOptions.actionsMode`, and the experiment inherits the template's `experimentOptions.emptyTargetResolutionMode`, but NEITHER value is consulted when the run is planned, so both are observable only as echoed members.

1. `actionsMode: skip-all` — FIS documents skip-all as "skip all actions", which is how a caller verifies target resolution WITHOUT performing the actions. An experiment started that way does not execute its actions, so it does not spend the actions' durations and cannot be failed by the execution role's missing action permissions. In the emulator `ExperimentRecord::from_template` calls `run_plan(&template.actions, failure)` and never looks at the `actions_mode` argument (it only copies it into `experiment_options`): the skip-all experiment is given the same critical-path run span computed from the actions' `duration` parameters as a run-all experiment, and the same permission-derived `failed` outcome. Expected: a skip-all experiment reaches `completed` without the actions being carried out (and without a privilege failure attributed to an action nothing ran). Actual: the run is indistinguishable from run-all apart from the echoed option.

2. `emptyTargetResolutionMode` — FIS documents `fail` (the default this emulator already applies, see DEFAULT_EMPTY_TARGET_RESOLUTION_MODE) as: the experiment fails when a target resolves to zero resources; `skip` skips the action instead. `resolve_targets` can return an empty resolution (a target identified by `resourceTags` resolves to nothing — see the open ListExperimentResolvedTargets issue), yet `run_plan` only ever distinguishes "a permission failure was found" from "completed". An experiment whose target resolved to nothing therefore reports `completed` under `fail`, and reports exactly the same thing under `skip`, so the two documented modes are behaviorally identical.

## Evidence

Smithy model: StartExperiment.experimentOptions -> StartExperimentExperimentOptionsInput.actionsMode -> ActionsMode (`run-all` | `skip-all`); CreateExperimentTemplateExperimentOptionsInput.emptyTargetResolutionMode -> EmptyTargetResolutionMode (`fail` | `skip`), both surfaced on ExperimentOptions. Well-established FIS behavior (FIS User Guide, experiment options): skip-all skips all actions so you can test target resolution without running the actions; emptyTargetResolutionMode=fail fails the experiment when a target resolves to zero resources, skip skips the action.
Handler/state: src/ops/start_experiment.rs lines 171-181 (validates the enum) and 289-299 (passes `actions_mode` to `from_template`); src/state.rs `ExperimentRecord::from_template` line 866 `let (run_span, outcome) = run_plan(&template.actions, failure);` — `actions_mode` is used only at line 902 to fill `experiment_options.actions_mode`; `run_plan` (src/state.rs ~1905) branches solely on the pre-computed permission `failure`; `resolve_targets` (~1306) returns an empty Vec for a tag-identified target and no caller inspects emptiness.
Tests: tests/all/experiment_run.rs::two_experiments_from_one_template_run_independently starts one run-all and one skip-all experiment from the same template, asserts only that the mode is echoed, and then asserts the skip-all experiment is still `running` — i.e. the existing test enshrines the divergence rather than covering it. tests/all/experiment_lifecycle.rs asserts only that emptyTargetResolutionMode round-trips (lines 590-629) and that an unmodelled value is refused (line 1437); no test exercises an empty resolution.

## Steps to Reproduce

1. Boot the emulator and point a botocore `fis` client at it.
2. create_experiment_template with one action whose `duration` is PT5M (e.g. aws:fis:inject-api-internal-error) and a role FIS may assume.
3. start_experiment(experimentTemplateId=tpl, experimentOptions={'actionsMode': 'skip-all'}) and poll get_experiment — observe the experiment spend the same compressed run span as a run-all start before reaching a terminal status, instead of completing without performing the actions.
4. Repeat with an execution role that lacks the action's permission and actionsMode='skip-all' — observe the experiment end `failed` with a privilege reason even though no action was to be run.
5. create_experiment_template with experimentOptions={'emptyTargetResolutionMode': 'fail'} (or omitted, which defaults to fail) and a target identified by resourceTags, start it and poll get_experiment — observe `completed` even though list_experiment_resolved_targets reports no resolved target; repeat with 'skip' and observe an identical outcome.
