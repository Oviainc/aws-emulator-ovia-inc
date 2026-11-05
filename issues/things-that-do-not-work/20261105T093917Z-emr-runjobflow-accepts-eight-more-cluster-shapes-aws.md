Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.core-fleet-zero-target-capacity-rejected emr.master-fleet-on-demand-plus-spot-rejected emr.master-placement-non-spread-strategy-rejected emr.multi-master-cluster-without-core-group-rejected emr.nested-classification-under-spark-defaults-rejected emr.placement-group-for-task-role-rejected emr.placement-group-on-single-master-rejected emr.three-masters-on-pre-multi-master-release-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.core-fleet-zero-target-capacity-rejected|_rejection#7|AssertionError|raise AssertionError(
Issue-Key: emr.master-fleet-on-demand-plus-spot-rejected|_rejection#7|AssertionError|raise AssertionError(
Issue-Key: emr.master-placement-non-spread-strategy-rejected|_rejected_error#8|AssertionError|raise AssertionError(
Issue-Key: emr.multi-master-cluster-without-core-group-rejected|_rejected#10|AssertionError|raise AssertionError(
Issue-Key: emr.nested-classification-under-spark-defaults-rejected|_rejection#7|AssertionError|raise AssertionError(
Issue-Key: emr.placement-group-for-task-role-rejected|_rejected#10|AssertionError|raise AssertionError(
Issue-Key: emr.placement-group-on-single-master-rejected|_rejected_error#9|AssertionError|raise AssertionError(
Issue-Key: emr.three-masters-on-pre-multi-master-release-rejected|_rejection#12|AssertionError|raise AssertionError(
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# EMR RunJobFlow accepts eight more cluster shapes AWS refuses with ValidationException (fleet capacity, placement groups, nested classifications, multi-master release gates)

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
RunJobFlow answers 200 with a JobFlowId for eight distinct request shapes AWS refuses at the API with a 400 ValidationException and a fixed sentence. In each check an unmodified control request is accepted first, so the fresh instance profile, service role and subnet are known good and the one mutated field is the only fault. Smithy: RunJobFlow's modelled error list carries only InternalServerError (httpError 500), so the ValidationException envelope and these messages are recorded behavior only; RunJobFlow.Instances -> JobFlowInstancesConfig and RunJobFlow.PlacementGroupConfigs -> PlacementGroupConfigList are the members involved.

THIS ISSUE BUNDLES 8 CAUSES. All eight must be closed.

Cause 1 of 8: a CORE instance fleet with both targets 0 is accepted.
  AWS: 400 ValidationException "The instance fleet (CoreInstanceFleet) should have a value of one or greater for either targetOnDemandCapacity or targetSpotCapacity." — fixed text with the offending fleet named from its InstanceFleetType ('CoreInstanceFleet'). Smithy: InstanceFleet.TargetOnDemandCapacity/TargetSpotCapacity -> WholeNumber (range min 0), documented as 'At least one of TargetSpotCapacity and TargetOnDemandCapacity should be greater than 0'.
  Emulator: 200, cluster created.
  Why: a client gets a cluster whose core fleet can never provision capacity instead of being told its request is empty.
  Fix: RunJobFlow's instance-fleet validation.
  Traces: emr.core-fleet-zero-target-capacity-rejected.

Cause 2 of 8: a MASTER fleet asking for one On-Demand AND one Spot unit is accepted.
  AWS: 400 ValidationException "Configure the master instance fleet using either targetOnDemandCapacity or targetSpotCapacity, but not both." — entirely fixed text. Smithy: the InstanceFleet target members document 'For a master instance fleet, only one of TargetSpotCapacity and TargetOnDemandCapacity can be specified, and its value must be 1'.
  Emulator: 200, cluster created.
  Why: the client believes a mixed-market primary fleet is supported.
  Fix: RunJobFlow's instance-fleet validation (primary fleet market rule).
  Traces: emr.master-fleet-on-demand-plus-spot-rejected.

Cause 3 of 8: a MASTER placement-group config with a strategy other than SPREAD/NONE is accepted.
  AWS: 400 ValidationException "Placement group for master node supports only the following placement strategy: [SPREAD, NONE]" — entirely fixed text. Smithy: PlacementGroupConfig.PlacementStrategy -> PlacementGroupStrategy, documented 'the only supported placement strategy is SPREAD for the MASTER instance role'.
  Emulator: 200, cluster created.
  Fix: RunJobFlow's PlacementGroupConfigs validation.
  Why: an unsupported strategy silently becomes cluster state no AWS cluster can hold.
  Traces: emr.master-placement-non-spread-strategy-rejected.

Cause 4 of 8: a 3-primary (HA) cluster with no CORE instance group is accepted.
  AWS: 400 ValidationException "A HA based cluster must have core instance group. Please specify a core instance group and resubmit." — entirely fixed text.
  Emulator: 200, cluster created.
  Why: a client gets an 'HA' cluster that cannot hold HDFS, instead of the refusal that tells it what is missing.
  Fix: RunJobFlow's instance-group validation (multi-primary shape rules).
  Traces: emr.multi-master-cluster-without-core-group-rejected.

