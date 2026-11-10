Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.vscan-unencrypted-gp2-volume-defaults-iops-and-omits-throughput-and-kms-key ec2.vscan-unencrypted-root-mapping-round-trips-onto-the-launched-volume ec2.snapshot-create-from-volume-and-describe ec2.stop-protected-instance-refuses-stop-until-the-attribute-is-cleared ec2.check-create-vpc-block-public-access-exclusion-missing-target-rejected ec2.concurrent-associate-route-server-same-vpc-race ec2.create-fleet-overrides-instance-type-and-requirements-conflict-rejected ec2.create-instance-event-window-contract ec2.network-performance-subscription-invalid-region-rejected ec2.op-contract-delete-key-pair ec2.ec2mr-usg-unused-groups-derive-per-region-from-each-regions-own-vpc
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.vscan-unencrypted-gp2-volume-defaults-iops-and-omits-throughput-and-kms-key|run#11|AssertionError|assert got == (8, "gp2", 100, az, False), (
Issue-Key: ec2.vscan-unencrypted-root-mapping-round-trips-onto-the-launched-volume|run#26|AssertionError|assert got == (8, "gp2", 100, snapshot), (
Issue-Key: ec2.snapshot-create-from-volume-and-describe|run#24|AssertionError|assert reuse_message == "Snapshot does not exist", reuse_message
Issue-Key: ec2.stop-protected-instance-refuses-stop-until-the-attribute-is-cleared|_error#11|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: ec2.check-create-vpc-block-public-access-exclusion-missing-target-rejected|run#14|AssertionError|assert message == "One of the following parameters must be provided: VpcId, SubnetId", f"unexpected MissingParameter message {message!r}"
Issue-Key: ec2.concurrent-associate-route-server-same-vpc-race|run#12|AssertionError|assert results == [("ok", None), ("ok", None)], f"expected both racing AssociateRouteServer calls to succeed, got {results}"
Issue-Key: ec2|CreateFleet|UnsupportedOperation
Issue-Key: ec2.create-instance-event-window-contract|run#15|AssertionError|assert error.get("Code") == "InvalidInstanceEventWindowCronExpression", (
Issue-Key: ec2.network-performance-subscription-invalid-region-rejected|run#8|AssertionError|assert code == "InvalidParameter", f"expected InvalidParameter for Statistic={bogus_statistic!r}, got {code!r}"
Issue-Key: ec2.op-contract-delete-key-pair|run#8|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code}"
Issue-Key: ec2.ec2mr-usg-unused-groups-derive-per-region-from-each-regions-own-vpc|run#6|AssertionError|assert {n["NetworkInterfaceId"] for n in interfaces} == {eni[region]}, (
Issue-Commit: 563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3

# ec2: 10 net-new causes — gp2 Iops never reported, StopInstances ignores DisableApiStop, DescribeNetworkInterfaces crosses regions, and 7 refusal-classification/message divergences

> _Note: this issue was identified against the code base at commit `563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopInstances

## Description
Ten independent defects, each grounded in a failing behavioral check listed under `### ec2` that the replay report marks neither ALREADY FILED nor OWNED. Named-but-not-covering open issues: 20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md (cause 9 is a different operation and carries no replay key there), 20261104T142958Z-ec2-regional-endpoints-share-one-store-cross-region.md (cause 10's check is not listed under it), 20261105T021000Z-ec2-5-net-new-ebs-snapshot-volume-root.md (cause 2's check is not listed under it), and the filed `DeleteKeyPair` stub issue (cause 9 is the DryRun contract on that same operation).

Cause 1 of 10: DescribeVolumes/CreateVolume omit Iops for a gp2 volume.
AWS: `Volume.Iops` (Smithy `CreateVolume` output member `Iops`, xmlName `iops`) is reported for every volume type that has IOPS, and gp2 always has them: AWS derives gp2 IOPS as max(100, 3 x SizeInGiB) and reports that number on CreateVolume, DescribeVolumes and on the root volume a launch creates from an AMI block-device mapping. A gp2 volume never reports Iops absent.
Emulator: the member is omitted — an 8 GiB gp2 volume describes as (8, 'gp2', None, <az>, False) where AWS reports (8, 'gp2', 100, <az>, False), and the root volume created from an AMI snapshot describes as (8, 'gp2', None, '<snapshot-id>') where AWS reports (8, 'gp2', 100, '<snapshot-id>').
Why: anything that reads provisioned IOPS — cost estimation, drift detection, performance assertions, `jq`/JMESPath expressions on Iops — gets a null for the most common volume type.
Fix: the gp2 IOPS derivation in volume state, surfaced by the CreateVolume/DescribeVolumes serializers and by the block-device mapping that RunInstances materialises.
Traces: ec2.vscan-unencrypted-gp2-volume-defaults-iops-and-omits-throughput-and-kms-key, ec2.vscan-unencrypted-root-mapping-round-trips-onto-the-launched-volume.

