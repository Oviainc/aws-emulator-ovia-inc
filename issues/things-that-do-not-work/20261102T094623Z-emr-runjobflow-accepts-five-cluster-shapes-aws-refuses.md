Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.core-fleet-six-instance-types-rejected emr.duplicate-master-placement-config-rejected emr.placement-group-on-two-master-group-rejected emr.primary-fleet-target-capacity-two-rejected emr.unknown-configuration-classification-rejected emr.fleets-and-instance-groups-together-rejected emr.step-concurrency-level-257-rejected emr.list-instance-groups-on-fleet-cluster-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.core-fleet-six-instance-types-rejected|_rejection#8|AssertionError|raise AssertionError(
Issue-Key: emr.duplicate-master-placement-config-rejected|_rejection#11|AssertionError|raise AssertionError(
Issue-Key: emr.placement-group-on-two-master-group-rejected|_rejection#9|AssertionError|raise AssertionError(
Issue-Key: emr.primary-fleet-target-capacity-two-rejected|_rejection#7|AssertionError|raise AssertionError(
Issue-Key: emr.unknown-configuration-classification-rejected|_rejection#9|AssertionError|raise AssertionError(
Issue-Key: emr.fleets-and-instance-groups-together-rejected|run#9|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: emr.step-concurrency-level-257-rejected|run#10|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: emr.list-instance-groups-on-fleet-cluster-rejected|_error_code_and_message#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: ebea418ed7238d7557ed4699ab6be023240b4505

# EMR RunJobFlow accepts five cluster shapes AWS refuses, and three cluster-plane refusals are emulator-authored (8 causes)

> _Note: this issue was identified against the code base at commit `ebea418ed7238d7557ed4699ab6be023240b4505`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
This issue bundles 8 net-new root causes in the EMR (elasticmapreduce) crate, all on the cluster-launch / cluster-read plane. Causes 1-5 are requests real EMR refuses synchronously that the emulator ACCEPTS with HTTP 200 and a created cluster; Causes 6-7 are refusals whose status and error code are right but whose Error.Message is text the emulator invented; Cause 8 is a read that answers 200 where AWS refuses. Each cause must be closed before submitting.

Shared Smithy grounding: RunJobFlow (services/service-com-amazonaws-emr-elasticmapreduce/smithy-model/com-amazonaws-emr-elasticmapreduce.json) models ONLY InternalServerError (server, 500) — the synchronous validation class AWS uses on this operation (ValidationException, HTTP 400) is unmodelled, so the status/code/message below are "recorded behavior only", taken from the recorded responses the checks assert against. In every message the fixed text is the requirement and the variable parts are named; the concrete fleet names, ids and values in the recordings are per-run values that only fill the template.

--- Cause 1 of 8: a CORE instance fleet listing six instance types is accepted (no fleet instance-type-count validation) ---
AWS: recorded behavior only. Without an allocation strategy a fleet may list at most five InstanceTypeConfigs; a sixth is refused with HTTP 400, code ValidationException and the message `The number of instance types in the instance fleet: <InstanceFleet.Name> exceeded the max value (5). Revise the configuration and resubmit.` Fixed text: `The number of instance types in the instance fleet: ` / ` exceeded the max value (5). Revise the configuration and resubmit.`; variable part: the offending fleet's Name from the request. No cluster is created.
Emulator: RunJobFlow answers 200 and mints a cluster (replay shows `RunJobFlow accepted the request (cluster j-...)`), and the cluster is then visible to ListClusters in a non-terminal state.
Why: a client that relies on the five-type limit being enforced at submit time gets a cluster it cannot get on real AWS; infrastructure code that treats 200 as "valid configuration" silently diverges, and the negative path of any fleet-sizing tool is untestable.
Fix: the fleet validation stage of the RunJobFlow handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/run_job_flow.rs), beside the existing fleets/groups and StepConcurrencyLevel gates, before any state is written.
Traces: emr.core-fleet-six-instance-types-rejected.

