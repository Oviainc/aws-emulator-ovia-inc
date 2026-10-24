Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: f0788135e0bb0704427b37eee4190850f5fa33c6

# A volume created for WorkSpace Instances is invisible to EC2/EBS: the vol- id this service mints exists in no EBS volume state

_Filed: 2026-10-24T12:27:25Z_
_Operation: CreateVolume | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `f0788135e0bb0704427b37eee4190850f5fa33c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateVolume mints a `vol-<counter>` id in this crate's own store and returns it, but creates no Amazon EBS volume in the EC2 emulator's volume state, and DeleteVolume removes only the local record.

Expected: the volume this operation creates is an Amazon EBS volume in the caller's account and in the Availability Zone the request names — the handler itself applies Amazon EBS's documented `CreateVolume` rules (capacity source, per-volume-type Size/Iops/Throughput ranges, `KmsKeyId` requires `Encrypted=true`) and the model's configuration surface is EBS's own (`VolumeType` gp2|gp3|io1|io2|sc1|st1|standard, `Iops`, `Throughput`, `SnapshotId` ^snap-…, `Encrypted`, `KmsKeyId`, `TagSpecifications` whose `ResourceTypeEnum` value is `volume`). So after a successful CreateVolume, `ec2:DescribeVolumes` with the reported `VolumeId` resolves an `available` volume of that size/type/zone carrying the requested tags, and DeleteVolume makes it disappear from there.

Actual: `ec2:DescribeVolumes(VolumeIds=[<the reported vol-… id>])` answers the EBS not-found error, because the record lives only in this crate's `state.volumes`. The volume is also never attached to anything in EC2, so no cross-service workflow (create volume here, inspect/snapshot it with EC2) can work.

## Evidence

Smithy model: `CreateVolumeResponse$VolumeId` pattern `^vol-[0-9a-zA-Z]{1,63}$` — EBS's own id form; `CreateVolumeRequest` carries exactly EBS's volume configuration (AvailabilityZone, SizeInGB 1..16384, VolumeType enum gp2..standard, Iops, Throughput, SnapshotId ^snap-…, Encrypted, KmsKeyId, TagSpecifications with ResourceTypeEnum `volume`), and `AssociateVolume`/`DisassociateVolume` attach it to a WorkSpace Instance at a `Device`, which is an EBS attachment.
Handler: services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/create_volume.rs — `state.mint_volume_id()` (src/state.rs: `format!("vol-{:017x}")`) and `state.volumes.insert(...)`; the file's own doc calls these "the configuration rules the backing Amazon EBS volume service applies" and state.rs calls the resource "the Amazon EBS volume a WorkSpace Instance carries" (VOLUME_RESOURCE_TYPE = "EBS_VOLUME"), yet nothing is written to EBS state. delete_volume.rs only does `state.volumes.remove(&input.volume_id)`.
Feasibility/observability: the EC2 emulator holds EBS volume state (services/service-com-amazonaws-ec2-amazonec2/src/state.rs `pub volumes: BTreeMap<String, VolumeRecord>`) and `DescribeVolumes` is implemented there (src/ops/describe_volumes.rs: "IMPLEMENTED: reports the EBS volumes crate::state holds").
Tests: tests/all/volume_lifecycle.rs and tests/all/volume_configuration_validation.rs exercise only this service's own store (create -> delete -> not-found, quota, idempotency); no test makes an EC2 call, so nothing asserts the cross-service visibility.

## Steps to Reproduce

1. Boot the multi-service emulator so both `workspaces-instances` and `ec2` are mounted
2. Call CreateVolume with {"AvailabilityZone":"us-east-1a","SizeInGB":100,"VolumeType":"gp3","TagSpecifications":[{"ResourceType":"volume","Tags":[{"Key":"env","Value":"dev"}]}]} signed for us-east-1 and keep the returned VolumeId
3. Call ec2:DescribeVolumes with VolumeIds=[that vol-… value] — the emulator answers the EBS not-found error; expected is one 100 GiB gp3 volume in us-east-1a carrying the env=dev tag
4. Call DeleteVolume for the id, then ec2:DescribeVolumes again — no EBS volume ever existed, so the delete propagates nothing