Cause 2 of 10: CreateVolume from a deleted snapshot reuses the describe-style not-found message.
AWS: CreateVolume naming a SnapshotId that no longer exists is refused `InvalidSnapshot.NotFound` with the message `Snapshot does not exist` — that fixed text, with no id and no trailing period. The id-bearing form `The snapshot '<snap-id>' does not exist.` is DescribeSnapshots' message, not CreateVolume's.
Emulator: CreateVolume answers `InvalidSnapshot.NotFound` (code correct) with `The snapshot '<snap-id>' does not exist.` — the describe message.
Why: a client matching CreateVolume's documented text does not recognise the refusal, and the two refusals become indistinguishable in logs.
Fix: the not-found refusal message in the CreateVolume handler's snapshot resolution.
Traces: ec2.snapshot-create-from-volume-and-describe.

Cause 3 of 10: StopInstances ignores DisableApiStop and stops a stop-protected instance.
AWS: an instance launched with `DisableApiStop=true` (and reading back true from DescribeInstanceAttribute) refuses StopInstances with HTTP 4xx `OperationNotPermitted` and the message `The instance '<instance-id>' may not be stopped. Modify its 'disableApiStop' instance attribute and try again.` (fixed text as quoted; only the instance id comes from the request). The instance stays `running`; after ModifyInstanceAttribute clears the flag, StopInstances succeeds and reports PreviousState `running`. The EC2 Smithy model declares no modeled errors for StopInstances (ec2Query), so the code and message are AWS-documented behavior.
Emulator: StopInstances returns HTTP 200 with StoppingInstances [{CurrentState: stopping, PreviousState: running}] — the protection is not enforced at all.
Why: stop protection is a safety control; an emulator that ignores it validates deployments that would be refused in AWS, and a client testing the guard sees it silently absent.
Fix: the DisableApiStop check in the StopInstances handler (read the instance attribute before transitioning state).
Traces: ec2.stop-protected-instance-refuses-stop-until-the-attribute-is-cleared.

Cause 4 of 10: CreateVpcBlockPublicAccessExclusion's MissingParameter message is emulator-authored.
AWS: Smithy `CreateVpcBlockPublicAccessExclusionRequest` has optional `SubnetId` and `VpcId` of which exactly one must be given; omitting both is refused `MissingParameter` with `One of the following parameters must be provided: VpcId, SubnetId` — that fixed text, with the member names spelled as the API spells them.
Emulator: the code is right but the message is `Either 'subnetId' or 'vpcId' must be specified`, which is not AWS's wording and names the wire-case members.
Why: the message is the only machine-readable statement of which parameters are missing; clients surfacing it to users show text AWS never produces.
Fix: the missing-parameter refusal message in the CreateVpcBlockPublicAccessExclusion handler's validation.
Traces: ec2.check-create-vpc-block-public-access-exclusion-missing-target-rejected.

Cause 5 of 10: a second AssociateRouteServer for an already-associated VPC is refused instead of answering the existing association.
AWS: Smithy `AssociateRouteServer` takes required `RouteServerId` and `VpcId` and returns `RouteServerAssociation`; associating the same route server with the same VPC again is accepted and returns that VPC's existing association, so two concurrent identical calls both succeed.
Emulator: one call succeeds and the other is refused with error code `RouteServerAssociationAlreadyExists`.
Why: any racing or retried association (a retry after a timeout, two workers converging the same VPC) fails, so a client must special-case an error code AWS does not return.
Fix: idempotent handling of an existing (route server, VPC) pair in the AssociateRouteServer handler/state.
Traces: ec2.concurrent-associate-route-server-same-vpc-race.

