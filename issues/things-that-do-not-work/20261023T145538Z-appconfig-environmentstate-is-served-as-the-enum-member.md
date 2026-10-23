Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-app-environment-configuration-profile-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-app-environment-configuration-profile-lifecycle appconfig-deployment-stop-rollback appconfig-end-to-end-deployment-workflow appconfig-environment-lifecycle-and-monitors appconfig-error-paths-not-found-and-invalid appconfig-experiment-list-operations appconfig-experiment-run-full-lifecycle appconfig-experiment-run-lifecycle appconfig-experiment-run-treatment-overrides appconfig-multiple-experiment-definitions-isolation
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|CreateEnvironment|2xx->2xx|$.State
Issue-Key: appconfig|GetEnvironment|2xx->2xx|$.State
Issue-Key: appconfig|ListEnvironments|2xx->2xx|$.Items[Name="sim2real-probe-appconfig-env-9c3ca1cd"].State
Issue-Key: appconfig|UpdateEnvironment|2xx->2xx|$.State
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# AppConfig EnvironmentState is served as the enum member name instead of the CamelCase wire value

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEnvironment

## Description
Every operation that serves an AppConfig Environment reports State in the SCREAMING_SNAKE enum member spelling rather than the CamelCase form AWS puts on the wire. Four replayed clusters, one per operation that returns an Environment. Numbered; close ALL FOUR before submitting.

=== Cause 1 of 4: CreateEnvironment ===
AWS: recorded behavior only — the 201 Environment document carries State='ReadyForDeployment'. Smithy: CreateEnvironment http {method:POST, uri:/applications/{ApplicationId}/environments, code:201}, output Environment.State target EnvironmentState (enum), whose MEMBER names are READY_FOR_DEPLOYMENT/DEPLOYING/ROLLING_BACK/ROLLED_BACK/REVERTED — i.e. the member name is not the recorded wire value.
Emulator: returns 201 with State='READY_FOR_DEPLOYMENT'. Status matches; only the enum rendering diverges.
Why: a client that compares State to AWS's value never matches, so a wait-for-ready loop after CreateEnvironment never terminates and a state machine keyed on 'ReadyForDeployment' falls through to its unknown-state branch.
Fix: the appconfig crate's environment-state constants/serializer must carry the recorded wire value for every EnvironmentState member, not the member name (state module plus the Environment serializer shared by create/get/update/list).
Traces: appconfig-app-environment-configuration-profile-lifecycle#task#2 (cluster of 10 fixtures).

=== Cause 2 of 4: GetEnvironment ===
AWS: recorded behavior only — GET /applications/{ApplicationId}/environments/{EnvironmentId} answers 200 with State='ReadyForDeployment'.
Emulator: 200 with State='READY_FOR_DEPLOYMENT'.
Why: same as cause 1 on the read path, which is where polling clients look.
Fix: as cause 1 (shared serializer).
Traces: appconfig-end-to-end-deployment-workflow#task#7 (cluster of 3).

=== Cause 3 of 4: ListEnvironments ===
AWS: recorded behavior only — Items[].State is 'ReadyForDeployment'.
Emulator: Items[].State='READY_FOR_DEPLOYMENT'.
Why: a lister that filters environments by state sees no matches.
Fix: as cause 1 — the list projection must use the same corrected serializer.
Traces: appconfig-environment-lifecycle-and-monitors#task#4.

=== Cause 4 of 4: UpdateEnvironment ===
AWS: recorded behavior only — the 200 response echoes State='ReadyForDeployment'.
Emulator: 200 with State='READY_FOR_DEPLOYMENT'.
Why: a client that reads the state back from the update response mismatches.
Fix: as cause 1.
Traces: appconfig-environment-lifecycle-and-monitors#task#5.

Related: the open behavioral issue 20261023T032202Z-appconfig-control-plane-fidelity-settings-route-missing-environmentstate.md describes the same rendering fault from the behavioral corpus (its cause 2); it carries no wire replay key, so this issue binds the fixture steps above.

## Evidence
Wire replay (simparity-wire-aws-v1):

Cause 1 — appconfig-app-environment-configuration-profile-lifecycle#task#2 CreateEnvironment, expected 201 / actual 201: $.State value_mismatch (expected 'ReadyForDeployment', got 'READY_FOR_DEPLOYMENT'); same cluster also hit by appconfig-deployment-stop-rollback#setup#2, appconfig-end-to-end-deployment-workflow#setup#2, appconfig-environment-lifecycle-and-monitors#task#2, appconfig-error-paths-not-found-and-invalid#task#2, appconfig-experiment-list-operations#setup#2, appconfig-experiment-run-full-lifecycle#setup#2, appconfig-experiment-run-lifecycle#task#2, appconfig-experiment-run-treatment-overrides#setup#2, appconfig-multiple-experiment-definitions-isolation#setup#2.

Cause 2 — appconfig-end-to-end-deployment-workflow#task#7 GetEnvironment, 200/200: $.State expected 'ReadyForDeployment', got 'READY_FOR_DEPLOYMENT' (also appconfig-environment-lifecycle-and-monitors#task#3 and #task#6).

Cause 3 — appconfig-environment-lifecycle-and-monitors#task#4 ListEnvironments, 200/200: $.Items[Name="sim2real-probe-appconfig-env-9c3ca1cd"].State expected 'ReadyForDeployment', got 'READY_FOR_DEPLOYMENT'.

Cause 4 — appconfig-environment-lifecycle-and-monitors#task#5 UpdateEnvironment, 200/200: $.State expected 'ReadyForDeployment', got 'READY_FOR_DEPLOYMENT'.

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.
1. create_application(Name=...) -> note Id.
2. create_environment(ApplicationId=<id>, Name=...) and read State from the 201 response (cause 1).
3. get_environment(ApplicationId=<id>, EnvironmentId=<env>) (cause 2).
4. list_environments(ApplicationId=<id>) and read Items[0].State (cause 3).
5. update_environment(ApplicationId=<id>, EnvironmentId=<env>, Description='x') and read State (cause 4).
Each must report 'ReadyForDeployment'; the emulator reports 'READY_FOR_DEPLOYMENT'.
Re-replay appconfig-app-environment-configuration-profile-lifecycle#task#2 plus the steps in trace_ids.
