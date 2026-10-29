Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rbin.ebs-snapshot-retained-in-recycle-bin-on-delete
Issue-Shape-Id: com.amazonaws.rbin#AmazonRecycleBin
Issue-Key: rbin|CreateSnapshot|NotImplemented
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# rbin retention rules do not retain deleted EBS snapshots: the EBS_SNAPSHOT end-to-end retain/restore path is unserved

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
Cause 1 of 1: a tag-level rbin retention rule for ResourceType=EBS_SNAPSHOT has no effect on EBS snapshot deletion, and the emulator cannot even reach that point because the EBS snapshot the workflow needs cannot be created. Failing check: rbin.ebs-snapshot-retained-in-recycle-bin-on-delete.

AWS: CreateRule (Smithy doc, com.amazonaws.rbin#CreateRule) documents tag-level retention rules: "For each retention rule, you specify one or more tag key and value pairs. Resources (of the specified type) that have at least one of these tag key and value pairs are automatically retained in the Recycle Bin upon deletion." With such a rule in place for EBS_SNAPSHOT, deleting a matching snapshot through EC2 does not permanently delete it: the snapshot becomes resident in the Recycle Bin (EC2 ListSnapshotsInRecycleBin returns it, carrying RecycleBinEnterTime), it is no longer a live snapshot (DescribeSnapshots on that id is refused with code InvalidSnapshot.NotFound and the message "The snapshot '<snapshot-id>' does not exist." — the fixed text is everything but <snapshot-id>, which is the id taken verbatim from the request), and RestoreSnapshotFromRecycleBin returns it to a normal snapshot that reaches State=completed. Creating the snapshot in the first place (EC2 CreateSnapshot) is a modelled, 200-returning operation that mints a SnapshotId.

Emulator: the workflow is refused at its first snapshot step. EC2 CreateSnapshot answers HTTP 501 with code NotImplemented and the message "operation CreateSnapshot is scaffolded but not implemented in this emulator", so no snapshot exists and nothing downstream (the rule's interception of DeleteSnapshot, ListSnapshotsInRecycleBin, RestoreSnapshotFromRecycleBin) is exercised. Behind that refusal the rbin side is also inert: a created rule is recorded only in the rbin crate's own state and is consulted by no other service on deletion, so even with CreateSnapshot implemented a matching DeleteSnapshot would permanently delete the snapshot and the Recycle Bin would stay empty.

Why: a client that relies on Recycle Bin for EBS data protection gets the opposite of the guarantee it configured. A delete that AWS would have intercepted and made recoverable is, against the emulator, an unrecoverable permanent delete; rollback/restore paths built on ListSnapshotsInRecycleBin and RestoreSnapshotFromRecycleBin see an empty bin and read it as "nothing was retained", so tests of a retention policy pass vacuously and no retention bug is ever caught.

Fix: two components. (1) EC2's CreateSnapshot handler must be implemented so a snapshot can exist at all — the open issue 20261022T235108Z-ec2-createsnapshot-stub.md covers that stub, but its Issue-Trace-Ids do not include this check, so closing it alone does not unblock this check. (2) The rbin rule store needs an exported, cross-service lookup ("which rules match resource type T and these resource tags") that EC2's DeleteSnapshot handler consults, plus snapshot recycle-bin state in the EC2 crate feeding ListSnapshotsInRecycleBin / RestoreSnapshotFromRecycleBin and the InvalidSnapshot.NotFound refusal from DescribeSnapshots while resident. The related open issue 20261024T155459Z-rbin-createrule-event-propagation.md describes the inert-rule condition for EBS_VOLUME from a static read; it does not carry this check's replay key and does not cover the EBS_SNAPSHOT retain/restore path asserted here.

Traces: rbin.ebs-snapshot-retained-in-recycle-bin-on-delete.

## Evidence
Check rbin.ebs-snapshot-retained-in-recycle-bin-on-delete, verdict [error].

Expected (AWS): ec2.create_snapshot(VolumeId=..., TagSpecifications=[{snapshot, [{Key: <tag-key>, Value: <tag-value>}]}]) -> 200 with a SnapshotId; rbin.create_rule(ResourceType="EBS_SNAPSHOT", RetentionPeriod={1, DAYS}, ResourceTags=[{ResourceTagKey: <tag-key>, ResourceTagValue: <tag-value>}]) -> rule reaching Status=available; ec2.delete_snapshot(SnapshotId=<id>) -> the snapshot is retained, so ec2.list_snapshots_in_recycle_bin(SnapshotIds=[<id>]) returns exactly one entry for <id> including RecycleBinEnterTime, ec2.describe_snapshots(SnapshotIds=[<id>]) is refused with Code=InvalidSnapshot.NotFound and Message="The snapshot '<id>' does not exist.", and ec2.restore_snapshot_from_recycle_bin(SnapshotId=<id>) brings it back to State=completed.

Actual (emulator): the check aborts on the CreateSnapshot call with botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSnapshot operation: operation CreateSnapshot is scaffolded but not implemented in this emulator (HTTP 501). No snapshot is created, so the rule's retention of a deleted snapshot, the Recycle Bin listing, the InvalidSnapshot.NotFound refusal and the restore are all unserved. Diverging status/code/message: 501 NotImplemented "operation CreateSnapshot is scaffolded but not implemented in this emulator" where AWS returns a 200 CreateSnapshot result; the message is wholly emulator-authored (AWS has no such text) and only the operation name in it comes from the request.

## Steps to Reproduce
1. Boot the emulator and point ec2 + rbin clients at it.
2. ec2.describe_availability_zones() -> take the first ZoneName; ec2.create_volume(AvailabilityZone=<az>, Size=1, VolumeType="gp3") and wait for volume_available.
3. ec2.create_snapshot(VolumeId=<vol>, TagSpecifications=[{"ResourceType": "snapshot", "Tags": [{"Key": "sim2real-rbin-test", "Value": "<unique>"}]}]) -> expected 200 with a SnapshotId; actual HTTP 501 NotImplemented "operation CreateSnapshot is scaffolded but not implemented in this emulator".
4. With CreateSnapshot stubbed out of the way, the rest of the path still diverges: rbin.create_rule(ResourceType="EBS_SNAPSHOT", RetentionPeriod={"RetentionPeriodValue": 1, "RetentionPeriodUnit": "DAYS"}, ResourceTags=[{"ResourceTagKey": "sim2real-rbin-test", "ResourceTagValue": "<unique>"}]), poll GetRule until Status=available, then ec2.delete_snapshot(SnapshotId=<snap>).
5. ec2.list_snapshots_in_recycle_bin(SnapshotIds=[<snap>]) -> expected one entry with RecycleBinEnterTime; ec2.describe_snapshots(SnapshotIds=[<snap>]) -> expected InvalidSnapshot.NotFound "The snapshot '<snap>' does not exist."; ec2.restore_snapshot_from_recycle_bin(SnapshotId=<snap>) -> expected the snapshot back at State=completed. The emulator never populates any snapshot recycle-bin state, because no handler outside rbin reads the rule store.
