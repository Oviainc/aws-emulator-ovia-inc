Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-deployment-strategy-crud
Issue-Step: teardown#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-deployment-strategy-crud appconfig-deployment-strategy-update-and-list-lifecycle appconfig-end-to-end-deployment-workflow appconfig-global-listings appconfig-environment-lifecycle-and-monitors appconfig-extension-and-association-lifecycle
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|GetDeploymentStrategy|4xx->2xx|$
Issue-Key: appconfig|GetDeploymentStrategy|4xx->2xx|$.Message
Issue-Key: appconfig|GetDeploymentStrategy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appconfig|GetDeploymentStrategy|4xx->2xx|status
Issue-Key: appconfig|GetEnvironment|4xx->2xx|$
Issue-Key: appconfig|GetEnvironment|4xx->2xx|status
Issue-Key: appconfig|GetEnvironment|4xx->2xx|$.Message
Issue-Key: appconfig|GetEnvironment|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appconfig|UpdateEnvironment|4xx->2xx|$.Message
Issue-Key: appconfig|UpdateEnvironment|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appconfig|UpdateEnvironment|4xx->2xx|status
Issue-Key: appconfig|GetExtension|4xx->2xx|$.Message
Issue-Key: appconfig|GetExtension|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appconfig|GetExtension|4xx->2xx|status
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# AppConfig deletes of deployment strategy, environment and extension leave the resource readable

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDeploymentStrategy

## Description
Same class of fault as the application/profile deletes, on three further resource kinds: after a successful Delete* the emulator still serves the resource with HTTP 200 instead of refusing with ResourceNotFoundException/404. Eight replayed clusters. Numbered; close ALL EIGHT before submitting.

=== Cause 1 of 8: GetDeploymentStrategy 200 after DeleteDeploymentStrategy (polled, full body) ===
AWS: GetDeploymentStrategy on a deleted strategy answers ResourceNotFoundException, HTTP 404, with x-amzn-errortype set (Smithy: GetDeploymentStrategy errors include ResourceNotFoundException httpError 404; recorded message example 'DeploymentStrategy with Id <id> could not be found.' — status and code are the requirement, the sentence is an example).
Emulator: 200 with the whole strategy document for 15 poll attempts, no Message, no errortype header.
Why: a delete-then-confirm client never converges and the strategy stays usable in StartDeployment.
Fix: DeleteDeploymentStrategy's handler/state must remove the record; GetDeploymentStrategy must miss on it.
Traces: appconfig-deployment-strategy-crud#teardown#6.

=== Cause 2 of 8: GetDeploymentStrategy 200 after delete (single shot) ===
AWS: as cause 1.
Emulator: 200, with $.Message and $headers.x-amzn-errortype absent entirely.
Why: the gap is visible without polling, in the middle of a strategy update/list lifecycle.
Fix: as cause 1.
Traces: appconfig-deployment-strategy-update-and-list-lifecycle#task#8.

=== Cause 3 of 8: GetDeploymentStrategy 200 after the deployment workflow's teardown delete ===
AWS: as cause 1.
Emulator: 200 with the instant-deploy strategy still present (DeploymentDurationInMinutes 0, GrowthFactor 100).
Why: as cause 1; a repeated workflow accumulates strategies AWS would have removed.
Fix: as cause 1.
Traces: appconfig-end-to-end-deployment-workflow#teardown#11.

=== Cause 4 of 8: GetDeploymentStrategy 200 after delete in the global-listings fixture ===
AWS: as cause 1 — status 404.
Emulator: 200 (status-only divergence in this cluster).
Why: as cause 1, and the strategy keeps showing up in account-wide listings.
Fix: as cause 1.
Traces: appconfig-global-listings#teardown#5.

=== Cause 5 of 8: GetEnvironment 200 after DeleteEnvironment (polled) ===
AWS: a deleted environment is gone — GetEnvironment answers ResourceNotFoundException/404 (Smithy: ResourceNotFoundException httpError 404).
Emulator: 200 with the environment document for 15 poll attempts.
Why: the environment stays deployable and keeps its deployment history, so teardown and re-create flows diverge from production.
Fix: DeleteEnvironment must remove the environment from state (with its deployments); the Get path must miss on it.
Traces: appconfig-end-to-end-deployment-workflow#teardown#17.