--- Cause 2 of 8: two PlacementGroupConfigs entries for the same InstanceRole are accepted ---
AWS: recorded behavior only. At most one placement-group config per instance role is allowed; two entries for the same role (even byte-identical ones) are refused 400 ValidationException with `Placement group supports single placement strategy for an instance role. Found multiple strategy for the following instance role: <InstanceRole>` — fixed text `Placement group supports single placement strategy for an instance role. Found multiple strategy for the following instance role: `; variable part: the duplicated InstanceRole (MASTER in the recording). No cluster is created.
Emulator: answers 200 and creates the cluster.
Why: a template or SDK caller that duplicates a role entry gets a cluster whose placement semantics are undefined instead of the refusal AWS gives, so the defect only surfaces on real AWS.
Fix: PlacementGroupConfigs validation in the RunJobFlow handler (run_job_flow.rs) / the shared placement-group validation helper in crate::state.
Traces: emr.duplicate-master-placement-config-rejected.

--- Cause 3 of 8: a MASTER placement group on a non-multi-master primary group is accepted ---
AWS: recorded behavior only. A MASTER placement-group config requires a 3-node (multi-master) primary group; on a 1- or 2-node primary group it is refused 400 ValidationException with the CONSTANT message `Placement group support for master node is available only for multi-master cluster and not for single-master cluster.` (no variable part). The same 2-node primary group WITHOUT a placement group is accepted — the check's own positive control.
Emulator: answers 200 and creates the cluster.
Why: the client never learns that its HA topology is invalid, and the cluster it gets back is one real EMR would never launch.
Fix: cross-field validation in the RunJobFlow handler (run_job_flow.rs): gate PlacementGroupConfigs with InstanceRole=MASTER on the requested primary instance count.
Traces: emr.placement-group-on-two-master-group-rejected.

--- Cause 4 of 8: a primary instance fleet with TargetOnDemandCapacity=2 is accepted ---
AWS: recorded behavior only. The primary (MASTER) instance fleet may target only 1 or 3 units; any other value is refused 400 ValidationException with the CONSTANT message `The primary instance fleet can only have a target capacity of 1 or 3. Revise the configuration and resubmit.` (no variable part).
Emulator: answers 200 and creates the cluster.
Why: a caller gets a primary fleet capacity AWS rejects; the 1-or-3 rule is invisible until the same template is run against AWS.
Fix: primary-fleet target-capacity validation in the RunJobFlow handler (run_job_flow.rs), covering TargetOnDemandCapacity and TargetSpotCapacity on the MASTER fleet.
Traces: emr.primary-fleet-target-capacity-two-rejected.

--- Cause 5 of 8: a Configurations entry with an unknown Classification is accepted ---
AWS: recorded behavior only. A classification name that does not exist for the requested release is refused 400 ValidationException with `Classification '<Classification>' is not valid for parent classification 'null'.` — fixed text `Classification '` / `' is not valid for parent classification '` / `'.`; variable parts: the offending Classification from the request and the parent classification (the literal `null` for a top-level entry). No cluster is created.
Emulator: answers 200 and creates the cluster, so a typo'd classification is persisted and later served back by DescribeCluster.
Why: the single most common EMR misconfiguration (a misspelled classification such as `spark-defaultz`) is silently accepted, so a client cannot distinguish a valid configuration set from a typo.
Fix: Configurations validation in the RunJobFlow handler (run_job_flow.rs) against the release's classification catalog held in crate::state.
Traces: emr.unknown-configuration-classification-rejected.

--- Cause 6 of 8: the InstanceFleets+InstanceGroups mutual-exclusion refusal message is the emulator's own sentence ---
AWS: the RunJobFlow doc in the Smithy model states the rule ("The RunJobFlow request can contain InstanceFleets parameters or InstanceGroups parameters, but not both"); the message is recorded behavior only: 400 ValidationException with the CONSTANT message `Please configure instances using one and only one of the following: instance groups; instance fleets; instance count, master and slave instance type.` (no variable part).
Emulator: right status and code, but the message is `Instances may carry InstanceGroups or InstanceFleets, but not both` — authored at run_job_flow.rs in the `has_groups && has_fleets` branch.
Why: a client, CLI wrapper or runbook that matches AWS's sentence (it is the sentence that tells the user the THIRD legal form — instance count + master/slave type) does not recognise the refusal.
Fix: that branch of the RunJobFlow handler (run_job_flow.rs) — emit AWS's constant sentence.
Traces: emr.fleets-and-instance-groups-together-rejected.

