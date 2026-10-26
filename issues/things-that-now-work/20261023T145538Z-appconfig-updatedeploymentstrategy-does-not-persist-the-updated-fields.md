Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-deployment-strategy-crud
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-deployment-strategy-crud appconfig-deployment-strategy-update-and-list-lifecycle appconfig-update-deployment-strategy-persists-then-not-found
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|GetDeploymentStrategy|2xx->2xx|$.GrowthFactor
Issue-Key: appconfig|GetDeploymentStrategy|2xx->2xx|$.DeploymentDurationInMinutes
Issue-Key: appconfig|GetDeploymentStrategy|2xx->2xx|$.FinalBakeTimeInMinutes
Issue-Key: appconfig|GetDeploymentStrategy|2xx->2xx|$.Description
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# AppConfig UpdateDeploymentStrategy does not persist the updated fields

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDeploymentStrategy

## Description
A PATCH to a deployment strategy returns success, but a later GetDeploymentStrategy still reports the pre-update values: the mutation is dropped instead of being written to state. Three replayed clusters, differing in which members were updated. Numbered; close ALL THREE before submitting.

=== Cause 1 of 3: GrowthFactor update is lost ===
AWS: UpdateDeploymentStrategy is PATCH /deploymentstrategies/{DeploymentStrategyId}, HTTP 200, output DeploymentStrategy; the members it accepts (Description, DeploymentDurationInMinutes, FinalBakeTimeInMinutes, GrowthFactor @range 1..100, GrowthType) overwrite the stored strategy, so a following GetDeploymentStrategy serves the new values (Smithy: UpdateDeploymentStrategy input/output as above; GetDeploymentStrategy output DeploymentStrategy.GrowthFactor target Percentage).
Emulator: GetDeploymentStrategy after the update still reports the creation-time GrowthFactor (20.0 where 50.0 was set and acknowledged).
Why: a client that tunes a rollout and reads it back sees its change silently discarded, and any deployment started afterwards rolls out at the old rate — in the emulator only.
Fix: UpdateDeploymentStrategy's handler must write the supplied members back into the deployment-strategy record in appconfig state (and return the merged record), rather than echoing input while leaving state untouched.
Traces: appconfig-deployment-strategy-crud#task#4.

=== Cause 2 of 3: DeploymentDurationInMinutes, FinalBakeTimeInMinutes and GrowthFactor updates are all lost ===
AWS: as cause 1 — all three members persist (DeploymentDurationInMinutes/FinalBakeTimeInMinutes target MinutesBetween0And24Hours @range 0..1440).
Emulator: Get after update reports the original 10 / 0 / 10.0 instead of the updated 20 / 5 / 25.0.
Why: the whole strategy is stale, not one field, so a rollout's duration and bake time are wrong too.
Fix: as cause 1 (all PATCH-able members, not a subset).
Traces: appconfig-deployment-strategy-update-and-list-lifecycle#task#4.

=== Cause 3 of 3: the updated Description is dropped entirely ===
AWS: as cause 1, and Description (@length 0..1024) is present in the Get response after the update.
Emulator: Description is MISSING from the Get response, and DeploymentDurationInMinutes/FinalBakeTimeInMinutes/GrowthFactor again report pre-update values (10 / 0 / 5.0 instead of 20 / 5 / 25.0).
Why: in addition to the lost mutation, a member AWS returns is absent from the document, so a client that reads Description unconditionally gets a key error.
Fix: as cause 1, plus make the DeploymentStrategy serializer emit Description when the record has one.
Traces: appconfig-update-deployment-strategy-persists-then-not-found#task#3.

## Evidence
Wire replay (simparity-wire-aws-v1), all appconfig GetDeploymentStrategy, expected 200 / actual 200:

Cause 1 — appconfig-deployment-strategy-crud#task#4: $.GrowthFactor value_mismatch (expected 50.0, got 20.0).
Cause 2 — appconfig-deployment-strategy-update-and-list-lifecycle#task#4: $.DeploymentDurationInMinutes (expected 20, got 10), $.FinalBakeTimeInMinutes (expected 5, got 0), $.GrowthFactor (expected 25.0, got 10.0).
Cause 3 — appconfig-update-deployment-strategy-persists-then-not-found#task#3: $.DeploymentDurationInMinutes (expected 20, got 10), $.Description missing (expected 'updated-strategy', got None), $.FinalBakeTimeInMinutes (expected 5, got 0), $.GrowthFactor (expected 25.0, got 5.0).

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.
1. create_deployment_strategy(Name=..., DeploymentDurationInMinutes=10, GrowthFactor=10, FinalBakeTimeInMinutes=0, ReplicateTo='NONE').
2. update_deployment_strategy(DeploymentStrategyId=<id>, Description='updated-strategy', DeploymentDurationInMinutes=20, FinalBakeTimeInMinutes=5, GrowthFactor=25).
3. get_deployment_strategy(DeploymentStrategyId=<id>).
The Get must report the updated duration/bake time/growth factor and the Description; the emulator reports the creation-time values and omits Description.
Re-replay appconfig-deployment-strategy-crud#task#4 plus both steps in trace_ids.
