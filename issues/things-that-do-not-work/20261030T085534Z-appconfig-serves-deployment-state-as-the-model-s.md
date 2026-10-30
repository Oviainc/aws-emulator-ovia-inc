Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.stop-experiment-run-double-stop-conflict
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.stop-experiment-run-double-stop-conflict|_poll_until#2|Timeout|time.sleep(interval_s)
Issue-Commit: 5421661bb9d17acf3f504f47184fa9f2fc2d7801

# AppConfig serves Deployment.State as the model's SCREAMING_SNAKE member name instead of AWS's CamelCase wire value, so a client's wait for 'Complete' never terminates

> _Note: this issue was identified against the code base at commit `5421661bb9d17acf3f504f47184fa9f2fc2d7801`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeployment

## Description
One net-new root cause. Cause 1 of 1: the Deployment document's State member is serialized in the model's member-name form (DEPLOYING / BAKING / COMPLETE / ROLLING_BACK / ROLLED_BACK / REVERTED) rather than in the CamelCase form AppConfig actually puts on the wire (Deploying / Baking / Complete / RollingBack / RolledBack / Reverted), so a caller that polls GetDeployment until the deployment is complete never sees a terminal value and loops forever.

AWS: Smithy GetDeployment (GET /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, success 200) returns the Deployment shape whose State member targets the DeploymentState enum (members BAKING, VALIDATING, DEPLOYING, COMPLETE, ROLLING_BACK, ROLLED_BACK, REVERTED). The shipped model JSON carries NO @enumValue trait on those members, so the member names are NOT the authoritative wire form — exactly the situation already settled for the sibling enum EnvironmentState, where this repo accepted (issue 20261023T145538Z-appconfig-environmentstate-is-served-as-the-enum-member.md, fixed) that AppConfig serves Environment$State as 'ReadyForDeployment | Deploying | RollingBack | RolledBack | Reverted'. Recorded behavior only for the deployment enum: the failing check carries an explicit MEASURED note — "despite the API documentation showing these enums in SCREAMING_SNAKE_CASE (e.g. COMPLETE, READY_FOR_DEPLOYMENT), the actual wire values returned by the service are PascalCase (\"Complete\", \"ReadyForDeployment\") -- confirmed against real AWS" — i.e. a deployment whose strategy carries DeploymentDurationInMinutes=0 and FinalBakeTimeInMinutes=0 reaches State 'Complete' within seconds, and that is the value a completion wait matches on. The requirement is the CASING of the enum member the response carries; which member is reported (the deployment reaching completion after duration+bake) is already correct here, and the per-run deployment number, ids and timestamps are not part of it.

Emulator: the DeploymentState values are hard-coded as the member names and stored/served verbatim — services/service-com-amazonaws-appconfig-amazonappconfig/src/state.rs declares `pub const DEPLOYMENT_COMPLETE: &str = "COMPLETE"` (with DEPLOYMENT_DEPLOYING="DEPLOYING", DEPLOYMENT_BAKING="BAKING", DEPLOYMENT_ROLLING_BACK="ROLLING_BACK", DEPLOYMENT_ROLLED_BACK="ROLLED_BACK", DEPLOYMENT_REVERTED="REVERTED") and a doc comment that deliberately exempts DeploymentState from the CamelCase treatment it gives EnvironmentState ("Every other enum this module names a value of (DeploymentState, ...) is served as its member name"); Deployment::materialize() writes `self.state = DEPLOYMENT_COMPLETE.to_string()` and the Deployment serializer reports that string. So GetDeployment answers State 'COMPLETE' where AWS answers 'Complete'.

Why: this is not a cosmetic string. A client that waits for a deployment to finish compares State to AWS's value; against this emulator the comparison is never true, so the wait never completes — the configuration deploy step of any client, CDK/CLI waiter, test harness or orchestration script hangs until its own timeout and then reports a deployment failure for a deployment that in fact succeeded. The failing check is killed exactly this way: its bounded wait for State == 'Complete' burns its whole budget and the harness kills the check at 60.0s before any of its own assertions is graded, so the entire StopExperimentRun double-stop behavior it exists to check goes unexercised. The same mis-cased value also reaches every other consumer of the member (ListDeployments item State, and the Deployment document returned by StartDeployment/StopDeployment).

Note for the fixer (an internal inconsistency to reconcile, not a second cause): a sibling check, appconfig.stop-already-stopped-experiment-run-conflict, polls its setup deployment for the SCREAMING_SNAKE 'COMPLETE' and matches today, which is how it got past setup to its own (separately owned) assertion. One casing is right for both; the only recorded, live-confirmed statement available here is the MEASURED note quoted above, so the CamelCase form is what this issue requires — re-confirm against a recording before changing the constant, and expect that sibling's setup wait to need the same value.

Also note: once this cause is fixed, the failing check will proceed to its real assertion, StopExperimentRun on an already-DONE run, which diverges for a DIFFERENT and already-filed reason — 20261029T195212Z-appconfig-control-plane-refusals-wrong-error-classes-and.md, Cause 2 (409 ConflictException instead of 400 BadRequestException with 'Experiment run <run> for experiment definition <definitionId> is not running.'). That issue does not cover this check (its replay set does not carry this check id) and this issue does not cover that cause; both must be closed for this check to pass.

