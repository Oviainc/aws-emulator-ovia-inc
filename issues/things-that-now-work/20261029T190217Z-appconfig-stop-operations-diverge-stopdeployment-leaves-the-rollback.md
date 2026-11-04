Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-deployment-stop-rollback
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-deployment-stop-rollback appconfig-experiment-run-conflict-errors
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|StopDeployment|2xx->2xx|$.EventLog[*].Description
Issue-Key: appconfig|StopDeployment|2xx->2xx|$.EventLog[*].EventType
Issue-Key: appconfig|StopDeployment|2xx->2xx|$.State
Issue-Key: appconfig|StopExperimentRun|4xx->4xx|$.Message
Issue-Key: appconfig|StopExperimentRun|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appconfig|StopExperimentRun|4xx->4xx|status
Issue-Commit: 473105e5105ba5cee28620541f40434a5fe4ef94

# AppConfig stop operations diverge: StopDeployment leaves the rollback unfinished and StopExperimentRun raises the wrong error class

> _Note: this issue was identified against the code base at commit `473105e5105ba5cee28620541f40434a5fe4ef94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeployment

## Description
Both AppConfig "stop" operations report the wrong terminal outcome: StopDeployment returns an in-progress rollback where AWS returns a finished one, and StopExperimentRun refuses a stop of an already-finished run with the wrong HTTP status, error code and message. 2 causes are bundled.

Cause 1 of 2: StopDeployment returns ROLLING_BACK instead of ROLLED_BACK - fixture appconfig-deployment-stop-rollback, step task#8.
AWS: Smithy StopDeployment (DELETE /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, success code 202, output shape Deployment, errors BadRequestException 400 / InternalServerException 500 / ResourceNotFoundException 404). Deployment.State targets the DeploymentState enum, whose values are BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED. Recorded behavior: the 202 response to StopDeployment already reports the rollback as COMPLETE - State == "ROLLED_BACK", and the newest EventLog entry has EventType "ROLLBACK_COMPLETED" with Description "Deployment rolled back by user request". AWS settles the deployment record synchronously in the stop response rather than handing back a transitional state. (The open behavioral issue "AppConfig DeleteEnvironment cascade-deletes in-flight deployment records, so StopDeployment on the orphaned deployment 404s instead of returning it ROLLED_BACK" independently records the same AWS contract - 202 with State ROLLED_BACK and ROLLBACK_COMPLETED in the EventLog - but it is about the orphaned-deployment lookup, not about this normal-path state, so it does not cover this step.)
Emulator: answers 202 but with State "ROLLING_BACK" and EventLog[0] = EventType "ROLLBACK_STARTED", Description "Deployment rollback started" - the transitional state, never advanced.
Why: a client that stops a deployment and reads the response to confirm the rollback landed sees it still in flight. Code that waits for a terminal state from the stop response hangs or polls forever, and code that branches on ROLLED_BACK/ROLLBACK_COMPLETED never takes that branch. The enum offers both values, so the emulator is choosing the wrong one, not approximating a missing one.
Fix: the StopDeployment handler and the deployment state machine in the appconfig service - a stop must transition the deployment to ROLLED_BACK and append a ROLLBACK_COMPLETED event with the "Deployment rolled back by user request" description before serializing the 202, so Get/List of that deployment report the same terminal state.

Cause 2 of 2: StopExperimentRun on a finished run answers 409 ConflictException instead of 400 BadRequestException - fixture appconfig-experiment-run-conflict-errors, step task#13.
AWS: Smithy StopExperimentRun (PATCH /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}/experimentruns/{Run}/stop, success 200, errors BadRequestException 400 / ConflictException 409 / InternalServerException 500 / ResourceNotFoundException 404) - both error classes are modeled, so which one applies is a contract, not a choice. Recorded behavior: stopping a run that is already DONE is a BAD REQUEST, not a conflict: HTTP 400, response header x-amzn-errortype "BadRequestException:" (as recorded, with the trailing colon AppConfig emits), body Message "Experiment run <run> for experiment definition <experimentDefinitionId> is not running.". The message's FIXED text is "Experiment run " / " for experiment definition " / " is not running."; the VARIABLE parts are the run number from the request path and the experiment definition id from the request path (the recorded run 1 and definition id are per-run values, not requirements).
Emulator: answers HTTP 409 with x-amzn-errortype "ConflictException" and Message "Run <run> of experiment definition <id> is DONE, so it cannot be stopped: only a running experiment can be stopped." - wrong status, wrong error code and a wholly different message wording.
Why: SDK clients dispatch on the modeled exception type, so a caller catching BadRequestException for this case does not catch it at all and the error escapes as an unexpected ConflictException; retry layers also treat 409 and 400 differently, and a 409 invites a retry that can never succeed. The message text is part of the contract too: anything parsing or logging it sees unrecognised wording.
Fix: the StopExperimentRun handler's state check in the appconfig service - when the addressed run is not RUNNING, raise BadRequestException (HTTP 400) with the "Experiment run <run> for experiment definition <id> is not running." message template and the corresponding x-amzn-errortype header, reserving ConflictException for the cases AWS actually uses it for.

Related open issue (does not cover this step): "Implement appconfig StopExperimentRun: unimplemented scaffold stub" - the operation now answers (200 on the happy path, 409 here), so this error-class divergence is net-new.

## Evidence
Cause 1 (appconfig-deployment-stop-rollback#task#8, StopDeployment, expected 202 / actual 202):
  $.State: expected 'ROLLED_BACK', got 'ROLLING_BACK'
  $.EventLog[0].EventType: expected 'ROLLBACK_COMPLETED', got 'ROLLBACK_STARTED'
  $.EventLog[0].Description: expected 'Deployment rolled back by user request', got 'Deployment rollback started'
  Both expected values are members of the modeled DeploymentState / deployment-event vocabulary, so this is the wrong value, not an unmodeled one.

Cause 2 (appconfig-experiment-run-conflict-errors#task#13, StopExperimentRun):
  status: expected 400, got 409
  $headers.x-amzn-errortype: expected 'BadRequestException:' (as recorded), got 'ConflictException'
  $.Message: expected 'Experiment run <run> for experiment definition <experimentDefinitionId> is not running.', got 'Run <run> of experiment definition <id> is DONE, so it cannot be stopped: only a running experiment can be stopped.'
  Message decomposition - fixed text: "Experiment run " / " for experiment definition " / " is not running."; variable: the run number and the experiment definition id, both taken from the request path (recorded run 1 and definition id 'q92lkf0' are per-run values).

Smithy support: StopDeployment DELETE .../deployments/{DeploymentNumber}, success 202, output Deployment, Deployment.State -> DeploymentState enum [BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED]; StopExperimentRun PATCH .../experimentruns/{Run}/stop, success 200, errors BadRequestException (client, 400) / ConflictException (client, 409) / InternalServerException 500 / ResourceNotFoundException 404.

## Steps to Reproduce
Cause 1:
1. Replay fixture appconfig-deployment-stop-rollback and inspect step task#8.
2. Create an application, environment, configuration profile, hosted configuration version and a deployment strategy long enough that the deployment stays in flight, then StartDeployment.
3. Call StopDeployment(ApplicationId, EnvironmentId, DeploymentNumber).
   Expected: HTTP 202 with State == 'ROLLED_BACK' and EventLog[0] == {EventType 'ROLLBACK_COMPLETED', Description 'Deployment rolled back by user request'}.
   Observed: HTTP 202 with State 'ROLLING_BACK' and EventLog[0] EventType 'ROLLBACK_STARTED', Description 'Deployment rollback started'.

Cause 2:
4. Replay fixture appconfig-experiment-run-conflict-errors and inspect step task#13.
5. CreateExperimentDefinition, StartExperimentRun, then StopExperimentRun once (the run becomes DONE).
6. Call StopExperimentRun on that same run again.
   Expected: HTTP 400, x-amzn-errortype BadRequestException, Message 'Experiment run <run> for experiment definition <id> is not running.'
   Observed: HTTP 409, x-amzn-errortype ConflictException, Message 'Run <run> of experiment definition <id> is DONE, so it cannot be stopped: only a running experiment can be stopped.'
7. Re-replay both bundled fixture#step bindings listed under Traces to confirm the fix.
