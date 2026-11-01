Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.exponential-growth-type-advances-in-powers-of-the-growth-factor
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.exponential-growth-type-advances-in-powers-of-the-growth-factor|_sample_sessions#31|Timeout|return out
Issue-Commit: f80d17848b0354e4d278a13ef40d60fb16242eb6

# AppConfig EXPONENTIAL rollout is never partial at the data plane: PercentageComplete is reported but never gates what a configuration session is served

> _Note: this issue was identified against the code base at commit `f80d17848b0354e4d278a13ef40d60fb16242eb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
One net-new root cause.

Cause 1 of 1: a deployment's rollout percentage is reported but never enforced, so an EXPONENTIAL (G*(2^N)) rollout is never partial in what consumers are actually served. Failing check: appconfig.exponential-growth-type-advances-in-powers-of-the-growth-factor.

AWS: Smithy: Deployment$GrowthType -> GrowthType, enum [LINEAR, EXPONENTIAL], "The algorithm used to define how percentage grew over time"; Deployment$GrowthFactor -> Percentage (range 1.0..100.0), "The percentage of targets to receive a deployed configuration during each interval"; Deployment$PercentageComplete -> Percentage, "The percentage of targets for which the deployment is available"; Deployment$State -> DeploymentState. AppConfig documents the EXPONENTIAL schedule as G*(2^N), so GrowthFactor=10 advances 10 -> 20 -> 40 -> 80 -> 100. That percentage is a SERVING decision, not a label: while a deployment is DEPLOYING at a percentage strictly between 0 and 100, independently opened AppConfigData configuration sessions for the same application/environment/profile are split — some are served the previous configuration version's exact bytes and some the new version's, bucketed per session and independently, no session ever served a blend — and the fleet converges on the new version only once the rollout reaches 100%.

Emulator: the serving decision ignores the rollout entirely. services/service-com-amazonaws-appconfig-amazonappconfig/src/state.rs decides which deployment is in force with Deployment::serves_the_configuration(), which is `self.is_in_flight() || self.state == DEPLOYMENT_COMPLETE` — true from the instant StartDeployment is answered (DEPLOYING) — and Environment::in_force_deployment() then picks the highest-numbered such deployment for the profile. Deployment::percentage_complete is advanced by the clock in Deployment::materialize()/rollout_schedule() for REPORTING only (and for the PERCENTAGE_UPDATED event text), and nothing consults it when answering a configuration poll; there is no per-session bucketing anywhere. So every session opened after StartDeployment — at 0%, at 20%, at 99% — is served the NEW version's bytes, and no session is ever served the previous version while the rollout is in flight. The check samples 4 fresh sessions on every tick where 0.0 < PercentageComplete < 100.0 and accumulates until BOTH versions have been observed; that condition can never become true on this emulator, so the check burns its declared _ROLLOUT_TIMEOUT_S (900 s) and is killed by the harness at 60.0 s with no assertion having fired. The exponential GRID itself is implemented (state.rs Deployment::rollout_schedule computes GrowthFactor*2^step for GROWTH_EXPONENTIAL, GrowthFactor*(step+1) for LINEAR, capped at 100), so only the partial-serving leg is attributed to this cause; the check's step-count, off-grid and convergence legs were never graded because it never leaves the rollout loop.

Why: a client relying on AWS's behavior believes a canary is a canary. GrowthType/GrowthFactor/DeploymentDurationInMinutes are the entire safety mechanism of an AppConfig deployment — an exponential rollout is supposed to expose 10% of the fleet, then 20%, then 40%, while the operator watches. Against this emulator every rollout is an instant 100% flip at StartDeployment while PercentageComplete cosmetically reports 10/20/40/80, so canary verification, blast-radius testing and any safe-deployment gating logic is validated against a target that cannot reproduce the failure mode it exists to catch. It is also the over-permissive direction: the new configuration reaches every consumer earlier than AWS would ever serve it.

Fix: the configuration-serving decision in the appconfig service's state — Deployment::serves_the_configuration / Environment::in_force_deployment must take the materialized PercentageComplete into account and bucket per configuration session (a session opened while the rollout is at P% is served the new version with probability P%, independently per session, and the previous in-force version otherwise), and the appconfigdata poll (and this service's own GetConfiguration) must answer from that decision. The rolling-back / rolled-back / reverted fallback to the preceding version is already correct; the gap is the partial, in-flight case. Confirming the fix also needs a wall-clock budget larger than the current 60 s cap, since the check's rollout is 4 minutes and the emulator paces a rollout in real minutes (Deployment::rollout_duration = 60 * DeploymentDurationInMinutes seconds).

Related open issue that does NOT carry this check's current replay key: 20261029T233120Z-appconfig-rollout-percentage-never-gates-what-the-data.md names this check id in its Issue-Trace-Ids but its recorded Issue-Key for it (`_sample_sessions#31|Timeout|...`) no longer matches where the replay now dies, so the replay did not report this check as OWNED; it is the same serving decision in state.rs and should be closed together. Also not covering it: 20261024T112615Z-appconfigdata-data-plane-deployed-configuration-never-served-sessions.md (session/serving defects, not percentage gating) and 20261025T020208Z-appconfig-cloudwatch-appconfig-deployments-with-cloudwatch-alarm-monitors-never-reach.md (monitored deployments never reaching a terminal state; this deployment carries no Monitors).

