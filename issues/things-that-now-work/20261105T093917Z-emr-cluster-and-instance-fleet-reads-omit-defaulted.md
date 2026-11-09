Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.omitted-scaling-and-config-fields-take-documented-defaults emr.nested-configuration-without-properties-returns-empty-map emr.spark-cluster-applications-resolve-release-versions emr.fleet-default-ebs-volumes-applied-for-m5-2xlarge emr.fleet-explicit-ebs-config-expands-volumes-per-instance emr.spark-fleet-cluster-reports-two-single-unit-fleets emr.spark-fleet-cluster-attributes-and-core-resize-default emr.placement-strategy-none-round-trips-as-empty
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.omitted-scaling-and-config-fields-take-documented-defaults|run#9|AssertionError|assert got == want, f"omitted fields should take the defaults {want!r}, got {got!r}"
Issue-Key: emr.nested-configuration-without-properties-returns-empty-map|run#8|AssertionError|assert env.get("Properties") == {}, (
Issue-Key: emr.spark-cluster-applications-resolve-release-versions|run#6|AssertionError|assert got == want, f"{RELEASE_LABEL} should resolve the applications to {want!r}, got {got!r}"
Issue-Key: emr.fleet-default-ebs-volumes-applied-for-m5-2xlarge|run#4|AssertionError|assert vols == [("gp2", 32)] * 4, (
Issue-Key: emr.fleet-explicit-ebs-config-expands-volumes-per-instance|run#10|AssertionError|assert vols["MASTER"] == ([("gp2", 32)] * 4, "<absent>"), (
Issue-Key: emr.spark-fleet-cluster-reports-two-single-unit-fleets|run#6|AssertionError|assert specs == [(INSTANCE_TYPE, 1, 100.0)], (
Issue-Key: emr.spark-fleet-cluster-attributes-and-core-resize-default|run#11|AssertionError|assert core_resize == {"SpotResizeSpecification": {"TimeoutDurationMinutes": 60}}, (
Issue-Key: emr.placement-strategy-none-round-trips-as-empty|run#9|AssertionError|assert got == [], f"strategy NONE should be stored as PlacementGroups == [], got {got!r}"
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# EMR cluster and instance-fleet reads omit defaulted members: no ScaleDownBehavior/Configurations, no application versions, no fleet EBS/weight/bid/resize defaults, and PlacementStrategy NONE is echoed back

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Seven representation defects: the emulator stores only what the request named and serves it back, so every member AWS fills in from a default or resolves from the release label is absent, empty or wrong. THIS ISSUE BUNDLES 7 CAUSES. All seven must be closed.

Cause 1 of 7: DescribeCluster omits the members AWS defaults when the request omits them.
  AWS: a cluster launched without ScaleDownBehavior, UnhealthyNodeReplacement, StepConcurrencyLevel or Configurations is described with all four present — ScaleDownBehavior taking the release's documented default (TERMINATE_AT_TASK_COMPLETION for the release under test), UnhealthyNodeReplacement true, StepConcurrencyLevel 1 and Configurations an empty list. Smithy: Cluster.ScaleDownBehavior -> ScaleDownBehavior (enum), Cluster.UnhealthyNodeReplacement -> BooleanObject, Cluster.Configurations -> ConfigurationList, Cluster.StepConcurrencyLevel -> Integer; the default VALUES are recorded behavior/documentation, the requirement is that the members are always present.
  Emulator: ScaleDownBehavior absent, UnhealthyNodeReplacement false, Configurations absent (StepConcurrencyLevel 1 is correct).
  Why: a client reading the effective cluster configuration cannot tell a default from an unset value, and gets the opposite boolean for node replacement.
  Fix: the RunJobFlow -> cluster state defaulting and the DescribeCluster serializer.
  Traces: emr.omitted-scaling-and-config-fields-take-documented-defaults.

