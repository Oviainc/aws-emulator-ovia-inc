Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.start-experiment-run-invalid-treatment-allocation-rejected
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.start-experiment-run-invalid-treatment-allocation-rejected|_poll_until#2|Timeout|time.sleep(interval_s)
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# appconfig: GetDeployment/GetEnvironment serve State as the model's SCREAMING_SNAKE enum member instead of AWS's PascalCase wire value, so a client's barrier on 'Complete'/'ReadyForDeployment' never releases and the StartExperimentRun treatment-override refusal is never reached

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeployment

## Description
One net-new root cause in the appconfig service. This check is NOT marked ALREADY FILED and NOT marked OWNED by any open issue in this replay (the open issue that lists the same check id, 20261107T182959Z-appconfig-2-net-new-causes-startexperimentrun-accepts-an.md, carries a different replay key: it was filed when the graded call was actually reached and answered 201; in THIS replay the check never reaches its graded call at all, and the barrier it dies in is a different defect).

=== Cause 1 of 1: Deployment$State and Environment$State are serialized as the Smithy enum MEMBER NAME (SCREAMING_SNAKE) rather than AWS's PascalCase wire value, so any client that waits for the deployment/environment to settle never observes a settled state and blocks forever ===
Failing check id: appconfig.start-experiment-run-invalid-treatment-allocation-rejected.

AWS: Smithy (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json) models Deployment$State -> DeploymentState, an enum whose members are [BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED], doc "The state of the deployment"; and Environment$State -> EnvironmentState, members [READY_FOR_DEPLOYMENT, DEPLOYING, ROLLING_BACK, ROLLED_BACK, REVERTED], doc "The state of the environment. An environment can be in one of the following states: READY_FOR_DEPLOYMENT, DEPLOYING, ROLLING_BACK, or ROLLED_BACK". The model therefore states the member NAMES only and carries no enumValue override for them. That the value actually put on the wire by real AWS is the PascalCase form of each member -- 'Complete', 'Baking', 'Validating', 'Deploying', 'RollingBack', 'RolledBack', 'Reverted' for a deployment and 'ReadyForDeployment', 'Deploying', 'RollingBack', 'RolledBack', 'Reverted' for an environment -- is recorded behavior only; the check records it explicitly ("despite the documented SCREAMING_SNAKE_CASE enums, the real wire values are PascalCase ('Complete', 'ReadyForDeployment') -- confirmed against real AWS") and builds its settle barriers on exactly those two values. The requirement is the mapping member -> PascalCase wire token, not any one run's state.

Emulator: the member name is emitted verbatim. GetDeployment answers State 'COMPLETE' and GetEnvironment answers the SCREAMING_SNAKE form, so the check's two setup barriers -- _poll_until(GetDeployment(...)['State'], == 'Complete', timeout_s=60) and _poll_until(GetEnvironment(...)['State'], == 'ReadyForDeployment', timeout_s=120) -- can never be satisfied by any value the emulator can produce. The check is consequently killed by the harness: "Error: Check timed out after 60.0s", with every assertion it exists for ungraded. The SCREAMING_SNAKE value is directly observable elsewhere in this same replay: three consecutive GetDeployment reads returned ['COMPLETE', 'COMPLETE', 'COMPLETE'], a StopDeployment refusal message read "... cannot be stopped, it has a status of COMPLETE.", and a rollout barrier reported "the rollout reached 'COMPLETE' at 100.0%" -- the emulator never emits the PascalCase token anywhere.

Why: State is the only completion signal AppConfig offers for a deployment or an environment. Every client that waits for a release to settle before doing the next thing -- a deploy script, a CI gate, a Step Functions poller, or in this case the StartExperimentRun precondition that an experiment's configuration profile already have a completed deployment -- compares State against the value real AWS sends. Against this emulator that comparison is false for every state, so the client hangs until its own timeout instead of proceeding, and any code path gated behind a settled deployment becomes unreachable. Here it makes the whole StartExperimentRun treatment-override negative test (BadRequestException 'Invalid treatment name: <key>. Valid treatment names are: [<control key>, <treatment keys>]', no run created, definition left IDLE -- the cause graded by open issue 20261107T182959Z) impossible to reach, so that issue's fix cannot be confirmed while this one stands.

Fix: the appconfig service's enum serialization for DeploymentState and EnvironmentState -- the response serializers for GetDeployment/StartDeployment/StopDeployment/ListDeployments and GetEnvironment/CreateEnvironment/UpdateEnvironment/ListEnvironments must map each enum member to its PascalCase wire token on output, and the inverse mapping must be applied wherever a State value is interpolated into a message (the StopDeployment terminal-state refusal above prints the raw member today). The state machine's internal representation can stay as-is; the defect is at the wire boundary, so the mapping belongs in one shared place rather than per handler. Prior work that does NOT carry this check's replay key: open issue 20261030T085534Z-appconfig-serves-deployment-state-as-the-model-s.md describes this same seam and owns appconfig.stop-experiment-run-double-stop-conflict in this replay, but it does not carry this check's key and does not cover Environment$State's 'ReadyForDeployment' barrier, which this check also blocks on.

Traces: appconfig.start-experiment-run-invalid-treatment-allocation-rejected. Corroborating observations from other checks in the same replay (each owned by a different open issue, listed here only as evidence that the raw member name is what goes on the wire): the ['COMPLETE','COMPLETE','COMPLETE'] reads, the "status of COMPLETE" refusal message, and "reached 'COMPLETE' at 100.0%". Generated application/environment/profile/strategy ids and names, deployment numbers and the particular timeout the harness killed the check at are per-run values; the requirement is the PascalCase wire token for every member of both enums.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the booted emulator: 211/246 passed (85.8%), 29 mismatched, 6 errored, 0 skipped, 0 unloadable; bootstrap_signal false (the emulator answered throughout, so this is not a bootstrap failure). Per-service appconfig 193/220 (88%), 27 failed; 26 of those 27 are marked OWNED by open issues and are excluded here.

