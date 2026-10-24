Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# CreateVolume accepts volumes EBS refuses: empty AvailabilityZone, no size and no snapshot, and Iops/Throughput/size inconsistent with VolumeType

_Filed: 2026-10-24T01:58:46Z_
_Operation: CreateVolume | Class: validation_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler stores every supplied member verbatim and mints a volume id, with no cross-field validation at all. Configurations the backing EBS volume service rejects with a 400 are accepted with 200:

* `AvailabilityZone` is only length-checked by the boundary (len 0..64), so `""` or `"not-a-zone"` creates a volume. A volume must be created in a real Availability Zone of the service's Region.
* Neither `SizeInGB` nor `SnapshotId` supplied: a volume is created with no size at all. EBS requires a size unless the volume is restored from a snapshot.
* `Iops` supplied for a type that does not support provisioned IOPS (gp2, st1, sc1, standard) and `Throughput` supplied for anything but gp3 are accepted; so are `Iops: 0` / `Throughput: 0` for io1/io2/gp3, which are below those types' minimums, and sizes below the per-type minimum (e.g. `SizeInGB: 1` with `VolumeType: "st1"`/`"sc1"`, whose minimum is 125 GiB).
* `KmsKeyId` with `Encrypted` absent/false is accepted, which EBS refuses as a parameter dependency.

Expected: ValidationException (HTTP 400, modelled on this operation) naming the offending member. Actual: HTTP 200 and a `vol-…` id for every one of these.

## Evidence

Spec: CreateVolume models AvailabilityZone (required, String64 len 0..64), SizeInGB (range 1..16384), SnapshotId, VolumeType (VolumeTypeEnum gp2|gp3|io1|io2|sc1|st1|standard), Iops/Throughput (range 0..) and ValidationException (400) — the per-type rules are not expressible in the model, so they are handler work; they are well-established Amazon EBS CreateVolume semantics (size required unless SnapshotId; Iops only for io1/io2/gp3; Throughput only for gp3; st1/sc1 minimum 125 GiB; encryption-key dependency). Handler: src/ops/create_volume.rs lines ~190-230 copies availability_zone/size_in_gb/volume_type/iops/throughput/encrypted/kms_key_id/snapshot_id straight into the record with no checks. Tests pin the permissive behaviour instead of covering it: tests/all/volume_lifecycle.rs create_rows() includes "an empty AvailabilityZone (len 0..64 admits it)", "the required AvailabilityZone alone" (no size, no snapshot) and "Iops and Throughput at the bottom of their range" (both 0), all asserted to succeed.

## Steps to Reproduce

1. Call CreateVolume with {"AvailabilityZone":""} — emulator answers 200 with a vol- id instead of ValidationException
2. Call CreateVolume with {"AvailabilityZone":"us-east-1a"} only (no SizeInGB, no SnapshotId) — accepted, creating a sizeless volume
3. Call CreateVolume with {"AvailabilityZone":"us-east-1a","VolumeType":"st1","SizeInGB":1,"Iops":500,"Throughput":250} — accepted, although st1 has a 125 GiB minimum and supports neither provisioned IOPS nor configurable throughput
4. Call CreateVolume with {"AvailabilityZone":"us-east-1a","SizeInGB":10,"KmsKeyId":"arn:aws:kms:us-east-1:123456789012:key/abcd"} and no Encrypted — accepted