Fix: the DeploymentState value constants in the appconfig service's state (services/service-com-amazonaws-appconfig-amazonappconfig/src/state.rs, the DEPLOYMENT_* constants and the doc comment that exempts them), so every path that writes or compares a deployment state — Deployment::materialize, Deployment::serves_the_configuration, the stop/revert guards — and the Deployment/DeploymentSummary serializers carry the CamelCase wire form, the same way ENVIRONMENT_* already does.

Traces: appconfig.stop-experiment-run-double-stop-conflict.

## Evidence
Failing check: appconfig.stop-experiment-run-double-stop-conflict — "Error: Check timed out after 60.0s", with no assertion of the check's own graded.

Where it hangs: the check's setup (_create_experiment_ready_env) creates an application, environment, AWS.AppConfig.FeatureFlags profile and hosted version, creates a strategy with DeploymentDurationInMinutes=0 / GrowthFactor=100 / FinalBakeTimeInMinutes=0 / ReplicateTo='NONE', calls StartDeployment, and then polls
    _poll_until(lambda: appconfig.get_deployment(...)["State"], lambda state: state == "Complete", timeout_s=60.0, interval_s=2.0)
Expected: GetDeployment reports State 'Complete' (CamelCase) within seconds for a 0-minute/0-bake strategy. Observed: the emulator reports State 'COMPLETE', so the predicate is never satisfied, the 60s poll runs out, and the harness's 60.0s cap kills the check before StartExperimentRun / StopExperimentRun are reached. (The check's second wait, for Environment State == 'ReadyForDeployment', matches today — EnvironmentState is already served CamelCase — which isolates the defect to the deployment enum.)

Emulator source (services/service-com-amazonaws-appconfig-amazonappconfig/src/state.rs):
  line 42-54: `pub const DEPLOYMENT_DEPLOYING: &str = "DEPLOYING";` … `pub const DEPLOYMENT_COMPLETE: &str = "COMPLETE";` … `DEPLOYMENT_ROLLING_BACK = "ROLLING_BACK"`, `DEPLOYMENT_ROLLED_BACK = "ROLLED_BACK"`, `DEPLOYMENT_REVERTED = "REVERTED"`.
  line 56-71 (the EnvironmentState block): "AppConfig serves Environment$State in the CamelCase form … Every other enum this module names a value of (DeploymentState, GrowthType, DeploymentEventType, TriggeredBy) is served as its member name" — the explicit decision this cause disputes for DeploymentState.
  line 2524: `self.state = DEPLOYMENT_COMPLETE.to_string();` in Deployment::materialize(), the single place the completion state is written.

Recorded/measured support (the check's own note, verbatim): "despite the API documentation showing these enums in SCREAMING_SNAKE_CASE (e.g. COMPLETE, READY_FOR_DEPLOYMENT), the actual wire values returned by the service are PascalCase (\"Complete\", \"ReadyForDeployment\") -- confirmed against real AWS. Polling for the documented-but-wrong casing never matches".

Smithy support: GetDeployment -> output shape Deployment, member State -> com.amazonaws.appconfig#DeploymentState (enum, members BAKING | VALIDATING | DEPLOYING | COMPLETE | ROLLING_BACK | ROLLED_BACK | REVERTED, NO @enumValue traits in the shipped model); EnvironmentState is modeled the same way (members READY_FOR_DEPLOYMENT | DEPLOYING | ROLLING_BACK | ROLLED_BACK | REVERTED) yet is served CamelCase after the already-fixed issue 20261023T145538Z-appconfig-environmentstate-is-served-as-the-enum-member.md.

## Steps to Reproduce
1. CreateApplication, CreateEnvironment, CreateConfigurationProfile(LocationUri='hosted', Type='AWS.AppConfig.FeatureFlags'), CreateHostedConfigurationVersion with a small feature-flag document.
2. CreateDeploymentStrategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo='NONE').
3. StartDeployment with that strategy, profile and version.
4. Call GetDeployment(ApplicationId, EnvironmentId, DeploymentNumber) in a loop. Expected: State transitions to the CamelCase terminal value 'Complete' within seconds (and the in-flight value reads 'Deploying'/'Baking'). Observed: State is 'COMPLETE' (and 'DEPLOYING'/'BAKING'), so any wait written against AWS's wire value never terminates.
5. Same check for the other members: StopDeployment of an in-flight deployment must report 'RollingBack' then 'RolledBack'; StopDeployment(AllowRevert=true) of a completed one must report 'Reverted'; the State in ListDeployments items and in the StartDeployment/StopDeployment response documents must use the same CamelCase form.
6. Re-replay appconfig.stop-experiment-run-double-stop-conflict to confirm the fix: it must get past its setup wait (needs a wall-clock budget larger than the current 60s cap — its own declared budgets are 60s/120s/300s) and reach its StopExperimentRun assertions, which additionally require Cause 2 of 20261029T195212Z-appconfig-control-plane-refusals-wrong-error-classes-and.md (400 BadRequestException with 'Experiment run <run> for experiment definition <definitionId> is not running.').