Cause 2 of 7: a nested Configuration declared with no Properties is served without the member instead of with an empty map.
  AWS: a child Configuration (e.g. 'spark-env' nested under a parent) that declares no Properties is reported with Properties as an empty map. Smithy: Configuration.Properties -> StringMap.
  Emulator: the member is absent ('<absent>').
  Why: a client walking the configuration tree has to special-case a missing map where AWS always gives it one.
  Fix: the Configuration serializer used by DescribeCluster/ListInstanceGroups.
  Traces: emr.nested-configuration-without-properties-returns-empty-map.

Cause 3 of 7: installed applications carry no Version.
  AWS: DescribeCluster's Applications resolve each requested application to the version the release ships — for emr-5.36.1: Spark 2.4.8, Zeppelin 0.10.0, Hadoop 2.10.1, Pig 0.17.0. Smithy: Application.Name / Application.Version (both String).
  Emulator: every Application's Version is null.
  Why: a client that pins behavior to the application version installed on the release gets nothing to read.
  Fix: the release-label -> application-version catalog consulted when RunJobFlow records the cluster's Applications.
  Traces: emr.spark-cluster-applications-resolve-release-versions.

Cause 4 of 7: instance fleets get no default EBS volumes, and an explicit EbsConfiguration is not expanded per instance.
  AWS: an instance type in a fleet that declares no EbsConfiguration is described with the instance type's default EBS layout as EbsBlockDevices (for m5.2xlarge: 4 volumes of 32 GiB gp2), and a fleet that declares an EbsConfiguration has it expanded into one EbsBlockDevices entry per requested volume, while a role that declared nothing keeps the default layout. Smithy: InstanceTypeSpecification.EbsBlockDevices -> EbsBlockDeviceList.
  Emulator: EbsBlockDevices is [] for the undeclared MASTER fleet, and the undeclared role's layout stays empty in the explicit-config case ([], '<absent>').
  Why: a client sizing storage or asserting on the provisioned layout sees a fleet with no disks.
  Fix: the instance-fleet recording path (default-EBS table per instance type, plus expansion of a declared EbsConfiguration) and the ListInstanceFleets/DescribeCluster serializer.
  Traces: emr.fleet-default-ebs-volumes-applied-for-m5-2xlarge, emr.fleet-explicit-ebs-config-expands-volumes-per-instance.

Cause 5 of 7: InstanceTypeSpecifications omit WeightedCapacity and BidPriceAsPercentageOfOnDemandPrice.
  AWS: each InstanceTypeSpecification reports its WeightedCapacity (1 when the request omitted it) and BidPriceAsPercentageOfOnDemandPrice, which defaults to 100 when neither BidPrice nor the percentage was supplied. Smithy: InstanceTypeSpecification.WeightedCapacity -> WholeNumber; InstanceTypeSpecification.BidPriceAsPercentageOfOnDemandPrice -> NonNegativeDouble, documented 'If neither BidPrice nor BidPriceAsPercentageOfOnDemandPrice is provided, BidPriceAsPercentageOfOnDemandPrice defaults to 100%'.
  Emulator: both are null — the fleet reports [('m5.2xlarge', None, None)].
  Why: capacity accounting and spot-bid behavior are unreadable; a client cannot compute provisioned units.
  Fix: the instance-fleet recording defaults and the fleet serializer.
  Traces: emr.spark-fleet-cluster-reports-two-single-unit-fleets.

