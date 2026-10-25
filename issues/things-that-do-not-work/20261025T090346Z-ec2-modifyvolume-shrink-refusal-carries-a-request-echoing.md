Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.volume-modify-size-and-type-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ModifyVolume|InvalidParameterValue
Issue-Commit: 595f98730159e51618e43b199ea812ccaa6b5afe

# EC2 ModifyVolume shrink refusal carries a request-echoing message instead of AWS's constant template (1 cause)

> _Note: this issue was identified against the code base at commit `595f98730159e51618e43b199ea812ccaa6b5afe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVolume

## Description
One net-new root cause in the EC2 emulator, grounded in the failing behavioral check ec2.volume-modify-size-and-type-lifecycle.

Related open issue: 20261025T030047Z-ec2-rejection-message-templates-diverge-from-aws-on.md records the SAME ModifyVolume shrink-message defect, but it carries only the replay key for ec2.check-modify-volume-size-decrease-rejected (plus a DescribeInstances cause); it does NOT carry the replay key for ec2.volume-modify-size-and-type-lifecycle, so that check is filed here and a fixer should re-replay it as well. See also 20261024T081028Z-ec2-error-semantics-5-rejection-message-templates-diverge.md, which records the same CLASS of defect for DeleteVpc / DeleteSecurityGroup / CreateSubnet / CreatePlacementGroup / DetachVolume. Neither covers this check.

Cause 1 of 1: ModifyVolume refuses a size DECREASE with the wrong message template.
AWS: recorded behavior only — EC2 is an ec2Query service whose errors are not modelled as Smithy shapes (Smithy: ec2 ModifyVolume input VolumeId required (VolumeId), Size Integer, VolumeType enum, Iops/Throughput Integer, MultiAttachEnabled Boolean; errors: [] — so the code and message come from the recording). A ModifyVolume whose Size is lower than the volume's current Size is refused HTTP 400 with error code InvalidParameterValue and the FIXED message "New size cannot be smaller than existing size" — a constant sentence that names no request value. That constant is the requirement; the status and the code are requirements too.
Emulator: the same HTTP 400 and the same code InvalidParameterValue, but the message is a request-derived sentence of the emulator's own design — in this run, with a 2 GiB volume asked to shrink to 1 GiB: "Invalid value '1' for size. The target size must be greater than or equal to the volume's existing size of 2 GiB" (the sizes are one run's values and are examples; the divergence is that the template echoes the request instead of being AWS's constant).
Why: InvalidParameterValue is the shared code for every bad parameter on ModifyVolume, so the message is the only thing that distinguishes "you tried to shrink" from any other parameter refusal. A client, retry policy or diagnostic that keys off AWS's constant shrink text sees an unrecognised message and cannot classify the failure; an exact-match harness fails outright.
Fix: the size-validation error construction in the ModifyVolume handler/validation for the EC2 service (services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_volume.rs) — emit AWS's constant template for the shrink case instead of an interpolated sentence.
Note (not filed as a defect, unverified): the check aborts at this assertion, so its later claims in the same lifecycle — that a rejected shrink leaves the volume's Size untouched, that DescribeVolumesModifications for a never-modified volume answers InvalidVolumeModification.NotFound rather than an empty page, and the subsequent type/size change reaching a terminal modification state — were never evaluated against the emulator.
Traces: ec2.volume-modify-size-and-type-lifecycle (assertion at check_ec2_volume_modify_size_and_type_lifecycle.py line 204).

## Evidence
Check ec2.volume-modify-size-and-type-lifecycle — verdict: mismatch.
AssertionError: expected the shrink refusal message 'New size cannot be smaller than existing size', got "Invalid value '1' for size. The target size must be greater than or equal to the volume's existing size of 2 GiB".
The HTTP status (400) and the error code (InvalidParameterValue) matched; only the message template diverged. The 1 GiB / 2 GiB figures come from one run and are examples — the requirement is the 400, the code InvalidParameterValue, and the CONSTANT message text AWS emits for a shrink.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. CreateVolume(AvailabilityZone=<an available AZ>, Size=2, VolumeType='gp3'); wait until the volume is 'available'.
3. ModifyVolume(VolumeId=<that volume>, Size=1) — i.e. a Size strictly smaller than the current size.
4. Expect HTTP 400, Error.Code = InvalidParameterValue, Error.Message = "New size cannot be smaller than existing size".
5. Observe instead the emulator's request-echoing sentence "Invalid value '<requested>' for size. The target size must be greater than or equal to the volume's existing size of <current> GiB".
