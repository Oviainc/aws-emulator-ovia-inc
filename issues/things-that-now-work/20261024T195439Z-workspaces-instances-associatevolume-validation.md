Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 422492f47e5217da15117465faf05f5c44507550

# Refuse AssociateVolume when the volume and the WorkSpace Instance are in different Availability Zones

_Filed: 2026-10-24T19:54:39Z_
_Operation: AssociateVolume | Class: validation_

> _Note: this issue was identified against the code base at commit `422492f47e5217da15117465faf05f5c44507550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler resolves both ids, checks instance teardown, double attachment and device-path occupancy, then stores the attachment — but it never compares the volume's Availability Zone with the Availability Zone the WorkSpace Instance was placed in. A volume created in `us-east-1a` can be attached to an instance whose `ManagedInstance.Placement.AvailabilityZone` is `us-east-1b`, answering HTTP 200 and persisting an attachment that could not exist in the real service.

Expected: an attach across Availability Zones is refused (the backing Amazon EBS attach fails with a zone mismatch, surfaced here through the operation's modelled `ValidationException`/`ConflictException`) and no attachment is stored — observable through `DeleteVolume`, which then succeeds instead of answering the in-use conflict. Actual: 200 and a stored cross-zone attachment, which also makes the volume undeletable until a `DisassociateVolume` call.

Both facts are in the store already: `VolumeRecord.availability_zone` (validated against the Region's zones at create time) and `WorkspaceInstanceRecord.managed_instance.placement.availability_zone`, so the check needs no new state.

## Evidence

Well-established EBS semantics: a volume can only be attached to an instance in the SAME Availability Zone (Amazon EBS AttachVolume — "The volume and the instance must be in the same Availability Zone"); this service's AssociateVolume is documented as attaching the managed EBS volume to the WorkSpace Instance and declares both ValidationException (400) and ConflictException (409). Handler: src/ops/associate_volume.rs — checks are empty `Device`, instance lookup, `is_tearing_down`, `volume_attached_at` occupancy and `volume.attachment.is_some()`; nothing reads `volume.availability_zone` or the instance's placement. State: src/state.rs `VolumeRecord.availability_zone` and `WorkspaceInstanceRecord.managed_instance`. Tests: tests/all/volume_attachment.rs covers not-found, double attach, device occupancy, teardown release and Region scoping — no zone-mismatch case (grep for `availability` in that suite returns nothing).

## Steps to Reproduce

1. CreateWorkspaceInstance signed for us-east-1 with {"ManagedInstance": {"InstanceType": "t3.medium", "Placement": {"AvailabilityZone": "us-east-1b"}}}
2. CreateVolume signed for us-east-1 with {"AvailabilityZone": "us-east-1a", "SizeInGB": 8}
3. AssociateVolume with that WorkspaceInstanceId, that VolumeId and Device "/dev/sdf" — observe HTTP 200 instead of a refusal
4. DeleteVolume for that VolumeId — observe the in-use ConflictException, proving the cross-zone attachment was persisted