Cause 6 of 7: a fleet's spot resize timeout default is absent.
  AWS: a CORE instance fleet created without ResizeSpecifications reports ResizeSpecifications.SpotResizeSpecification.TimeoutDurationMinutes = 60 (EMR's default). Smithy: InstanceFleet.ResizeSpecifications -> InstanceFleetResizingSpecifications; SpotResizingSpecification.TimeoutDurationMinutes -> WholeNumber.
  Emulator: None.
  Why: a client reading the resize policy of a fleet it did not configure sees no policy at all.
  Fix: the instance-fleet defaulting in RunJobFlow and the fleet serializer.
  Traces: emr.spark-fleet-cluster-attributes-and-core-resize-default.

Cause 7 of 7: PlacementStrategy NONE is echoed back instead of collapsing the placement-group list.
  AWS: a PlacementGroupConfigs entry whose PlacementStrategy is NONE creates no placement group, so DescribeCluster reports PlacementGroups == []. Smithy: Cluster.PlacementGroups -> PlacementGroupConfigList; PlacementGroupConfig.PlacementStrategy -> PlacementGroupStrategy.
  Emulator: PlacementGroups == [{'InstanceRole': 'MASTER', 'PlacementStrategy': 'NONE'}] — the request is stored verbatim.
  Why: a client that reads back the cluster believes a placement group exists where AWS created none.
  Fix: RunJobFlow's placement-group recording (drop NONE entries) and the DescribeCluster serializer.
  Traces: emr.placement-strategy-none-round-trips-as-empty.

Note: open issue 20261102T083234Z-elasticmapreduce-emr-cluster-and-instance-group-reads-omit-modeled.md covers other omitted cluster/instance-group members and does not carry these check ids.

## Evidence
Replay of simparity-behavioral-aws-v1, '### emr':

1. emr.omitted-scaling-and-config-fields-take-documented-defaults — expected {'ScaleDownBehavior': 'TERMINATE_AT_TASK_COMPLETION', 'StepConcurrencyLevel': 1, 'UnhealthyNodeReplacement': True, 'Configurations': []}, got {'ScaleDownBehavior': '<absent>', 'StepConcurrencyLevel': 1, 'UnhealthyNodeReplacement': False, 'Configurations': '<absent>'}.
2. emr.nested-configuration-without-properties-returns-empty-map — spark-env declared no Properties; real AWS reports an empty map, got '<absent>'.
3. emr.spark-cluster-applications-resolve-release-versions — emr-5.36.1 should resolve to {'Spark': '2.4.8', 'Zeppelin': '0.10.0', 'Hadoop': '2.10.1', 'Pig': '0.17.0'}, got {'Spark': None, 'Zeppelin': None, 'Hadoop': None, 'Pig': None}.
4. emr.fleet-default-ebs-volumes-applied-for-m5-2xlarge — MASTER: an undeclared EBS config should default to 4 x 32 GiB gp2 for m5.2xlarge, got []. emr.fleet-explicit-ebs-config-expands-volumes-per-instance — MASTER declared nothing and should keep the default layout, got ([], '<absent>').
5. emr.spark-fleet-cluster-reports-two-single-unit-fleets — MASTER fleet should hold one m5.2xlarge spec with weight 1 and bid 100.0%, got [('m5.2xlarge', None, None)].
6. emr.spark-fleet-cluster-attributes-and-core-resize-default — the CORE fleet should default to a 60-minute spot resize timeout, got None.
7. emr.placement-strategy-none-round-trips-as-empty — strategy NONE should be stored as PlacementGroups == [], got [{'InstanceRole': 'MASTER', 'PlacementStrategy': 'NONE'}].

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator.
2. RunJobFlow a cluster omitting ScaleDownBehavior, UnhealthyNodeReplacement and Configurations, then DescribeCluster and compare those members (cause 1).
3. RunJobFlow with a Configurations tree containing a child classification that declares no Properties, then DescribeCluster (cause 2).
4. RunJobFlow on emr-5.36.1 with Applications Spark, Zeppelin, Hadoop, Pig and read Cluster.Applications[*].Version (cause 3).
5. RunJobFlow with MASTER and CORE instance fleets of m5.2xlarge — one run declaring no EbsConfiguration, one declaring an explicit one for a single role — then ListInstanceFleets and read InstanceTypeSpecifications[*].EbsBlockDevices, WeightedCapacity and BidPriceAsPercentageOfOnDemandPrice, and InstanceFleet.ResizeSpecifications (causes 4-6).
6. RunJobFlow with PlacementGroupConfigs=[{InstanceRole: MASTER, PlacementStrategy: NONE}], then DescribeCluster and read PlacementGroups (cause 7).
7. Replay the checks in trace_ids to confirm.
