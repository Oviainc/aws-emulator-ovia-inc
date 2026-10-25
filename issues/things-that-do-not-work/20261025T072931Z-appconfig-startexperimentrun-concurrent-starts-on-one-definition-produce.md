Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.concurrent-start-experiment-run-only-one-succeeds
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.concurrent-start-experiment-run-only-one-succeeds|run#15|AssertionError|assert len(results) == 1, f"expected exactly one StartExperimentRun to succeed, got {len(results)}"
Issue-Commit: b2b6f2b35a27af8dd637d5ecea8580938de38787

# AppConfig StartExperimentRun: concurrent starts on one definition produce no RUNNING run and no ConflictException loser

> _Note: this issue was identified against the code base at commit `b2b6f2b35a27af8dd637d5ecea8580938de38787`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExperimentRun

## Description
Cause 1 of 1 — the single-active-run guard on StartExperimentRun does not exist, because the operation answers no call.
AWS: Smithy: `StartExperimentRun` is POST /applications/{ApplicationIdentifier}/experimentdefinitions/{ExperimentDefinitionIdentifier}/experimentruns with success code 201 and output shape `ExperimentRun` (members include `Run` (Integer) and `Status` (`ExperimentRunStatus` enum)); its modeled client errors are `BadRequestException` (httpError 400), `ConflictException` (httpError 409) and `ResourceNotFoundException` (404). Recorded behavior: two concurrent `StartExperimentRun` calls against the SAME experiment definition race under optimistic locking and exactly one wins — it returns 201 with a `Run` number and `Status` == "RUNNING" — while the other is refused with HTTP 409, Error.Code `ConflictException`. An independent `GetExperimentRun(ApplicationIdentifier, ExperimentDefinitionIdentifier, Run=<winner's run>)` then reports the same `Run` with `Status` == "RUNNING". Only one run may be active per definition.
Emulator: BOTH concurrent calls were refused with error code `NotImplemented` (HTTP 501, "operation StartExperimentRun is scaffolded but not implemented in this emulator"), so 0 runs were created: there is no winner carrying Status RUNNING, no 409 `ConflictException` loser, and nothing for `GetExperimentRun` to read. This is the net-new requirement: the existing open stub issue `20261024T131534Z-appconfig-startexperimentrun-stub.md` carries 13 replay keys and this concurrency check is NOT among them, so implementing the operation without a single-active-run guard would close that issue while leaving the race wrong and unverified.
Why: a client that starts an experiment run from more than one worker relies on the server to serialize the start — exactly one run becomes active and the duplicate is told `ConflictException` so it can attach to the existing run. Against the emulator every starter instead receives an unmodeled 501 that cannot be matched as `ConflictException`, so neither the winner's run number nor the duplicate-suppression contract is observable, and a handler implemented without the guard would let two RUNNING runs exist for one definition.
Fix: `StartExperimentRun`'s handler and the experiment-run state it writes (services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/start_experiment_run.rs): implement the handler and make the create of a run conditional under the state lock on no run being active for that definition, returning 201 `ExperimentRun` with `Status` RUNNING to the winner and `ConflictException` (409) to the loser, with the new run readable via `GetExperimentRun`.
Traces: appconfig.concurrent-start-experiment-run-only-one-succeeds

## Evidence
Check appconfig.concurrent-start-experiment-run-only-one-succeeds (mismatch):
  Expected: of two concurrent StartExperimentRun(ApplicationIdentifier=<app>, ExperimentDefinitionIdentifier=<exp>) calls, exactly 1 succeeds (HTTP 201, Status == "RUNNING", a Run number) and exactly 1 fails with Error.Code == "ConflictException" (HTTP 409); GetExperimentRun on the winner's Run then reports Status "RUNNING" and the same Run.
  Actual: AssertionError: expected exactly one StartExperimentRun to succeed, got 0 — both calls raised botocore ClientError with Error.Code "NotImplemented" (HTTP 501, "operation StartExperimentRun is scaffolded but not implemented in this emulator"), so no run exists and no ConflictException is produced.

## Steps to Reproduce
1. Boot the appconfig emulator and replay appconfig.concurrent-start-experiment-run-only-one-succeeds.
2. CreateApplication, CreateEnvironment, and a AWS.AppConfig.FeatureFlags configuration profile with a hosted version containing one flag; CreateDeploymentStrategy(DeploymentDurationInMinutes=0, GrowthFactor=100, GrowthType=LINEAR, FinalBakeTimeInMinutes=0, ReplicateTo=NONE) and StartDeployment of that version, waiting for GetDeployment State == COMPLETE.
3. CreateExperimentDefinition against that application/environment/profile and flag key.
4. From two threads, call StartExperimentRun(ApplicationIdentifier=<app>, ExperimentDefinitionIdentifier=<exp>) simultaneously.
5. Observe both calls refused with error code NotImplemented (HTTP 501) instead of one HTTP 201 with Status RUNNING and one HTTP 409 ConflictException; GetExperimentRun has no run to return.
