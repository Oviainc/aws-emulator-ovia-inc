Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# Experiment logging cannot be switched off: an update can never remove a template's logConfiguration

_Filed: 2026-10-24T15:29:01Z_
_Operation: UpdateExperimentTemplate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

FIS exposes no DeleteLogConfiguration operation — UpdateExperimentTemplate is the only way to stop an experiment template from logging, by sending an empty `logConfiguration` (every member of UpdateExperimentTemplateLogConfigurationInput is optional precisely so the member can be sent empty). After such an update the template is reported with NO `logConfiguration` member at all.

The emulator cannot reach that state. `ExperimentTemplateRecord::apply_update` treats a present `logConfiguration` as "replace", and `updated_log_configuration` fills `logSchemaVersion` in from the stored configuration when the request omits it, so `logConfiguration: {}` leaves `Some(ExperimentTemplateLogConfiguration { cloud_watch_logs_configuration: None, s3_configuration: None, log_schema_version: Some(<stored>) })` on the record: the response and every later GetExperimentTemplate still carry a `logConfiguration`, now naming no destination. For a template that never had one, the same request is refused with 400 ("logConfiguration.logSchemaVersion is required: the template carries no log configuration to take it from."). Expected: an empty logConfiguration clears the member, and a later read reports the template without `logConfiguration` (and experiments started afterwards carry none). Actual: the member is never removable, and the template is left in a destination-less state instead.

## Evidence

Smithy model: the service declares no operation that deletes a log configuration (26 operations; UpdateExperimentTemplate is the only mutator of a template's logConfiguration), and UpdateExperimentTemplateLogConfigurationInput declares all three members optional, unlike CreateExperimentTemplateLogConfigurationInput where logSchemaVersion is @required — the looser update shape is what makes an empty logConfiguration expressible at all.
Handler/state: src/ops/update_experiment_template.rs calls `candidate.apply_update(&input, now)`; src/state.rs `ExperimentTemplateRecord::apply_update` (`if let Some(configuration) = input.log_configuration.as_ref() { self.log_configuration = Some(self.updated_log_configuration(configuration)?) }`) can only ever set `Some(...)`, never `None`; `updated_log_configuration` inherits the stored logSchemaVersion.
Tests: tests/all/update_experiment_template.rs exercises log configuration replacement (with and without a schema version) and the no-stored-configuration refusal, but no case sends an empty logConfiguration or asserts that the member can be removed.

## Steps to Reproduce

1. Boot the emulator and create an experiment template with logConfiguration={'logSchemaVersion': 2, 'cloudWatchLogsConfiguration': {'logGroupArn': 'arn:aws:logs:us-east-1:000000000000:log-group:/fis:*'}}.
2. Call update_experiment_template(id=<it>, logConfiguration={}) — observe HTTP 200 and a response whose experimentTemplate still contains `logConfiguration` ({'logSchemaVersion': 2}) instead of the member being gone.
3. Call get_experiment_template(id=<it>) — observe the destination-less logConfiguration is persisted; there is no request that removes it.
4. Repeat on a template created WITHOUT a logConfiguration: update_experiment_template(id=<it>, logConfiguration={}) answers 400 ValidationException.