[mismatch] appconfig.start-experiment-run-invalid-treatment-allocation-rejected
  Error: Check timed out after 60.0s
No assertion is reached, which is itself the finding: the check dies inside its setup barrier, before the graded StartExperimentRun is ever issued. From the check source in the replay report, the setup is: CreateApplication -> CreateEnvironment -> CreateConfigurationProfile(LocationUri='hosted', Type='AWS.AppConfig.FeatureFlags') -> CreateHostedConfigurationVersion -> CreateDeploymentStrategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo='NONE') -> StartDeployment, then
  _poll_until(lambda: appconfig.get_deployment(...)['State'], lambda state: state == 'Complete', timeout_s=60.0, interval_s=2.0)
  _poll_until(lambda: appconfig.get_environment(...)['State'], lambda state: state == 'ReadyForDeployment', timeout_s=120.0, interval_s=3.0)
with the in-source note "despite the documented SCREAMING_SNAKE_CASE enums, the real wire values are PascalCase ('Complete', 'ReadyForDeployment') -- confirmed against real AWS".

Expected (AWS): GetDeployment returns State 'Complete' once the zero-duration/zero-bake deployment settles, and GetEnvironment then returns State 'ReadyForDeployment'; both barriers release in seconds and the check proceeds to its graded call.
Actual (emulator): the enum MEMBER NAME is served instead. Corroborated inside this same replay by other appconfig checks reading the identical field:
  - "three consecutive GetDeployment reads during the bake must all report BAKING -- reading must not advance the deployment; got ['COMPLETE', 'COMPLETE', 'COMPLETE']"
  - botocore.errorfactory.BadRequestException ... StopDeployment ... "Deployment 2 for App ho65pb1 and Environment algcbw8 cannot be stopped, it has a status of COMPLETE." (the raw member leaks into the message text too)
  - "the rollout reached 'COMPLETE' at 100.0% before both versions were ever observed in service"
  - "a deployment whose environment monitor role AppConfig cannot assume must terminate ROLLED_BACK, got 'COMPLETE'"
Since 'Complete' is never produced, the 60 s barrier consumes the whole per-check budget and the harness kills the check at exactly 60.0s; every graded assertion (BadRequestException 'Invalid treatment name: nosuchtr. Valid treatment names are: [<control key>, <treatment key>]', ListExperimentRuns == [], GetExperimentDefinition Status 'IDLE', then Status 'RUNNING' for a valid key) is unreachable.

Smithy consulted (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json): DeploymentState enum values = [BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED]; EnvironmentState enum values = [READY_FOR_DEPLOYMENT, DEPLOYING, ROLLING_BACK, ROLLED_BACK, REVERTED]; Deployment$State -> DeploymentState ("The state of the deployment"); Environment$State -> EnvironmentState. No enumValue override is modelled, so the PascalCase wire form is recorded behavior only.

The appconfig service is stood up in this checkout (193 of its 220 checks pass), so this is a fidelity finding, not a bootstrap gap.

## Steps to Reproduce
Boot the emulator and point an appconfig client at its data listener (region/credentials irrelevant).
1. CreateApplication(Name=<new>); CreateEnvironment(ApplicationId=<app>, Name=<new>).
2. CreateConfigurationProfile(ApplicationId=<app>, Name=<new>, LocationUri='hosted', Type='AWS.AppConfig.FeatureFlags'); CreateHostedConfigurationVersion with a feature-flag document defining one flag.
3. CreateDeploymentStrategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo='NONE'); StartDeployment of that version.
4. THE GRADED READ: GetDeployment(ApplicationId=<app>, EnvironmentId=<env>, DeploymentNumber=<n>) and inspect ['State'].
   EXPECTED (AWS): the PascalCase wire token -- 'Complete' once settled (and 'Baking'/'Validating'/'Deploying' while in flight).
   OBSERVED: 'COMPLETE' (the Smithy enum member name), so a client comparing against 'Complete' never matches.
5. GetEnvironment(ApplicationId=<app>, EnvironmentId=<env>) and inspect ['State'].
   EXPECTED (AWS): 'ReadyForDeployment' (and 'Deploying'/'RollingBack'/'RolledBack'/'Reverted' for the other members).
   OBSERVED: the SCREAMING_SNAKE member name, so a client comparing against 'ReadyForDeployment' never matches.
6. Confirm the same mapping on every other State-bearing response: StartDeployment, StopDeployment, ListDeployments items, CreateEnvironment, UpdateEnvironment, ListEnvironments items.
7. Confirm the inverse mapping where a State is interpolated into a message: call StopDeployment twice on a settled deployment. EXPECTED message 'Deployment <n> for App <app-id> and Environment <env-id> cannot be stopped, it has a status of Complete.' with the PascalCase token; OBSERVED '... it has a status of COMPLETE.'
8. Re-replay appconfig.start-experiment-run-invalid-treatment-allocation-rejected: with the mapping fixed, both setup barriers release in seconds and the check proceeds to its graded StartExperimentRun call. Note that the graded refusal itself (BadRequestException 'Invalid treatment name: <key>. Valid treatment names are: [<control key>, <treatment keys>]', no run created, definition left IDLE) is the cause graded by open issue 20261107T182959Z-appconfig-2-net-new-causes-startexperimentrun-accepts-an.md, so this check will keep failing -- on that assertion rather than on a timeout -- until that issue is also closed.
