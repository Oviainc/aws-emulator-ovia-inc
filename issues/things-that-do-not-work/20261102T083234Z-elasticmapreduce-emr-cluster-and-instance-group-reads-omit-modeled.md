Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-multi-master-reference-create-and-protected-terminate
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-multi-master-reference-create-and-protected-terminate elasticmapreduce-multi-master-topology-release-and-type-mistakes
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Applications[*].Version
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Configurations
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Ec2InstanceAttributes.RequestedEc2AvailabilityZones
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.Ec2InstanceAttributes.RequestedEc2SubnetIds
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.KerberosAttributes
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.ScaleDownBehavior
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.SessionEnabled
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.StepExecutionMode
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.SystemLogUploadDisabled
Issue-Key: elasticmapreduce|DescribeCluster|2xx->2xx|$.Cluster.TerminationProtected
Issue-Key: elasticmapreduce|ListInstanceGroups|2xx->2xx|$.InstanceGroups[Name="Core"].Configurations
Issue-Key: elasticmapreduce|ListInstanceGroups|2xx->2xx|$.InstanceGroups[Name="Core"].ConfigurationsVersion
Issue-Key: elasticmapreduce|ListInstanceGroups|2xx->2xx|$.InstanceGroups[Name="Core"].EbsBlockDevices
Issue-Key: elasticmapreduce|ListInstanceGroups|2xx->2xx|$.InstanceGroups[Name="Core"].ShrinkPolicy
Issue-Key: elasticmapreduce|ListInstanceGroups|2xx->2xx|$.InstanceGroups[Name="Master"].Configurations
Issue-Key: elasticmapreduce|ListInstanceGroups|2xx->2xx|$.InstanceGroups[Name="Master"].ConfigurationsVersion
Issue-Key: elasticmapreduce|ListInstanceGroups|2xx->2xx|$.InstanceGroups[Name="Master"].EbsBlockDevices
Issue-Key: elasticmapreduce|ListInstanceGroups|2xx->2xx|$.InstanceGroups[Name="Master"].ShrinkPolicy
Issue-Key: elasticmapreduce|TerminateJobFlows|4xx->2xx|$
Issue-Key: elasticmapreduce|TerminateJobFlows|4xx->2xx|status
Issue-Key: elasticmapreduce|ListSupportedInstanceTypes|2xx->2xx|$.SupportedInstanceTypes[*]
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EMR cluster and instance-group reads omit modeled members and ignore stored termination protection

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
The emulator's EMR read path serves partially-populated resources: successful 200 responses leave modeled Cluster/InstanceGroup members unset, the cluster's TerminationProtected flag is not persisted from the request, and the instance-type catalog served for a release label is not the one AWS serves. The missing TerminationProtected state then lets TerminateJobFlows destroy a protected cluster that AWS refuses to shut down.

THIS ISSUE BUNDLES 5 CAUSES. All five must be closed.

Cause 1 of 5: DescribeCluster omits modeled Cluster members and loses TerminationProtected.
  AWS: 200 with the Cluster structure fully populated for every member the service defaults. Smithy: Cluster members Configurations (ConfigurationList) and Ec2InstanceAttributes.RequestedEc2AvailabilityZones / RequestedEc2SubnetIds are serialized as empty lists when nothing was requested; KerberosAttributes is serialized as an empty structure; ScaleDownBehavior (enum ScaleDownBehavior) is defaulted by the service and returned; SessionEnabled and SystemLogUploadDisabled are returned as false when not set; Applications[].Version carries the application version resolved for the cluster's ReleaseLabel; TerminationProtected echoes the value the cluster was created with (Smithy doc: "Indicates whether Amazon EMR will lock the cluster to prevent the Amazon EC2 instances from being terminated by an API call"). StepExecutionMode is recorded behavior only (not in the on-disk Cluster shape) and is returned as 'STANDARD'.
  Emulator: 200 but Applications[0].Version, Configurations, Ec2InstanceAttributes.RequestedEc2AvailabilityZones, Ec2InstanceAttributes.RequestedEc2SubnetIds, KerberosAttributes, ScaleDownBehavior, SessionEnabled, StepExecutionMode and SystemLogUploadDisabled are all absent, and TerminationProtected is false where the cluster was created with it true.
  Why: a client iterating Configurations or reading ScaleDownBehavior/Applications[].Version hits a null where AWS guarantees a value, and any client gating a destructive action on TerminationProtected reads it as unprotected.
  Fix: DescribeCluster's serializer (default-member population) plus RunJobFlow/cluster state, which must persist TerminationProtected and the resolved application versions.
  Traces: elasticmapreduce-multi-master-reference-create-and-protected-terminate#task#9.

