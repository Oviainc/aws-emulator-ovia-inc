Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# StartExperiment accepts a multi-account template that holds no target account configuration

_Filed: 2026-10-24T15:29:01Z_
_Operation: StartExperiment | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A template whose `experimentOptions.accountTargeting` is `multi-account` can only be run once it carries at least one target account configuration: the Smithy model's own documentation for CreateTargetAccountConfiguration states "A target account configuration is required when accountTargeting of experimentOptions is set to multi-account". Real FIS therefore refuses StartExperiment for such a template with ValidationException (HTTP 400) and mints no experiment id.

The emulator never relates the two. `src/ops/start_experiment.rs` loads the persisted template, checks the role trust policy, the active-experiment quota and the store ceilings, and then builds the experiment — it never reads `template.experiment_options.account_targeting` nor `template.target_account_configurations`. A multi-account template with an EMPTY configuration map therefore answers HTTP 200, the experiment is persisted with `experimentOptions.accountTargeting: multi-account` and `targetAccountConfigurationsCount: 0`, and ListExperimentTargetAccountConfigurations on it reports an empty page.

The converse relationship is equally unenforced: CreateTargetAccountConfiguration only checks that the template exists and that `roleArn` belongs to `accountId` (`src/ops/create_target_account_configuration.rs`), so a configuration can be attached to a `single-account` template, which real FIS refuses as well. Expected: a multi-account start with no configuration (and a configuration attached to a single-account template) is refused with ValidationException (400) and nothing is created. Actual: both answer 200 and persist.

## Evidence

Smithy model: CreateTargetAccountConfiguration documentation — "Creates a target account configuration for the experiment template. A target account configuration is required when accountTargeting of experimentOptions is set to multi-account."; StartExperiment declares ValidationException (client error, httpError 400); ExperimentTemplateExperimentOptions.accountTargeting is modelled `multi-account | single-account` (state.rs ACCOUNT_TARGETING_VALUES / DEFAULT_ACCOUNT_TARGETING = single-account).
Handler: services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/start_experiment.rs `handle` (lines ~166-320) — the only refusals are resolve_actions_mode, template lookup, role_trust_refusal, MAX_ACTIVE_EXPERIMENTS and MAX_EXPERIMENTS; nothing consults accountTargeting or the template's target_account_configurations map.
Tests: tests/all/experiment_lifecycle.rs `template_cases()` contains a template with `experimentOptions: {accountTargeting: "multi-account", ...}` and NO target account configuration (lines ~380-410), and `an_experiment_carries_the_persisted_template_and_runs_to_the_status_its_actions_earn` (line ~904) starts every one of those templates asserting HTTP 200 — so the current accepting behavior is pinned by a test rather than refused. No test asserts the multi-account/configuration relationship in either direction.

## Steps to Reproduce

1. Boot the emulator and point a botocore `fis` client at it.
2. Call create_experiment_template with experimentOptions={'accountTargeting': 'multi-account'}, a wait action and stopConditions=[{'source':'none'}] — note the template id; do NOT create any target account configuration.
3. Call start_experiment(clientToken=..., experimentTemplateId=<that id>) — observe HTTP 200 and an experiment whose experimentOptions.accountTargeting is 'multi-account' and targetAccountConfigurationsCount is 0, instead of a 400 ValidationException naming the missing target account configuration.
4. Call get_experiment on the returned id — observe the experiment exists and runs to a terminal status.
5. Converse: create a template with experimentOptions={'accountTargeting': 'single-account'} and call create_target_account_configuration(experimentTemplateId=<it>, accountId='111122223333', roleArn='arn:aws:iam::111122223333:role/x') — observe HTTP 200 for a configuration real FIS refuses on a single-account template.