--- Cause 7 of 8: the StepConcurrencyLevel above-maximum refusal message is the emulator's own sentence ---
AWS: Smithy: RunJobFlow.StepConcurrencyLevel is an unconstrained Integer (its doc states "The default value is 1. The maximum value is 256"), so the message is recorded behavior only: a value above the maximum under the default StepExecutionMode is refused 400 ValidationException with `StepConcurrencyLevel <value> exceeds the maximum (256) for StepExecutionMode STANDARD. Set StepExecutionMode to DISTRIBUTED for up to 6400 concurrent steps.` — fixed text `StepConcurrencyLevel ` / ` exceeds the maximum (256) for StepExecutionMode STANDARD. Set StepExecutionMode to DISTRIBUTED for up to 6400 concurrent steps.`; variable part: the submitted StepConcurrencyLevel. The boundary value 256 is accepted and reads back as 256 (the check's positive control).
Emulator: right status and code, message `StepConcurrencyLevel 257 is outside 1..256` — a different sentence rendering the bounds in Rust range syntax, built at run_job_flow.rs in the MAX_STEP_CONCURRENCY_LEVEL check.
Why: the client loses the actionable half of AWS's message (switch StepExecutionMode to DISTRIBUTED for up to 6400) and cannot match the refusal.
Fix: the StepConcurrencyLevel range check in the RunJobFlow handler (run_job_flow.rs). Note the open wire issue 20261029T212242Z-elasticmapreduce-emr-validation-refusals-carry-the-wrong-error-code.md (Cause 9) covers the BELOW-minimum case, whose recorded sentence is the different constant `Invalid step concurrency level. Allowed level is from 1 to 256`; the above-maximum sentence recorded here is a separate template and that issue carries no key for it.
Traces: emr.step-concurrency-level-257-rejected.

--- Cause 8 of 8: ListInstanceGroups on an instance-fleet cluster answers 200 instead of refusing ---
AWS: Smithy: ListInstanceGroups(ClusterId required) models InvalidRequestException (client, HTTP 400). Recorded behavior for the text: on a cluster that uses instance FLEETS the operation is refused 400 InvalidRequestException with the CONSTANT message `Instance fleets and instance groups are mutually exclusive. The EMR cluster specified in the request uses instance fleets. The ListInstanceGroups operation does not support clusters that use instance fleets. Use the ListInstanceFleets operation instead.` (no variable part). ListInstanceFleets on the same cluster succeeds and reports both fleets (the check's positive control, which the emulator passes).
Emulator: ListInstanceGroups succeeds on a fleet cluster (replay: `the call succeeded; real AWS rejects it, so there is no error code`), so the emulator presents instance groups for a cluster that has none.
Why: a caller probing which topology a cluster uses by calling ListInstanceGroups and catching the refusal concludes the cluster is group-based, and then drives group-only operations (ModifyInstanceGroups, PutAutoScalingPolicy) at a fleet cluster.
Fix: the ListInstanceGroups handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/list_instance_groups.rs) — refuse when the stored ClusterRecord carries instance fleets, with the constant sentence above.
Traces: emr.list-instance-groups-on-fleet-cluster-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (service heading `emr`, 33/217 checks passing). All eight checks below ran their positive control successfully first, so each failure isolates the one broken field.

Cause 1 — emr.core-fleet-six-instance-types-rejected: expected ('ValidationException', 'The number of instance types in the instance fleet: CoreInstanceFleet exceeded the max value (5). Revise the configuration and resubmit.'); got AssertionError `RunJobFlow accepted the request (cluster j-0000000000002); real AWS rejects it`.
Cause 2 — emr.duplicate-master-placement-config-rejected: PlacementGroupConfigs=[{InstanceRole:MASTER,PlacementStrategy:SPREAD}]*2 expected ('ValidationException', 'Placement group supports single placement strategy for an instance role. Found multiple strategy for the following instance role: MASTER'); got `RunJobFlow accepted the request (cluster j-0000000000002)`.
Cause 3 — emr.placement-group-on-two-master-group-rejected: a 2-node MASTER group + MASTER placement group expected ('ValidationException', 'Placement group support for master node is available only for multi-master cluster and not for single-master cluster.'); got `RunJobFlow accepted the request (cluster j-0000000000002)`. The same shape without the placement group was accepted as the control.
Cause 4 — emr.primary-fleet-target-capacity-two-rejected: Instances.InstanceFleets[0].TargetOnDemandCapacity=2 expected ('ValidationException', 'The primary instance fleet can only have a target capacity of 1 or 3. Revise the configuration and resubmit.'); got `RunJobFlow accepted the request (cluster j-0000000000002)`.
Cause 5 — emr.unknown-configuration-classification-rejected: Configurations += {Classification:'spark-defaultz',Properties:{a:b}} expected ('ValidationException', "Classification 'spark-defaultz' is not valid for parent classification 'null'."); got `RunJobFlow accepted the request (cluster j-0000000000002)`.
Cause 6 — emr.fleets-and-instance-groups-together-rejected: expected ValidationException 'Please configure instances using one and only one of the following: instance groups; instance fleets; instance count, master and slave instance type.'; got 'ValidationException' 'Instances may carry InstanceGroups or InstanceFleets, but not both'.
Cause 7 — emr.step-concurrency-level-257-rejected: StepConcurrencyLevel=257 expected ValidationException 'StepConcurrencyLevel 257 exceeds the maximum (256) for StepExecutionMode STANDARD. Set StepExecutionMode to DISTRIBUTED for up to 6400 concurrent steps.'; got 'ValidationException' 'StepConcurrencyLevel 257 is outside 1..256'.
Cause 8 — emr.list-instance-groups-on-fleet-cluster-rejected: ListInstanceGroups on a MASTER+CORE fleet cluster expected ('InvalidRequestException', 'Instance fleets and instance groups are mutually exclusive. ... Use the ListInstanceFleets operation instead.'); got AssertionError `the call succeeded; real AWS rejects it, so there is no error code` (ListInstanceFleets on the same cluster reported both fleets).

Source grounding for Causes 6 and 7: services/service-com-amazonaws-emr-elasticmapreduce/src/ops/run_job_flow.rs lines ~340-344 (the `has_groups && has_fleets` refusal text) and ~400-407 (the `StepConcurrencyLevel ... is outside ...` refusal text). The same handler contains no fleet instance-type-count, placement-group, primary-fleet-capacity or Configurations-classification gate at all.

## Steps to Reproduce
Boot the emulator and replay these checks from suite simparity-behavioral-aws-v1 (service heading `emr`):
1. emr.core-fleet-six-instance-types-rejected — RunJobFlow with a CORE InstanceFleet carrying six InstanceTypeConfigs and no allocation strategy; assert 400 ValidationException with the exceeded-max-value sentence and that ListClusters shows no cluster of that Name.
2. emr.duplicate-master-placement-config-rejected — RunJobFlow with PlacementGroupConfigs holding the same {InstanceRole:MASTER} entry twice.
3. emr.placement-group-on-two-master-group-rejected — RunJobFlow with a 2-instance MASTER instance group plus a MASTER PlacementGroupConfig (the same request without the placement group must still be accepted).
4. emr.primary-fleet-target-capacity-two-rejected — RunJobFlow with the MASTER fleet's TargetOnDemandCapacity set to 2.
5. emr.unknown-configuration-classification-rejected — RunJobFlow with a Configurations entry whose Classification does not exist (e.g. 'spark-defaultz').
6. emr.fleets-and-instance-groups-together-rejected — RunJobFlow with both Instances.InstanceGroups and Instances.InstanceFleets populated; compare Error.Message.
7. emr.step-concurrency-level-257-rejected — RunJobFlow with StepConcurrencyLevel=257 (and 256 as the accepted boundary control); compare Error.Message.
8. emr.list-instance-groups-on-fleet-cluster-rejected — launch an instance-FLEET cluster, call ListInstanceFleets (must succeed), then ListInstanceGroups on the same ClusterId and assert the 400 InvalidRequestException mutual-exclusion sentence.
