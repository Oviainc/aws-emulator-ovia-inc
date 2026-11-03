Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-acceptCapacityReservationBillingOwnership-b ec2.op-contract-list-snapshots-in-recycle-bin ec2.ec2mr-pip-assign-private-ip-outside-the-subnet-cidr-rejected ec2.op-contract-delete-public-ipv4-pool-standalone-rejection ec2.op-contract-deleteTransitGatewayConnect ec2.op-contract-disable-instance-sql-ha-standby-detections ec2.vvd-vpc-delete-with-subnets-rejected ec2.create-store-image-task-unsupported-bucket-region-rejected ec2.op-contract-list-volumes-in-recycle-bin
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-acceptCapacityReservationBillingOwnership-b|run#7|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-list-snapshots-in-recycle-bin|run#13|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code!r}"
Issue-Key: ec2.ec2mr-pip-assign-private-ip-outside-the-subnet-cidr-rejected|run#12|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: ec2.op-contract-delete-public-ipv4-pool-standalone-rejection|run#2|AssertionError|assert code == "InvalidPublicIpv4Pool.NotFound", f"unexpected code for {bogus_pool_id}: {code}"
Issue-Key: ec2.op-contract-deleteTransitGatewayConnect|run#14|AssertionError|assert code == "IncorrectState", f"unexpected error code for in-use Connect: {code}"
Issue-Key: ec2.op-contract-disable-instance-sql-ha-standby-detections|run#5|AssertionError|assert code == "InvalidInstanceID.Malformed", (
Issue-Key: ec2.vvd-vpc-delete-with-subnets-rejected|run#9|AssertionError|assert [v["State"] for v in still] == ["available"], f"after the refusal the VPC reads {still!r}"
Issue-Key: ec2.create-store-image-task-unsupported-bucket-region-rejected|run#10|WaiterError|ec2.get_waiter("snapshot_completed").wait(SnapshotIds=[snap_id], WaiterConfig={"Delay": 2, "MaxAttempts": 300})
Issue-Key: ec2.op-contract-list-volumes-in-recycle-bin|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# EC2: five refusal paths answer NotImplemented, a new VPC never leaves pending, and two destination reads never settle (8 net-new root causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssignPrivateIpAddresses

## Description
Eight net-new root causes in the EC2 emulator, all grounded in behavioral checks that are NOT covered by an already-filed or OWNED entry in the replay report. They share a family with the open issues 20261029T175114Z-ec2-refusal-paths-answer-notimplemented-instead-of-the.md and 20261029T175114Z-ec2-dryrun-contract-authorized-dryrun-requests-answer-notimplemented.md, but none of the checks below carries those issues' replay keys, so they are additional work items. A fixer must close all EIGHT causes before submitting.

Cause 1 of 8 — DryRun is not honoured by AcceptCapacityReservationBillingOwnership or ListSnapshotsInRecycleBin.
AWS: Smithy (ec2, ec2Query): AcceptCapacityReservationBillingOwnership.DryRun and ListSnapshotsInRecycleBin.DryRun are modeled Boolean members; EC2 models no error shapes on these operations (ec2Query), so recorded behavior only. For an authorized request with DryRun=true AWS refuses with HTTP 412, error code `DryRunOperation`, message exactly `Request would have succeeded, but DryRun flag is set.` — that message is entirely fixed text with no variable part.
Emulator: both calls come back with error code `NotImplemented` (AssertionError: expected DryRunOperation, got 'NotImplemented').
Why: a client cannot use DryRun to pre-flight permissions; the standard "probe with DryRun, then act" pattern reads every operation as unavailable instead of authorized, and code that branches on DryRunOperation takes the failure path.
Fix: the DryRun pre-check in each operation's handler (the ec2 ops for accept_capacity_reservation_billing_ownership and list_snapshots_in_recycle_bin), ahead of the request body handling.
Traces: ec2.op-contract-acceptCapacityReservationBillingOwnership-b, ec2.op-contract-list-snapshots-in-recycle-bin.