Cause 6 of 10: CreateFleet classifies the InstanceRequirements/InstanceType conflict as UnsupportedOperation with its own prose.
AWS: an override that carries both `InstanceRequirements` and `InstanceType` is refused `InvalidParameterValue` with `You can specify either InstanceRequirements or InstanceType, but not both. Remove either InstanceRequirements or InstanceType, and try again.` — fixed text, no request-derived parts. (The EC2 Smithy model declares no modeled errors for CreateFleet; the code and message are AWS-documented behavior.)
Emulator: the call is refused `UnsupportedOperation` with `CreateFleet launches the instances a launch template and the instance type, subnet, Availability Zone, key pair and AMI of an override describe; the override member 'instanceRequirements' is not one this service applies` — a different error class (the emulator reports the member as unsupported rather than the combination as invalid) and wholly emulator-authored prose.
Why: UnsupportedOperation reads as "this feature does not exist here", so a client treats a fixable request error as an unsupported API and never corrects the override.
Fix: override validation in the CreateFleet handler.
Traces: ec2.create-fleet-overrides-instance-type-and-requirements-conflict-rejected.

Cause 7 of 10: CreateInstanceEventWindow's cron refusal uses the generic code.
AWS: an `InstanceEventWindowCronExpression` that does not satisfy the event-window rules (here a window shorter than the 4-hour minimum) is refused with the operation-specific code `InvalidInstanceEventWindowCronExpression`, not the generic `InvalidParameterValue`.
Emulator: the same request is refused `InvalidParameterValue`.
Why: clients branch on the specific code to tell a malformed cron from any other bad parameter; collapsing it to InvalidParameterValue makes that impossible.
Fix: the cron-expression validation in the CreateInstanceEventWindow handler (error classification).
Traces: ec2.create-instance-event-window-contract.

Cause 8 of 10: EnableAwsNetworkPerformanceMetricSubscription refuses an out-of-enum Statistic as ValidationError.
AWS: Smithy `StatisticType` has exactly one value, `p50`, so any other Statistic (e.g. `p99`) is a bad parameter value and is refused `InvalidParameter`, and nothing is subscribed.
Emulator: the refusal carries code `ValidationError`.
Why: `ValidationError` is not an EC2 parameter-error code; retry/branching logic keyed on EC2's InvalidParameter family does not match it.
Fix: enum validation and error classification in the EnableAwsNetworkPerformanceMetricSubscription handler.
Traces: ec2.network-performance-subscription-invalid-region-rejected.

Cause 9 of 10: DeleteKeyPair with DryRun=true answers NotImplemented instead of the DryRun contract.
AWS: Smithy `DeleteKeyPairRequest.DryRun` (xmlName `dryRun`) means the request is only checked: an authorized caller gets HTTP 4xx `DryRunOperation` with `Request would have succeeded, but DryRun flag is set.` and the key pair is not deleted.
Emulator: the call answers error code `NotImplemented` — the DryRun pre-check never runs because the operation itself is an unimplemented scaffold. (A separate filed issue covers implementing `DeleteKeyPair` at services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_key_pair.rs; this cause is the DryRun contract that must hold once it is implemented, and 20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md does not carry this check.)
Why: DryRun is how clients and IaC tools probe permissions without mutating; a NotImplemented answer makes a permitted operation look unavailable.
Fix: the DryRun branch of the DeleteKeyPair handler.
Traces: ec2.op-contract-delete-key-pair.

Cause 10 of 10: DescribeNetworkInterfaces answers with interfaces that live in other Regions.
AWS: EC2 state is Region-scoped. A DescribeNetworkInterfaces call to a Region's endpoint whose `vpc-id` filter names VPCs in three Regions returns only the interfaces of that Region's own VPC; the other two VPC ids simply match nothing.
Emulator: the call made against the us-east-1 endpoint returns the interfaces of all three Regions' VPCs (three ENIs where one is owed), so the regional store is shared.
Why: every per-Region derivation built on network interfaces (unused-security-group analysis, inventories, capacity counts) over-counts, and a client cannot trust a Region's answer to describe that Region.
Fix: Region scoping of the network-interface store and of the `vpc-id` filter in the DescribeNetworkInterfaces handler.
Traces: ec2.ec2mr-usg-unused-groups-derive-per-region-from-each-regions-own-vpc.

## Evidence
Cause 1 — ec2.vscan-unencrypted-gp2-volume-defaults-iops-and-omits-throughput-and-kms-key [mismatch]
  AssertionError: the volume must describe as (8, gp2, 100 IOPS, <az>, single-attach), got (8, 'gp2', None, '<az>', False)
       ec2.vscan-unencrypted-root-mapping-round-trips-onto-the-launched-volume [mismatch]
  AssertionError: the root volume must be (8, gp2, 100 IOPS, from '<snap-id>'), got (8, 'gp2', None, '<snap-id>')

