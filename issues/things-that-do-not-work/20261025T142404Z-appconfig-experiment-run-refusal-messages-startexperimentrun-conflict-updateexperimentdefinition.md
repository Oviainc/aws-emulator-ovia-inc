Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.experiment-run-managed-computation-not-client-controlled appconfig.update-experiment-definition-while-running-rejected appconfig.get-experiment-run-invalid-id-not-found
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.experiment-run-managed-computation-not-client-controlled|run#21|AssertionError|assert err["Message"] == conflict, f"expected message {conflict!r}, got {err['Message']!r}"
Issue-Key: appconfig.update-experiment-definition-while-running-rejected|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.get-experiment-run-invalid-id-not-found|run#16|AssertionError|assert code_msg == (
Issue-Commit: 5ed22e125380c04fb8121d8e25f405dce5415572

# AppConfig experiment-run refusal messages (StartExperimentRun conflict, UpdateExperimentDefinition conflict, GetExperimentRun not-found) do not use AWS's request-derived templates (3 causes)

> _Note: this issue was identified against the code base at commit `5ed22e125380c04fb8121d8e25f405dce5415572`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExperimentRun

## Description
Three distinct root causes, all of the same fault class and each grounded in its own replayed behavioral check: on the experiment-run paths the HTTP status and the error CODE already match AWS, and the only divergence is the refusal's Message, which is an emulator-invented sentence instead of AppConfig's request-derived template. These templates are rule-determined, not incidental recorded text: each is assembled from fixed text plus values taken from the request (application identifier, experiment definition identifier, run number) and the account id. This is the same class the open issue 20261024T131534Z-appconfig-refusal-messages-on-the-tagging-pagination-hosted.md covers, but that issue names only TagResource/UntagResource/ListTagsForResource, ListExtensions/ListDeploymentStrategies pagination, CreateHostedConfigurationVersion's stale-version conflict, CreateExperimentDefinition's duplicate-name conflict and UpdateExtension's length validation — none of the three operations below, which are therefore NOT covered by it. Numbered; close ALL THREE before submitting.

=== Cause 1 of 3: StartExperimentRun's already-running conflict message ===
AWS: a second StartExperimentRun on a definition whose previous run is still RUNNING is refused with ConflictException, HTTP 409, and the template 'Experiment definition <ExperimentDefinitionIdentifier> already has a running experiment. The current experiment run must be stopped before a run can be started.' — fixed text around the definition id taken from the request; the active run's NUMBER and STATUS are not disclosed. Smithy: StartExperimentRun POST /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}/experimentruns, success code 201, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500; the sentence itself is recorded behavior only, but it is built from the request's own definition identifier.
Emulator: the correct 409 ConflictException, but the message is 'Experiment definition <defId> already has an active experiment run: run <N> is <STATUS>.' — a different sentence shape that additionally reveals the active run's number and status, which AWS's refusal withholds.
Why: a client that matches or displays AppConfig's refusal text (error classifiers, log scrapers, snapshot tests) does not match the emulator's sentence, and logic written against the emulator can parse a run number and status out of the refusal that simply are not there in production.
Fix: the already-running guard in StartExperimentRun's handler must render AWS's template from the request's experiment definition identifier only, without interpolating state about the active run.
Traces: appconfig.experiment-run-managed-computation-not-client-controlled (everything else in that check passed: the server-assigned Run/Status/StartedAt, the snapshot keys and the ConflictException code were all correct).

=== Cause 2 of 3: UpdateExperimentDefinition's active-run conflict message ===
AWS: UpdateExperimentDefinition against a definition with an active RUNNING run is refused with ConflictException, HTTP 409, and the template 'Cannot update experiment definition <ExperimentDefinitionIdentifier> with status <Status>. Stop the current experiment run before updating the experiment definition.' — fixed text 'Cannot update experiment definition ' / ' with status ' / '. Stop the current experiment run before updating the experiment definition.' around the request's definition identifier and the definition's current status as held in state (ACTIVE in this situation). Smithy: UpdateExperimentDefinition PATCH /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}, success code 200, errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500; the sentence is recorded behavior only, built from the request's identifier and the stored status.
Emulator: the correct 409 ConflictException, but the message is 'Experiment definition <defId> cannot be updated while an experiment run is active.' — a different sentence shape which, unlike AWS's, does not name the definition's status at all.
Why: AWS's text tells the caller WHICH status blocked the update, so a client can distinguish this conflict from other lifecycle refusals; the emulator's sentence is one AppConfig never emits and drops the status that makes the refusal actionable.
Fix: the active-run guard in UpdateExperimentDefinition's handler must render AWS's template from the request's definition identifier and the definition's stored status.
Traces: appconfig.update-experiment-definition-while-running-rejected (the ConflictException code matched, the rejected update correctly left Hypothesis untouched, and the post-stop positive control update succeeded).