Cause 5 of 8: a classification nested under a parent that does not accept it is accepted.
  AWS: 400 ValidationException "Classification 'export' is not valid for parent classification 'spark-defaults'." — fixed text "Classification '" + "' is not valid for parent classification '" + "'." with the child and parent classification names taken from the request's Configurations tree. Smithy: Configuration.Classification / Configuration.Configurations (ConfigurationList).
  Emulator: 200, cluster created.
  Why: a mis-nested configuration is silently stored and never applied, so the client debugs a cluster that AWS would have refused outright.
  Fix: RunJobFlow's Configurations validation (parent/child classification table).
  Traces: emr.nested-classification-under-spark-defaults-rejected.

Cause 6 of 8: a placement-group config for a non-MASTER instance role is accepted.
  AWS: 400 ValidationException "Placement group supports only the following instance role: [MASTER]" — entirely fixed text. Smithy: PlacementGroupConfig.InstanceRole -> InstanceRoleType (required), documented 'the only supported instance role is MASTER'.
  Emulator: 200, cluster created with a TASK placement-group config.
  Fix: RunJobFlow's PlacementGroupConfigs validation.
  Traces: emr.placement-group-for-task-role-rejected.

Cause 7 of 8: a MASTER placement group on a cluster whose primary count is not 3 is accepted.
  AWS: 400 ValidationException "Placement group support for master node is available only for multi-master cluster and not for single-master cluster." — entirely fixed text, and the same message for a 1-primary and a 4-primary request.
  Emulator: 200, cluster created for both the 1-primary and 4-primary shapes.
  Fix: RunJobFlow's PlacementGroupConfigs validation, cross-checked against the primary instance count.
  Traces: emr.placement-group-on-single-master-rejected.

Cause 8 of 8: three primaries on a release older than multi-master support is accepted.
  AWS: 400 ValidationException "A master instance group must specify a single instance." — entirely fixed text; multi-primary clusters require emr-5.23.0 or later, so a 3-instance primary group on (for example) emr-5.20.0 is refused. Smithy: RunJobFlow.ReleaseLabel -> XmlStringMaxLen256.
  Emulator: 200, cluster created.
  Why: a client is handed a cluster shape the requested release cannot run, and only discovers it at provisioning time (or never, in the emulator).
  Fix: RunJobFlow's release-capability gate for the primary instance count.
  Traces: emr.three-masters-on-pre-multi-master-release-rejected.

Note: open issue 20261102T094623Z-emr-runjobflow-accepts-five-cluster-shapes-aws-refuses.md covers five OTHER shapes (and owns those checks' replay keys); none of the eight rules above is one of them.

## Evidence
Replay of simparity-behavioral-aws-v1, '### emr'. Each check accepts an unmodified control request, mutates one field, and asserts the refusal; all eight fail identically with 'RunJobFlow accepted the request (cluster j-0000000000002); real AWS rejects it' — the emulator returned 200 and a JobFlowId where a 400 ValidationException and the quoted sentence were expected:

1. emr.core-fleet-zero-target-capacity-rejected — expected ('ValidationException', 'The instance fleet (CoreInstanceFleet) should have a value of one or greater for either targetOnDemandCapacity or targetSpotCapacity.').
2. emr.master-fleet-on-demand-plus-spot-rejected — expected ('ValidationException', 'Configure the master instance fleet using either targetOnDemandCapacity or targetSpotCapacity, but not both.').
3. emr.master-placement-non-spread-strategy-rejected — expected ('ValidationException', 'Placement group for master node supports only the following placement strategy: [SPREAD, NONE]').
4. emr.multi-master-cluster-without-core-group-rejected — expected ('ValidationException', 'A HA based cluster must have core instance group. Please specify a core instance group and resubmit.').
5. emr.nested-classification-under-spark-defaults-rejected — expected ('ValidationException', "Classification 'export' is not valid for parent classification 'spark-defaults'.").
6. emr.placement-group-for-task-role-rejected — expected ('ValidationException', 'Placement group supports only the following instance role: [MASTER]').
7. emr.placement-group-on-single-master-rejected — expected ('ValidationException', 'Placement group support for master node is available only for multi-master cluster and not for single-master cluster.') for both a 1-primary and a 4-primary request.
8. emr.three-masters-on-pre-multi-master-release-rejected — expected ('ValidationException', 'A master instance group must specify a single instance.') for a 3-instance primary group on emr-5.20.0.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator.
2. Create an EMR service role, an EC2 instance profile and a subnet, and confirm RunJobFlow accepts the unmodified request (the positive control each check runs first).
3. Re-send the same request with ONE field mutated and compare the response with the sentence quoted above:
   a. CORE InstanceFleet with TargetOnDemandCapacity=0 and TargetSpotCapacity=0;
   b. MASTER InstanceFleet with TargetOnDemandCapacity=1 and TargetSpotCapacity=1;
   c. PlacementGroupConfigs=[{InstanceRole: MASTER, PlacementStrategy: <not SPREAD/NONE>}];
   d. three primary instances and no CORE instance group;
   e. Configurations with {Classification: 'export'} nested under 'spark-defaults';
   f. PlacementGroupConfigs=[{InstanceRole: TASK, PlacementStrategy: SPREAD}];
   g. PlacementGroupConfigs=[{InstanceRole: MASTER, …}] on a 1-primary and on a 4-primary cluster;
   h. a 3-instance primary group with ReleaseLabel=emr-5.20.0.
4. Each must answer 400 ValidationException with the quoted message; the emulator answers 200 with a JobFlowId.
5. Replay the eight checks listed in trace_ids to confirm.