=== Cause 6 of 8: GetEnvironment 200 after DeleteEnvironment (single shot, errortype asserted) ===
AWS: 404 with x-amzn-errortype 'ResourceNotFoundException:' (recorded); message example 'Environment with Application Id <appId> and Environment Id <envId> could not be found for account <account>.'
Emulator: 200, no error body, no errortype header.
Why: an SDK cannot classify the outcome at all — it sees success where AWS raises a modeled, catchable error.
Fix: as cause 5.
Traces: appconfig-environment-lifecycle-and-monitors#teardown#8.

=== Cause 7 of 8: UpdateEnvironment succeeds on a deleted environment ===
AWS: a write against a deleted environment is refused with ResourceNotFoundException/404 and x-amzn-errortype 'ResourceNotFoundException:' (recorded).
Emulator: 200 — the update is accepted and applied to a resource that should not exist.
Why: worse than a stale read: the emulator resurrects/mutates deleted state, so a client's error-handling path for deleted environments is never exercised and the emulator's state drifts from AWS.
Fix: as cause 5, plus an existence check in UpdateEnvironment's handler before mutating state.
Traces: appconfig-environment-lifecycle-and-monitors#teardown#9.

=== Cause 8 of 8: GetExtension 200 after DeleteExtension ===
AWS: 404 ResourceNotFoundException for a deleted extension (recorded message example 'Extension could not be found for account <account>.').
Emulator: 200 with the extension document, no Message, no errortype header.
Why: extensions and their associations survive deletion, so association lifecycles cannot be tested and stale extensions keep being applied to deployments.
Fix: DeleteExtension must remove the extension (and its associations) from state; GetExtension must miss on it.
Traces: appconfig-extension-and-association-lifecycle#teardown#13.

## Evidence
Wire replay (simparity-wire-aws-v1), all appconfig, all 'expected 404, got 200':

Cause 1 — appconfig-deployment-strategy-crud#teardown#6 GetDeploymentStrategy: $.Message missing (expected 'DeploymentStrategy with Id ho65pb1 could not be found.'), $headers.x-amzn-errortype missing, poll: status 200 not in [404] after 15 attempts, body {'Id': 'ho65pb1', 'Name': 'sim2real-probe-appconfig-strategy-b447beb6', 'DeploymentDurationInMinutes': 5, 'GrowthType': 'LINEAR', 'GrowthFactor': 20.0, 'FinalBakeTimeInMinutes': 1, 'ReplicateTo': 'NONE'}.
Cause 2 — appconfig-deployment-strategy-update-and-list-lifecycle#task#8 GetDeploymentStrategy: $.Message missing, $headers.x-amzn-errortype missing.
Cause 3 — appconfig-end-to-end-deployment-workflow#teardown#11 GetDeploymentStrategy: poll body {'Id': 'q92lkf0', ..., 'DeploymentDurationInMinutes': 0, 'GrowthFactor': 100.0}.
Cause 4 — appconfig-global-listings#teardown#5 GetDeploymentStrategy: status expected 404, got 200.
Cause 5 — appconfig-end-to-end-deployment-workflow#teardown#17 GetEnvironment: poll body {'Id': 'algcbw8', 'Name': 'sim2real-probe-appconfig-env-8a8c076c', 'State': 'READY_FOR_DEPLOYMENT'}.
Cause 6 — appconfig-environment-lifecycle-and-monitors#teardown#8 GetEnvironment: $.Message missing (expected 'Environment with Application Id ho65pb1 and Environment Id algcbw8 could not be found for account 000000000000.'), $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException:').
Cause 7 — appconfig-environment-lifecycle-and-monitors#teardown#9 UpdateEnvironment: status expected 404, got 200; $.Message missing; $headers.x-amzn-errortype missing (expected 'ResourceNotFoundException:').
Cause 8 — appconfig-extension-and-association-lifecycle#teardown#13 GetExtension: $.Message missing (expected 'Extension could not be found for account 000000000000.'), $headers.x-amzn-errortype missing.

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.
Causes 1-4: create_deployment_strategy(...) -> delete_deployment_strategy(DeploymentStrategyId=<id>) -> get_deployment_strategy(DeploymentStrategyId=<id>). Must raise ResourceNotFoundException (404).
Causes 5-6: create_application -> create_environment -> delete_environment -> get_environment. Must be 404 with x-amzn-errortype ResourceNotFoundException.
Cause 7: same setup, then update_environment(Description='x') on the deleted environment. Must be refused 404, not applied.
Cause 8: create_extension -> delete_extension -> get_extension. Must be 404.
Re-replay appconfig-deployment-strategy-crud#teardown#6 plus every step in trace_ids.
