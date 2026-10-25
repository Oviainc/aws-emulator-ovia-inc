Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.modify-volume-iops-below-minimum-accepted
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ModifyVolume|InvalidParameterValue
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# EC2 ModifyVolume refuses a below-minimum gp3 Iops that real AWS accepts and clamps to the gp3 floor

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVolume

## Description
REOPENS 20261022T235108Z-ec2-modifyvolume-stub.md: its fix left the call refused.

Cause 1 of 1: ModifyVolume rejects an Iops value below the gp3 minimum instead of accepting the request and clamping the applied value up to the gp3 floor.

AWS: Smithy (com.amazonaws.ec2#ModifyVolume): the operation models NO errors at all, and ModifyVolumeRequest.Iops is a plain Integer member with no @range constraint. Its documentation lists the supported values per volume type ("gp3: 3,000 - 80,000 IOPS") but states no rejection and models no error for a value outside that list. Recorded behavior: ModifyVolume on an available gp3 volume with Iops below the floor is ACCEPTED synchronously (HTTP 200) and returns a VolumeModification; the modification then reaches a terminal completed/optimizing state whose TargetIops is NOT the requested value and is at or above the gp3 floor of 3000, and DescribeVolumes likewise never reports the out-of-range value as the volume's Iops. In other words the clamp, not a refusal, is the contract, and the below-minimum value must never be the one actually applied.

Emulator: the call is refused with HTTP 400 InvalidParameterValue (observed message, an example only: "Invalid value '100' for iops. A gp3 volume provisions from 3000 to 80000 IOPS"). No VolumeModification is created, so neither the clamp nor the terminal modification state can be observed.

Why: a client relying on AWS's behavior submits a stale or template-derived Iops below the floor and expects AWS to normalize it; against the emulator the same call is a hard client error. Infrastructure code that modifies a gp3 volume's size/throughput while passing along an unchanged, below-floor Iops therefore fails against the emulator and succeeds in AWS, and the modification-tracking path (DescribeVolumesModifications) is never exercised because nothing is recorded.

Fix: the ModifyVolume handler's Iops validation (it must not reject an out-of-range gp3 Iops) and the volume-modification state it writes (it must record a modification whose TargetIops is clamped to the gp3 floor, and leave the volume's reported Iops at or above that floor).

Traces: ec2.modify-volume-iops-below-minimum-accepted

## Evidence
Check ec2.modify-volume-iops-below-minimum-accepted, verdict error.

Expected (recorded AWS): ec2.modify_volume(VolumeId=<gp3 10GiB vol>, Iops=100) -> HTTP 200 with VolumeModification.ModificationState in (modifying|optimizing|completed|failed); DescribeVolumesModifications then reports ModificationState in (completed|optimizing) with TargetIops != 100 and TargetIops >= 3000; DescribeVolumes reports Iops != 100 and >= 3000.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the ModifyVolume operation: Invalid value '100' for iops. A gp3 volume provisions from 3000 to 80000 IOPS

Status/code divergence: expected HTTP 200 (no error), got HTTP 400 InvalidParameterValue. The quoted message text is an example, not the requirement.

## Steps to Reproduce
1. ec2.create_volume(AvailabilityZone=<first available AZ>, Size=10, VolumeType="gp3")
2. Poll ec2.describe_volumes until State == "available".
3. ec2.modify_volume(VolumeId=<volume>, Iops=100).
4. Observe InvalidParameterValue instead of a 200 with a VolumeModification.
5. In AWS the call returns 200 and ec2.describe_volumes_modifications(VolumeIds=[volume]) reaches completed/optimizing with TargetIops >= 3000.
