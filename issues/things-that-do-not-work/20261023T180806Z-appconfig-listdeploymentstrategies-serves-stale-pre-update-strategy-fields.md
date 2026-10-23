Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-deployment-strategy-update-and-list-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-deployment-strategy-update-and-list-lifecycle
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|ListDeploymentStrategies|2xx->2xx|$.Items[Id="ho65pb1"].DeploymentDurationInMinutes
Issue-Key: appconfig|ListDeploymentStrategies|2xx->2xx|$.Items[Id="ho65pb1"].FinalBakeTimeInMinutes
Issue-Key: appconfig|ListDeploymentStrategies|2xx->2xx|$.Items[Id="ho65pb1"].GrowthFactor
Issue-Commit: d46c9ea0387b1ea18140666d9f1aeb4877a1d699

# AppConfig ListDeploymentStrategies serves stale pre-update strategy fields

> _Note: this issue was identified against the code base at commit `d46c9ea0387b1ea18140666d9f1aeb4877a1d699`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDeploymentStrategies

## Description
The collection listing of deployment strategies serves creation-time values for a strategy that was updated, so the list view of a strategy disagrees with the mutation that was acknowledged. One cause.

=== Cause 1 of 1: the listed DeploymentStrategy item reports pre-update DeploymentDurationInMinutes / FinalBakeTimeInMinutes / GrowthFactor ===
AWS: ListDeploymentStrategies is GET /deploymentstrategies, HTTP 200, output DeploymentStrategies with Items targeting DeploymentStrategyList, i.e. the same DeploymentStrategy shape UpdateDeploymentStrategy returns (Smithy: ListDeploymentStrategies output Items -> DeploymentStrategyList; UpdateDeploymentStrategy PATCH /deploymentstrategies/{DeploymentStrategyId} -> DeploymentStrategy with members Description @length 0..1024, DeploymentDurationInMinutes and FinalBakeTimeInMinutes -> MinutesBetween0And24Hours @range 0..1440, GrowthFactor @range 1..100, GrowthType). Because the PATCH overwrites the stored strategy, the listing enumerates the updated record, so the item for an updated strategy carries the new duration, bake time and growth factor.
Emulator: after an acknowledged UpdateDeploymentStrategy, the listing's item for that strategy still carries the creation-time values: DeploymentDurationInMinutes 10 where 20 was set, FinalBakeTimeInMinutes 0 where 5 was set, GrowthFactor 10.0 where 25.0 was set. Status is 200 as expected; only the item's members diverge.
Why: a client that reconciles rollout configuration by listing strategies (the usual path for a console or a drift check, which lists rather than fetching each id) sees the update as never having happened, and will either re-apply it forever or compute a rollout plan from the stale duration and growth factor.
Fix: the appconfig state holding deployment strategies, and the ListDeploymentStrategies handler that projects it — the listing must serialize the stored record that UpdateDeploymentStrategy mutates, not a snapshot captured at create time. Hypothesis: the same dropped mutation reported for the single-resource read in the open issue 20261023T145538Z-appconfig-updatedeploymentstrategy-does-not-persist-the-updated-fields.md; that issue's replay keys cover GetDeploymentStrategy only and do not include this ListDeploymentStrategies step, so this divergence is filed separately and must be re-replayed in its own right even if that one is closed.
Traces: appconfig-deployment-strategy-update-and-list-lifecycle#task#5.

## Evidence
Wire replay (simparity-wire-aws-v1), service appconfig, operation ListDeploymentStrategies, expected status 200 / actual status 200, fixture appconfig-deployment-strategy-update-and-list-lifecycle step task#5:
  $.Items[Id="ho65pb1"].DeploymentDurationInMinutes: value_mismatch (expected 20, got 10)
  $.Items[Id="ho65pb1"].FinalBakeTimeInMinutes: value_mismatch (expected 5, got 0)
  $.Items[Id="ho65pb1"].GrowthFactor: value_mismatch (expected 25.0, got 10.0)
Cluster size 1 (no other affected steps). Replay summary for this run: emulator up, bootstrap_signal false, state_reset_failures 0, so the stale values are not an artifact of leaked fixture state.

## Steps to Reproduce
Boot the emulator and point an appconfig client at it.
1. create_deployment_strategy(Name=<name>, DeploymentDurationInMinutes=10, GrowthFactor=10, FinalBakeTimeInMinutes=0, ReplicateTo='NONE') -> note the returned Id.
2. update_deployment_strategy(DeploymentStrategyId=<id>, DeploymentDurationInMinutes=20, FinalBakeTimeInMinutes=5, GrowthFactor=25) -> 200.
3. list_deployment_strategies() and find Items[Id=<id>].
Expected: that item reports DeploymentDurationInMinutes 20, FinalBakeTimeInMinutes 5, GrowthFactor 25.0. Observed: 10, 0 and 10.0.
Re-replay appconfig-deployment-strategy-update-and-list-lifecycle#task#5 to confirm the fix.