Cause 2 of 8 — AssignPrivateIpAddresses does not validate the address against the subnet CIDR.
AWS: Smithy: AssignPrivateIpAddresses.PrivateIpAddresses targets PrivateIpAddressStringList, NetworkInterfaceId required; no modeled errors (ec2Query), so recorded behavior only. Assigning an address that does not fall inside the interface's subnet CIDR is refused with error code `InvalidParameterValue` and the message `Address does not fall within the subnet's address range` — all fixed text, no variable part. The interface's address set is unchanged, and a legal address on the same interface still succeeds.
Emulator: the call is refused with error code `NotImplemented` (AssertionError: assigning '10.99.99.99', outside 10.61.1.0/28, must fail InvalidParameterValue ... Got 'NotImplemented').
Why: an emulator that cannot refuse an out-of-range address either stores an address that could never have been allocated or makes the whole operation unusable; either way a client reading back the interface sees an IP inconsistent with the subnet.
Fix: validation in the AssignPrivateIpAddresses handler — resolve the interface's subnet and reject addresses outside its CIDR before mutating interface state.
Traces: ec2.ec2mr-pip-assign-private-ip-outside-the-subnet-cidr-rejected.

Cause 3 of 8 — DeletePublicIpv4Pool does not report an unknown pool.
AWS: Smithy: DeletePublicIpv4Pool.PoolId (Ipv4PoolEc2Id) required; no modeled errors (ec2Query), so recorded behavior only. A pool ID that was never provisioned is refused with error code `InvalidPublicIpv4Pool.NotFound` and the message `The pool ID '<pool-id>' does not exist.` — fixed text around one variable part, the PoolId copied from the request. The ID also never appears in DescribePublicIpv4Pools.
Emulator: error code `NotImplemented` (AssertionError: unexpected code for ipv4pool-ec2-00000000000000000: NotImplemented).
Why: delete-if-exists cleanup paths cannot distinguish "already gone" from "service broken", so teardown either aborts or silently leaves state behind.
Fix: the DeletePublicIpv4Pool handler's existence lookup and its not-found refusal.
Traces: ec2.op-contract-delete-public-ipv4-pool-standalone-rejection.

Cause 4 of 8 — DeleteTransitGatewayConnect does not refuse an attachment that still has Connect peers.
AWS: Smithy: DeleteTransitGatewayConnect.TransitGatewayAttachmentId required; no modeled errors (ec2Query), so recorded behavior only. While the Connect attachment still has a non-deleted Connect peer, the delete is refused with error code `IncorrectState` and the message `Connect <attachment-id> has non-deleted connect peers: <peer-id>.` — fixed text with two variable parts taken from the request and from state (the attachment id and the ids of its live peers). The attachment stays `available`; once the peer is deleted the same call succeeds and the attachment reaches `deleted`.
Emulator: error code `NotImplemented` (AssertionError: unexpected error code for in-use Connect: NotImplemented).
Why: dependency ordering is invisible — a client cannot learn that peers must be torn down first, and cannot tell a refusal from an unimplemented service.
Fix: the DeleteTransitGatewayConnect handler's dependency check against transit-gateway Connect peer state.
Traces: ec2.op-contract-deleteTransitGatewayConnect.

Cause 5 of 8 — DisableInstanceSqlHaStandbyDetections does not validate instance ids.
AWS: Smithy: DisableInstanceSqlHaStandbyDetections.InstanceIds (InstanceIdUpdateStringList, @length 1..30) required, DryRun modeled; no modeled errors (ec2Query), so recorded behavior only. An instance ID that does not resolve is refused with error code `InvalidInstanceID.Malformed` and the message `The instance ID '<instance-id>' is malformed` — fixed text around the id copied from the request — and the id check happens with and without DryRun, i.e. before the DryRun short-circuit.
Emulator: error code `NotImplemented` in both legs (AssertionError: expected InvalidInstanceID.Malformed even under DryRun, got NotImplemented).
Why: callers get no id-level diagnosis and cannot rely on the documented ordering of validation vs DryRun.
Fix: id validation in the DisableInstanceSqlHaStandbyDetections handler, evaluated before the DryRun branch.
Traces: ec2.op-contract-disable-instance-sql-ha-standby-detections.

Cause 6 of 8 — a created VPC never leaves `pending`, so no caller can wait for it.
AWS: Smithy: DeleteVpc.VpcId required; Vpc.State is a VpcState enum. Recorded behavior: a VPC created by CreateVpc transitions `pending` -> `available` within seconds, and DescribeVpcs reports `available` for the rest of its life; a DeleteVpc refused with `DependencyViolation` (message `The vpc '<vpc-id>' has dependencies and cannot be deleted.`) leaves the VPC readable and `available`.
Emulator: after the refused delete, DescribeVpcs still reports `State: 'pending'` for the VPC (AssertionError: after the refusal the VPC reads [{... 'State': 'pending' ...}]).
Why: every client that waits for a VPC to become available — the VpcAvailable waiter, CloudFormation-style provisioning, any poll on State — hangs or times out, and code that gates on `available` never proceeds.
Fix: the VPC state machine in the ec2 service state / the CreateVpc handler: advance a newly created VPC to `available` and keep it there.
Traces: ec2.vvd-vpc-delete-with-subnets-rejected.

Cause 7 of 8 — a snapshot created through the EBS direct APIs is invisible to EC2.
AWS: Smithy (ebs): StartSnapshot/PutSnapshotBlock/CompleteSnapshot mint a real EBS snapshot; recorded behavior: that snapshot id is immediately describable through ec2 DescribeSnapshots and the SnapshotCompleted waiter reaches `completed`.
Emulator: the EC2-side read never sees it — `botocore.exceptions.WaiterError: Waiter SnapshotCompleted failed: An error occurred (InvalidSnapshot.NotFound): The snapshot '<snap-id>' does not exist.` The two services keep separate snapshot stores. This is the same family as the open issue 20261030T201123Z-ec2-describesnapshots-cannot-see-a-snapshot-created-through.md, which owns a different check and does not carry this one's replay key.
Why: any workflow that seeds a snapshot through the direct APIs and then uses it through EC2 (CreateStoreImageTask, RegisterImage, CreateVolume) cannot find its own snapshot.
Fix: shared snapshot state between the ebs and ec2 services — the EBS CompleteSnapshot path must publish into the same snapshot store ec2 DescribeSnapshots reads.
Traces: ec2.create-store-image-task-unsupported-bucket-region-rejected.

Cause 8 of 8 — a Recycle Bin retention rule never becomes enforceable, so the volume recycle-bin flow cannot start.
AWS: recorded behavior only: rbin CreateRule returns a rule whose Status is `PENDING` and which reaches `AVAILABLE` within seconds; only then does deleting a tag-matched volume land it in ListVolumesInRecycleBin.
Emulator: the check never gets past that wait — `Error: Check timed out after 60.0s`, with no EC2 response recorded. Hypothesis: the rbin GetRule status served by the emulator never advances to `AVAILABLE`, so the 60 s poll that precedes the first ListVolumesInRecycleBin call never completes.
Why: the whole Recycle Bin path for EBS volumes is unreachable — a client cannot observe a deleted volume being retained, and any code that waits for a rule to become available stalls.
Fix: the Recycle Bin rule state machine (rbin CreateRule/GetRule) so a new rule reaches AVAILABLE, and the EC2 DeleteVolume path that consults it.
Traces: ec2.op-contract-list-volumes-in-recycle-bin.

## Evidence
Cause 1: expected code DryRunOperation + message "Request would have succeeded, but DryRun flag is set."; got code 'NotImplemented' (both ec2.op-contract-acceptCapacityReservationBillingOwnership-b and ec2.op-contract-list-snapshots-in-recycle-bin).
Cause 2: expected code InvalidParameterValue + message "Address does not fall within the subnet's address range"; got 'NotImplemented' for AssignPrivateIpAddresses with 10.99.99.99 against subnet 10.61.1.0/28.
Cause 3: expected code InvalidPublicIpv4Pool.NotFound + message "The pool ID 'ipv4pool-ec2-00000000000000000' does not exist."; got 'NotImplemented'.
Cause 4: expected code IncorrectState + message "Connect <tgw-attach-id> has non-deleted connect peers: <peer-id>."; got 'NotImplemented'.
Cause 5: expected code InvalidInstanceID.Malformed + message "The instance ID 'i-1234567890abcdef0' is malformed" with and without DryRun; got 'NotImplemented' in both legs.
Cause 6: DescribeVpcs after a DependencyViolation-refused DeleteVpc returned 'State': 'pending' (expected 'available'); full body recorded in the replay output for ec2.vvd-vpc-delete-with-subnets-rejected.
Cause 7: botocore WaiterError — Waiter SnapshotCompleted failed: An error occurred (InvalidSnapshot.NotFound): The snapshot 'snap-<id>' does not exist, for a snapshot minted via ebs.start_snapshot/put_snapshot_block.
Cause 8: Check timed out after 60.0s with no EC2 call recorded; the only preceding wait is rbin.get_rule(Identifier=...)['Status'] == 'available' (timeout 60 s).

## Steps to Reproduce
Replay the behavioral suite checks listed in trace_ids against the emulator (service heading `ec2`).
Cause 1: create a capacity reservation, then call AcceptCapacityReservationBillingOwnership(CapacityReservationId=<it>, DryRun=True); separately call ListSnapshotsInRecycleBin(DryRun=True). Expect DryRunOperation, observe NotImplemented.
Cause 2: create a VPC 10.61.0.0/16 and subnet 10.61.1.0/28, create an ENI in it, then AssignPrivateIpAddresses(NetworkInterfaceId=<eni>, PrivateIpAddresses=['10.99.99.99']).
Cause 3: DeletePublicIpv4Pool(PoolId='ipv4pool-ec2-00000000000000000').
Cause 4: create a TGW, a VPC attachment, a Connect attachment and a Connect peer, then DeleteTransitGatewayConnect on the Connect attachment while the peer exists.
Cause 5: DisableInstanceSqlHaStandbyDetections(InstanceIds=['i-1234567890abcdef0'], DryRun=True) and again without DryRun.
Cause 6: CreateVpc, create two subnets, DeleteVpc (refused DependencyViolation), then DescribeVpcs(VpcIds=[vpc]) and read State.
Cause 7: ebs.start_snapshot + put_snapshot_block + complete_snapshot, then ec2 waiter 'snapshot_completed' / DescribeSnapshots on the returned snapshot id.
Cause 8: rbin.create_rule(ResourceType='EBS_VOLUME', ...) then poll rbin.get_rule(Identifier=<id>)['Status'] for 'available'.