Cause 2 of 5: ListInstanceGroups omits modeled InstanceGroup members.
  AWS: 200 with each InstanceGroup carrying Configurations as an empty list when none were supplied, ConfigurationsVersion (Long) as 0, ShrinkPolicy as an empty structure, and EbsBlockDevices expanded to one entry per EBS volume requested for the group (each with VolumeSpecification.SizeInGB and VolumeType taken from the group's EbsConfiguration). Smithy: InstanceGroup{Configurations, ConfigurationsVersion, EbsBlockDevices, ShrinkPolicy}.
  Emulator: 200 but Configurations, ConfigurationsVersion, EbsBlockDevices and ShrinkPolicy are absent for every group (observed on both the Master and Core groups).
  Why: a client reading group EBS layout or configuration version sees nothing and cannot reconcile what it asked for; code that dereferences ConfigurationsVersion or ShrinkPolicy faults on null.
  Fix: ListInstanceGroups' serializer and the instance-group state written by RunJobFlow (EBS block devices derived from the requested EbsConfiguration).
  Traces: elasticmapreduce-multi-master-reference-create-and-protected-terminate#task#10.

Cause 3 of 5: TerminateJobFlows shuts down a termination-protected cluster.
  AWS: 400 with `__type` = `ValidationException` and `message` = "Could not shut down one or more job flows since they are termination protected." — this message is entirely fixed text, with no value interpolated from the request. Smithy: TerminateJobFlows.JobFlowIds required; the modelled error list carries only InternalServerError, so the refusal envelope is recorded behavior only.
  Emulator: 200 with an empty body — the protected cluster is terminated.
  Why: the protection feature is inert. A client that sets TerminationProtected to guard a running cluster, then issues a terminate (its own or a sweeper's), loses the cluster instead of receiving the refusal, and never sees the error it is written to expect.
  Fix: the TerminateJobFlows handler must read each target cluster's TerminationProtected from state and refuse before mutating; depends on cause 1's state fix.
  Traces: elasticmapreduce-multi-master-reference-create-and-protected-terminate#task#11.

Cause 4 of 5: DescribeCluster after a terminate still omits the same defaulted members.
  AWS: 200 with Applications[].Version, Configurations (empty list), Ec2InstanceAttributes.RequestedEc2AvailabilityZones / RequestedEc2SubnetIds (empty lists), KerberosAttributes (empty structure), ScaleDownBehavior, SessionEnabled (false), StepExecutionMode and SystemLogUploadDisabled (false) all present, independent of cluster lifecycle state.
  Emulator: 200 with all nine of those members absent.
  Why: the omission is not confined to the pre-terminate read, so no client can rely on these members at any point in the cluster's life.
  Fix: same serializer/state component as cause 1; this cluster confirms the gap is lifecycle-independent and must be re-replayed separately.
  Traces: elasticmapreduce-multi-master-reference-create-and-protected-terminate#task#14.

Cause 5 of 5: ListSupportedInstanceTypes serves the wrong catalog for the requested release label.
  AWS: 200 with SupportedInstanceTypes listing the instance types the given ReleaseLabel actually supports, each SupportedInstanceType carrying Type, MemoryGB, StorageGB, VCPU, Is64BitsOnly, InstanceFamilyId, EbsOptimizedAvailable, EbsOptimizedByDefault, NumberOfDisks, EbsStorageOnly and Architecture consistent with that EC2 instance type (Smithy: SupportedInstanceType). For the release label under test the first entry is an AARCH64 Graviton general-current-gen type.
  Emulator: 200 but the first entry is a hard-coded X86_64 m5-class entry regardless of the requested release label, so the whole list is the wrong catalog rather than one wrong field.
  Why: a client selecting an instance type for a release label gets types that release does not support, and architecture-aware selection (AARCH64 vs X86_64) picks wrong.
  Fix: ListSupportedInstanceTypes' handler/state — the per-release-label instance-type catalog it is backed by.
  Traces: elasticmapreduce-multi-master-topology-release-and-type-mistakes#task#8.

Related but NOT covering: 20261029T212242Z-elasticmapreduce-emr-studio-notebook-and-catalog-reads-accept-refused.md and 20261031T093431Z-elasticmapreduce-emr-setterminationprotection-appends-the-job-flow-id-to.md; neither owns these five replay keys (the latter concerns SetTerminationProtection's message text, not TerminateJobFlows ignoring the flag).

## Evidence
Replay of trace set simparity-wire-aws-v1, verdict=mismatch for each cluster:

1. DescribeCluster elasticmapreduce-multi-master-reference-create-and-protected-terminate#task#9, 200 vs 200 — $.Cluster.Applications[0].Version missing (expected '3.3.6'); $.Cluster.Configurations missing (expected []); $.Cluster.Ec2InstanceAttributes.RequestedEc2AvailabilityZones missing (expected []); $.Cluster.Ec2InstanceAttributes.RequestedEc2SubnetIds missing (expected []); $.Cluster.KerberosAttributes missing (expected {}); $.Cluster.ScaleDownBehavior missing (expected 'TERMINATE_AT_TASK_COMPLETION'); $.Cluster.SessionEnabled missing (expected False); $.Cluster.StepExecutionMode missing (expected 'STANDARD'); $.Cluster.SystemLogUploadDisabled missing (expected False); $.Cluster.TerminationProtected expected True, got False.
2. ListInstanceGroups ...#task#10, 200 vs 200 — for both InstanceGroups[Name="Master"] and [Name="Core"]: Configurations missing (expected []), ConfigurationsVersion missing (expected 0), ShrinkPolicy missing (expected {}), EbsBlockDevices missing (expected two entries, each VolumeSpecification {SizeInGB 32, VolumeType gp2}).
3. TerminateJobFlows ...#task#11 — status expected 400, got 200; body expected {'__type': 'ValidationException', 'message': 'Could not shut down one or more job flows since they are termination protected.'}, got {}.
4. DescribeCluster ...#task#14, 200 vs 200 — the same nine members missing (Applications[0].Version, Configurations, RequestedEc2AvailabilityZones, RequestedEc2SubnetIds, KerberosAttributes, ScaleDownBehavior, SessionEnabled, StepExecutionMode, SystemLogUploadDisabled).
5. ListSupportedInstanceTypes elasticmapreduce-multi-master-topology-release-and-type-mistakes#task#8, 200 vs 200 — $.SupportedInstanceTypes[0] expected {'Architecture': 'AARCH64', 'EbsOptimizedAvailable': True, 'EbsOptimizedByDefault': True, 'EbsStorageOnly': True, 'InstanceFamilyId': 'GENERAL_CURRENT_GEN', 'Is64BitsOnly': True, 'MemoryGB': 15.25, 'NumberOfDisks': 0, 'StorageGB': 0, 'Type': 'm7g.xlarge', 'VCPU': 4}, got an m5.large/X86_64 entry.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Representative: fixture elasticmapreduce-multi-master-reference-create-and-protected-terminate, step task#9 — RunJobFlow a multi-master cluster with TerminationProtected true, then DescribeCluster and compare the Cluster structure's members.
4. In the same fixture, step task#10 (ListInstanceGroups), step task#11 (TerminateJobFlows on the protected cluster — expect the 400 refusal) and step task#14 (DescribeCluster again).
5. Fixture elasticmapreduce-multi-master-topology-release-and-type-mistakes, step task#8 — ListSupportedInstanceTypes for the fixture's release label and compare SupportedInstanceTypes[0].