=== Cause 3 of 3: GetExperimentRun's not-found message template ===
AWS: GetExperimentRun for a run number that never existed under an existing definition is refused with ResourceNotFoundException, HTTP 404, and the template 'Experiment run with Experiment Definition Id <defId>, Application Id <appId>, and Experiment Run <Run> could not be found for account <accountId>.' — note the field ORDER (definition id first, then application id, then run), the lowercase word 'run' in 'Experiment run', and the labels 'Experiment Definition Id' / 'Application Id' / 'Experiment Run'; the three values come from the request and the account id from the caller's identity. Smithy: GetExperimentRun GET /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}/experimentruns/{Run}, success code 200, errors BadRequestException 400 / ResourceNotFoundException 404 / InternalServerException 500; the sentence is recorded behavior only, but is fully request-derived.
Emulator: the correct 404 ResourceNotFoundException, but the message is 'Experiment Run with Application Identifier <appId>, Experiment Definition Identifier <defId>, and Run <N> could not be found for account <accountId>.' — the fixed text differs in three ways at once: 'Experiment Run' instead of 'Experiment run', the labels 'Application Identifier'/'Experiment Definition Identifier'/'Run' instead of AWS's labels, and the application and definition values in the opposite order.
Why: this is the message callers key not-found handling and fixtures on; an emulator-specific label set and field order means text written against AppConfig never matches, while the status and code look right.
Fix: the not-found path of GetExperimentRun must render AWS's template (label wording, field order and casing) from the request's definition identifier, application identifier and run number plus the account id.
Traces: appconfig.get-experiment-run-invalid-id-not-found (the positive control — fetching the real run number and getting Status RUNNING — passed, and the error code matched).

## Evidence
Behavioral replay (simparity-behavioral-ga-prod-v1). In all three causes the HTTP status and the error code matched and only $.Message diverged. The concrete ids, run numbers and the account ('000000000000') below are the run's values and are examples; the requirement is each cause's template.

Cause 1 — appconfig.experiment-run-managed-computation-not-client-controlled [mismatch]:
AssertionError: expected message 'Experiment definition 5w5gjbo already has a running experiment. The current experiment run must be stopped before a run can be started.', got 'Experiment definition 5w5gjbo already has an active experiment run: run 1 is RUNNING.'
(Code ConflictException matched; the rejected attempt correctly created no second run.)

Cause 2 — appconfig.update-experiment-definition-while-running-rejected [mismatch]:
AssertionError: expected message 'Cannot update experiment definition 5w5gjbo with status ACTIVE. Stop the current experiment run before updating the experiment definition.', got 'Experiment definition 5w5gjbo cannot be updated while an experiment run is active.'
(Code ConflictException matched; Hypothesis was correctly left unchanged by the rejected call.)

Cause 3 — appconfig.get-experiment-run-invalid-id-not-found [mismatch]:
AssertionError: unexpected message: 'Experiment Run with Application Identifier ho65pb1, Experiment Definition Identifier 5w5gjbo, and Run 10000 could not be found for account 000000000000.' — expected 'Experiment run with Experiment Definition Id 5w5gjbo, Application Id ho65pb1, and Experiment Run 10000 could not be found for account 000000000000.'
(Code ResourceNotFoundException matched.)

## Steps to Reproduce
Point an appconfig client at the emulator's data listener and compare only the Message of each refusal (status and code already match). Common setup: create_application, create_environment, an AWS.AppConfig.FeatureFlags configuration profile with a deployed flag, and create_experiment_definition(...) -> <defId>.

Cause 1: start_experiment_run(ApplicationIdentifier=<appId>, ExperimentDefinitionIdentifier=<defId>) -> Run 1 RUNNING; call start_experiment_run again with the identical arguments -> 409 ConflictException; expect 'Experiment definition <defId> already has a running experiment. The current experiment run must be stopped before a run can be started.' and no run number/status in the text.

Cause 2: with that run still RUNNING, update_experiment_definition(ApplicationIdentifier=<appId>, ExperimentDefinitionIdentifier=<defId>, Hypothesis='changed hypothesis') -> 409 ConflictException; expect 'Cannot update experiment definition <defId> with status ACTIVE. Stop the current experiment run before updating the experiment definition.'

Cause 3: get_experiment_run(ApplicationIdentifier=<appId>, ExperimentDefinitionIdentifier=<defId>, Run=<realRun+9999>) -> 404 ResourceNotFoundException; expect 'Experiment run with Experiment Definition Id <defId>, Application Id <appId>, and Experiment Run <run> could not be found for account <accountId>.' (definition id before application id, 'Experiment run' with a lowercase 'run').

Re-replay every check in trace_ids to confirm all three causes are closed.
