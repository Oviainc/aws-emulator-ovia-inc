Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces-instances.associate-volume-nonexistent-instance-not-found
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: workspaces-instances|CreateTags|InvalidVolume.NotFound
Issue-Commit: 8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f

# A volume CreateVolume mints exists in no EC2/EBS state: ec2:CreateTags on the returned vol- id answers InvalidVolume.NotFound

> _Note: this issue was identified against the code base at commit `8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVolume

## Description
One net-new root cause survived the replay's own ownership markings; every other workspaces-instances failure this run was marked OWNED by an open issue and skipped.

Cause 1 of 1: the volume id a successful WorkSpaces Instances CreateVolume returns is not materialised in the EC2/EBS emulator's volume state, so no EC2 read or write resolves it.

AWS: Smithy (com.amazonaws.workspacesinstances#EUCMIFrontendAPIService): CreateVolume input is EBS's own configuration surface — AvailabilityZone (required), SizeInGB @range 1..16384, VolumeType enum, Iops, Throughput, SnapshotId pattern ^snap-[0-9a-zA-Z]{1,63}$, Encrypted/KmsKeyId, TagSpecifications (max 30) — and its output VolumeId carries pattern ^vol-[0-9a-zA-Z]{1,63}$; AssociateVolume then binds that VolumeId (same pattern) to a WorkspaceInstanceId at a Device, which is an EBS attachment, and models ResourceNotFoundException (HTTP 404). So the volume a successful CreateVolume reports is a real EBS volume in the caller's account and the named Availability Zone: ec2:DescribeVolumes with that VolumeId resolves an `available` volume of the requested size/type/zone, ec2:CreateTags on that id succeeds (HTTP 200, no error), and the check's independent 'effect absent' read at the destination is possible at all.

Emulator: the id lives only in this crate's own store. ec2:CreateTags(Resources=[<VolumeId just returned by CreateVolume>], Tags=[...]) is REFUSED with HTTP 400, error code `InvalidVolume.NotFound`, message "The volume '<VolumeId>' does not exist." — the fixed text is "The volume '" ... "' does not exist." and the only variable part is the volume id echoed from the request (the recorded run shows vol-00000000000000001, which is a per-run value, not part of the requirement). The sibling check that reads the same id back with ec2:DescribeVolumes is refused the same way. Because the refusal lands before the check reaches its subject, the real assertion of this check — that AssociateVolume with a well-formed but never-created WorkspaceInstanceId answers ResourceNotFoundException with message "Workspace instance is not found" (entirely fixed text), and that the volume is left with no attachments — was never exercised, so AssociateVolume's not-found classification remains unverified.

Why: a client that creates a volume through WorkSpaces Instances and then inspects, tags, snapshots, attaches or deletes it with EC2/EBS — the normal way these volumes are managed, since this API exposes no describe/tag of its own — gets a hard 400 InvalidVolume.NotFound for a volume the emulator itself just reported as created. The two services disagree about whether the resource exists, so any cross-service workflow (and any negative test that uses an EC2 read as its independent effect check) breaks.

Fix: CreateVolume's state/propagation path must register the volume in the EC2 emulator's EBS volume state with the requested AvailabilityZone, SizeInGB, VolumeType and tags and state `available`, and DeleteVolume must remove it there — services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/create_volume.rs plus that crate's src/state.rs, against the EC2 crate's volume state. Related open issues name this same propagation gap but do NOT own this check's replay key, so they do not cover it: 20261024T122725Z-workspaces-instances-createvolume-event-propagation.md (static inspection of the gap) and Cause 1 of 20261029T190356Z-workspaces-instances-6-fidelity-divergences-across-createvolume-listregions.md (whose recorded key was ec2:CreateTags answering `InvalidID`; the classification is now `InvalidVolume.NotFound`, so the key no longer matches while the propagation gap persists).

Traces: workspaces-instances.associate-volume-nonexistent-instance-not-found.

## Evidence
Replay report: workspaces-instances 10/25 (40%), 15 failed; 14 of the 15 were marked OWNED by open issues and skipped.

[error] workspaces-instances.associate-volume-nonexistent-instance-not-found
Expected: wsi:CreateVolume(AvailabilityZone=<available zone>, SizeInGB=8, VolumeType="gp3") returns a VolumeId; ec2:CreateTags(Resources=[that VolumeId], Tags=[{Key:"simparity-check",Value:<check>}]) succeeds (HTTP 200), after which wsi:AssociateVolume(WorkspaceInstanceId="wsinst-00000000notreal", VolumeId=<that id>, Device="/dev/sdf") is refused with ResourceNotFoundException (HTTP 404) and message "Workspace instance is not found", and ec2:DescribeVolumes shows the volume with no attachments.
Actual: ec2:CreateTags is refused before the subject is reached — botocore.exceptions.ClientError: An error occurred (InvalidVolume.NotFound) when calling the CreateTags operation: The volume 'vol-00000000000000001' does not exist. (HTTP 400). The AssociateVolume assertions never ran.

## Steps to Reproduce
Boot the multi-service emulator with workspaces-instances and ec2 mounted (and iam, for the service-linked role the check creates first), then:
1. iam:CreateServiceLinkedRole(AWSServiceName="workspaces-instances.amazonaws.com") (tolerate InvalidInput if it already exists).
2. ec2:DescribeAvailabilityZones -> pick the first available zone name AZ.
3. workspaces-instances:CreateVolume(AvailabilityZone=AZ, SizeInGB=8, VolumeType="gp3") -> note the returned VolumeId V.
4. ec2:CreateTags(Resources=[V], Tags=[{Key:"k",Value:"v"}]) -> expect HTTP 200; observe HTTP 400 InvalidVolume.NotFound "The volume 'V' does not exist."
5. ec2:DescribeVolumes(VolumeIds=[V]) -> expect an `available` 8 GiB gp3 volume in AZ; observe the same InvalidVolume.NotFound refusal.
6. With the EC2 reads working, workspaces-instances:AssociateVolume(WorkspaceInstanceId="wsinst-00000000notreal", VolumeId=V, Device="/dev/sdf") must answer ResourceNotFoundException (404) "Workspace instance is not found" and leave V unattached.