Cause 2 — ec2.snapshot-create-from-volume-and-describe [mismatch]
  expected CreateVolume refusal message: "Snapshot does not exist"
  actual:   "The snapshot '<snap-id>' does not exist."

Cause 3 — ec2.stop-protected-instance-refuses-stop-until-the-attribute-is-cleared [mismatch]
  expected: ('OperationNotPermitted', "The instance '<instance-id>' may not be stopped. Modify its 'disableApiStop' instance attribute and try again.") with HTTP 4xx
  actual:   AssertionError: expected a ClientError, but the call succeeded and returned {'StoppingInstances': [{'InstanceId': '<instance-id>', 'CurrentState': {'Code': 64, 'Name': 'stopping'}, 'PreviousState': {'Code': 16, 'Name': 'running'}}], ... 'HTTPStatusCode': 200 ...}

Cause 4 — ec2.check-create-vpc-block-public-access-exclusion-missing-target-rejected [mismatch]
  AssertionError: unexpected MissingParameter message "Either 'subnetId' or 'vpcId' must be specified" (expected "One of the following parameters must be provided: VpcId, SubnetId")

Cause 5 — ec2.concurrent-associate-route-server-same-vpc-race [mismatch]
  AssertionError: expected both racing AssociateRouteServer calls to succeed, got [('ok', None), ('error', 'RouteServerAssociationAlreadyExists')]

Cause 6 — ec2.create-fleet-overrides-instance-type-and-requirements-conflict-rejected [mismatch]
  AssertionError: expected ('InvalidParameterValue', 'You can specify either InstanceRequirements or InstanceType, but not both. Remove either InstanceRequirements or InstanceType, and try again.'), got ('UnsupportedOperation', "CreateFleet launches the instances a launch template and the instance type, subnet, Availability Zone, key pair and AMI of an override describe; the override member 'instanceRequirements' is not one this service applies")

Cause 7 — ec2.create-instance-event-window-contract [mismatch]
  AssertionError: expected InvalidInstanceEventWindowCronExpression for a sub-4-hour window, got 'InvalidParameterValue'

Cause 8 — ec2.network-performance-subscription-invalid-region-rejected [mismatch]
  AssertionError: expected InvalidParameter for Statistic='p99', got 'ValidationError'

Cause 9 — ec2.op-contract-delete-key-pair [mismatch]
  AssertionError: expected DryRunOperation, got NotImplemented

Cause 10 — ec2.ec2mr-usg-unused-groups-derive-per-region-from-each-regions-own-vpc [mismatch]
  AssertionError: the vpc-id filter naming all three VPCs returns interfaces ['eni-...8e', 'eni-...ac', 'eni-...ca'] in us-east-1 (expected only that Region's own ENI)

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; all eleven checks below fail.
2. Cause 1: CreateVolume(Size=8, VolumeType='gp2') and DescribeVolumes; also RunInstances from an AMI with a gp2 root mapping and describe the root volume. Expect Iops=100 in both; observe the member absent.
3. Cause 2: CreateSnapshot, DeleteSnapshot, then CreateVolume(SnapshotId=<deleted>). Expect InvalidSnapshot.NotFound 'Snapshot does not exist'; observe the describe-style message.
4. Cause 3: RunInstances(DisableApiStop=True), wait running, confirm DescribeInstanceAttribute disableApiStop is true, then StopInstances. Expect OperationNotPermitted 4xx and the instance still running; observe HTTP 200 and state stopping.
5. Cause 4: CreateVpcBlockPublicAccessExclusion with neither VpcId nor SubnetId. Compare the MissingParameter message.
6. Cause 5: create a route server and a VPC, then issue two concurrent AssociateRouteServer calls for the same pair. Expect both to succeed.
7. Cause 6: CreateFleet with a LaunchTemplateConfig override carrying both InstanceType and InstanceRequirements. Compare code and message.
8. Cause 7: CreateInstanceEventWindow with a cron expression whose window is under four hours. Expect InvalidInstanceEventWindowCronExpression.
9. Cause 8: EnableAwsNetworkPerformanceMetricSubscription with Statistic='p99'. Expect InvalidParameter.
10. Cause 9: DeleteKeyPair(KeyName=<existing>, DryRun=True). Expect DryRunOperation with 'Request would have succeeded, but DryRun flag is set.'
11. Cause 10: create one VPC with an ENI in each of us-east-1, us-west-1, us-west-2, then call DescribeNetworkInterfaces against us-east-1 with a vpc-id filter naming all three VPC ids. Expect only the us-east-1 ENI.