Traces: appconfig.exponential-growth-type-advances-in-powers-of-the-growth-factor.

## Evidence
Behavioral replay, suite simparity-behavioral-aws-v1. appconfig.exponential-growth-type-advances-in-powers-of-the-growth-factor [mismatch]: 'Error: Check timed out after 60.0s' — no assertion fired, because the condition the check polls for is unreachable on this emulator.

The check: CreateApplication / CreateEnvironment / CreateConfigurationProfile(LocationUri='hosted'); two hosted versions with distinguishable bytes (v1 b'{"tier":"one"}', v2 b'{"tier":"two"}'); an instant strategy (DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0) deploys v1 and 5/5 sessions are served v1 (correct today); then CreateDeploymentStrategy(DeploymentDurationInMinutes=4, GrowthType='EXPONENTIAL', GrowthFactor=10, FinalBakeTimeInMinutes=0) and StartDeployment of v2. On every tick where 0.0 < PercentageComplete < 100.0 it opens 4 fresh AppConfigData sessions and accumulates what each is served, until both versions have been seen or _ROLLOUT_TIMEOUT_S=900 elapses.

Expected: the accumulated mid-rollout payloads contain BOTH v1's and v2's exact bytes (assertion message: "the rollout was not partial — across N mid-rollout sessions only [...] was ever served, so the new version reached either none or all of the fleet instead of a percentage of it"), with no session served a blended document, and afterwards every session converges on v2.

Observed: every mid-rollout session is served v2 from the first tick, so the accumulation never short-circuits and the check is killed at the 60 s cap. The percentages/step count the check derives from GrowthFactor (10/20/40/80/100, five PERCENTAGE_UPDATED entries) were never reached, so they are not graded here.

Source of the cause (services/service-com-amazonaws-appconfig-amazonappconfig/src/state.rs): `pub fn serves_the_configuration(&self) -> bool { self.is_in_flight() || self.state == DEPLOYMENT_COMPLETE }`, consumed by `Environment::in_force_deployment` as the single place this service decides which deployment is in force for both GetConfiguration and the appconfigdata poll; `percentage_complete` is written only by `materialize()` (from `rollout_schedule()`) and read only by the Deployment serializer and the PERCENTAGE_UPDATED event text `format!("Configuration available to {reached}% of clients")`. Generated ids, names and the particular payload bytes in the run are examples; the requirement is that an in-flight rollout at P% serves the previous version to a share of independently opened sessions.

## Steps to Reproduce
1. CreateApplication, CreateEnvironment, CreateConfigurationProfile(LocationUri='hosted').
2. CreateHostedConfigurationVersion twice with distinguishable bytes (v1, v2).
3. CreateDeploymentStrategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo='NONE'); StartDeployment of v1; poll GetDeployment to State 'COMPLETE'.
4. appconfigdata StartConfigurationSession + GetLatestConfiguration from several independent sessions -> all return v1's bytes (correct today).
5. CreateDeploymentStrategy(DeploymentDurationInMinutes=4, GrowthType='EXPONENTIAL', GrowthFactor=10, FinalBakeTimeInMinutes=0, ReplicateTo='NONE'); StartDeployment of v2.
6. While GetDeployment reports 0.0 < PercentageComplete < 100.0, repeatedly open FRESH configuration sessions and record what each is served. Expected: a per-session mix of v1's and v2's exact bytes, no blended payload, the share on v2 tracking PercentageComplete. Observed: every session is served v2 from the first tick, already before PercentageComplete leaves 0.
7. After the deployment reaches COMPLETE, every session must be served v2's bytes (not graded today, the check never gets here).
8. Re-replay appconfig.exponential-growth-type-advances-in-powers-of-the-growth-factor with a wall-clock budget at least its declared _ROLLOUT_TIMEOUT_S (900 s) to confirm the fix.
