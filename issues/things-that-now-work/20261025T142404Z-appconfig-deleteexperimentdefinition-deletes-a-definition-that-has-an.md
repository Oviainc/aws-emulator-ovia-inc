Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.delete-experiment-definition-with-active-run-conflict-rejected
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.delete-experiment-definition-with-active-run-conflict-rejected|_client_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 5ed22e125380c04fb8121d8e25f405dce5415572

# AppConfig DeleteExperimentDefinition deletes a definition that has an ACTIVE experiment run instead of refusing it

> _Note: this issue was identified against the code base at commit `5ed22e125380c04fb8121d8e25f405dce5415572`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteExperimentDefinition

## Description
One root cause (Cause 1 of 1), grounded in a replayed behavioral check: DeleteExperimentDefinition has no active-run guard, so it destroys an experiment definition whose run is still RUNNING/ACTIVE where AppConfig refuses the call.

=== Cause 1 of 1: DeleteExperimentDefinition is not guarded by the definition's active run ===
AWS: with an experiment run started via StartExperimentRun and the definition therefore reporting Status=ACTIVE, DeleteExperimentDefinition (with DeleteType=DESTROY) is refused with BadRequestException, HTTP 400, and the request-derived template 'Experiment definition <ExperimentDefinitionIdentifier> cannot be deleted while an experiment is running.' — fixed text 'Experiment definition ' / ' cannot be deleted while an experiment is running.' around the definition id taken from the request. The definition is NOT deleted: a following GetExperimentDefinition still returns it with Status=ACTIVE. Once StopExperimentRun has moved the run to DONE (definition back to IDLE), the identical delete succeeds and cascades, so GetExperimentDefinition and GetExperimentRun then both answer ResourceNotFoundException. Smithy: DeleteExperimentDefinition DELETE /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}, success code 204, modeled errors BadRequestException 400 / ConflictException 409 / ResourceNotFoundException 404 / InternalServerException 500. The model lists both BadRequestException and ConflictException; recorded behavior shows AWS reports THIS conflict as BadRequestException (400), not ConflictException — that is the requirement.
Emulator: the call SUCCEEDS while the run is still RUNNING and answers HTTP 204 with an empty body ({'ResponseMetadata': {'HTTPStatusCode': 204, ...}}), i.e. no refusal at all and the definition (and its run) are destroyed under the client.
Why: a client that relies on AWS's guard to protect a live experiment loses it. Code that catches BadRequestException and stops the run before retrying never sees the refusal, and a caller that deletes by mistake — or a cleanup path that runs while an experiment is live — silently tears down a RUNNING experiment and its run history instead of being told to stop the run first. The absence of the refusal also hides the state machine: nothing tells the client the definition was ACTIVE.
Fix: DeleteExperimentDefinition's handler must consult the definition's run state before deleting and, when any run of the definition is RUNNING (definition Status ACTIVE), emit BadRequestException 400 with AWS's template rendered from the request's definition id, leaving the definition and its runs untouched; the delete-and-cascade path must remain as it is for an IDLE definition.
Traces: appconfig.delete-experiment-definition-with-active-run-conflict-rejected.

## Evidence
Behavioral replay (simparity-behavioral-ga-prod-v1), check appconfig.delete-experiment-definition-with-active-run-conflict-rejected [mismatch]:

AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 204, 'HTTPHeaders': {'content-type': 'application/json', 'date': '...'}, 'RetryAttempts': 0}}

The steps leading up to it all behaved correctly: CreateExperimentDefinition returned Status=IDLE, StartExperimentRun returned Status=RUNNING, and GetExperimentDefinition then reported Status=ACTIVE. The delete under test was issued with DeleteType=DESTROY against that ACTIVE definition and was expected to answer BadRequestException with message 'Experiment definition <defId> cannot be deleted while an experiment is running.' (fixed text around the request's definition id; the run's concrete id value is an example, not part of the requirement).

## Steps to Reproduce
Point an appconfig client at the emulator's data listener:
1. create_application, create_environment, and an AWS.AppConfig.FeatureFlags configuration profile with a deployed flag.
2. create_experiment_definition(...) -> Status IDLE.
3. start_experiment_run(ApplicationIdentifier=<appId>, ExperimentDefinitionIdentifier=<defId>) -> Status RUNNING; get_experiment_definition -> Status ACTIVE.
4. delete_experiment_definition(ApplicationIdentifier=<appId>, ExperimentDefinitionIdentifier=<defId>, DeleteType='DESTROY').
   Expected: HTTP 400 BadRequestException, Message 'Experiment definition <defId> cannot be deleted while an experiment is running.', and get_experiment_definition still returns the definition with Status ACTIVE.
   Actual: HTTP 204, definition deleted.
5. Positive control that must keep working: stop_experiment_run, then the identical delete succeeds and both get_experiment_definition and get_experiment_run answer ResourceNotFoundException.
Re-replay appconfig.delete-experiment-definition-with-active-run-conflict-rejected to confirm.
