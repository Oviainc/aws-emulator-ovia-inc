Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.check-modify-volume-size-decrease-rejected ec2__ssm.vscan-instance-information-filter-rejects-a-malformed-instance-id
Issue-Key: ec2.check-modify-volume-size-decrease-rejected|run#17|AssertionError|assert code_msg == "New size cannot be smaller than existing size", code_msg
Issue-Key: ec2__ssm.vscan-instance-information-filter-rejects-a-malformed-instance-id|run#14|AssertionError|assert msg == 'Invalid id: "not-an-instance-id"', f"expected the exact EC2 message, got {msg!r}"
Issue-Commit: ef659389d11311dc27a7ee58cb593d22d3f663cf

# EC2 rejection-message templates diverge from AWS on ModifyVolume (shrink) and DescribeInstances (malformed instance id)

> _Note: this issue was identified against the code base at commit `ef659389d11311dc27a7ee58cb593d22d3f663cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVolume

## Description
Two net-new root causes in the EC2 emulator, both grounded in failing behavioral checks. In both the HTTP status and the error CODE are correct; the MESSAGE the emulator builds does not follow AWS's template, so a client that reads the refusal text (or an exact-match harness) diverges.

Related open issue: 20261024T081028Z-ec2-error-semantics-5-rejection-message-templates-diverge.md records the same class of defect for DeleteVpc, DeleteSecurityGroup, CreateSubnet, CreatePlacementGroup and DetachVolume. It does NOT carry either replay key below, so these two causes are filed here; a fixer should expect the same message-construction code paths and may want to fix them together.

Cause 1 of 2: ModifyVolume refuses a shrink with the wrong message template.
AWS: recorded behavior only - EC2 is an ec2Query service and its errors are not modelled as Smithy shapes (Smithy: ec2 ModifyVolume input Size is an Integer, VolumeId required, errors: [] - no modelled error shapes, so the code/message come from the recording). ModifyVolume that lowers an existing volume's Size is refused HTTP 400 with error code InvalidParameterValue and the fixed message "New size cannot be smaller than existing size" - a constant string that names no request value.
Emulator: same 400 / InvalidParameterValue, but the message is a request-derived sentence of its own design, e.g. "Invalid value '5' for size. The target size must be greater than or equal to the volume's existing size of 10 GiB".
Why: a client or diagnostic that keys off AWS's shrink-refusal text sees an unrecognised message, so a shrink attempt cannot be told apart from any other InvalidParameterValue on ModifyVolume (the code is shared by every bad parameter on this operation).
Fix: the ModifyVolume size-validation error construction in the operation's handler/validation for the EC2 service (services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_volume.rs) - emit AWS's constant template for the shrink case.
Note (not filed as a defect, unverified): the check aborts at this assertion, so its later claims - that a REJECTED shrink leaves no modification record (DescribeVolumesModifications answering InvalidVolumeModification.NotFound) and that the volume's Size stays 10 - were never evaluated against the emulator. The positive control (a legal 10->12 grow accepted, and reported by DescribeVolumesModifications) did pass.
Traces: ec2.check-modify-volume-size-decrease-rejected.

Cause 2 of 2: DescribeInstances quotes a malformed instance id with single quotes instead of double quotes.
AWS: recorded behavior only (Smithy: ec2 DescribeInstances errors: [] - EC2 ec2Query errors are not modelled). DescribeInstances naming an id that is not an instance id at all is refused HTTP 400 with error code InvalidInstanceID.Malformed and a message BUILT FROM THE REQUEST: the literal prefix 'Invalid id: ' followed by the offending value in DOUBLE quotes - Invalid id: "not-an-instance-id".
Emulator: the same 400 / InvalidInstanceID.Malformed, but the value is wrapped in SINGLE quotes - Invalid id: 'not-an-instance-id'. The rule (prefix + echoed request value) is implemented; only the quoting character of the template is wrong.
Why: the message is the only place the refusal names WHICH id was malformed, and callers that parse it with AWS's template (or assert on it) fail to extract the id / fail the comparison, even though the code is right.
Fix: the malformed-instance-id validation error construction shared by the EC2 describe paths (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instances.rs and the id-validation helper it calls) - quote the echoed value exactly as AWS does.
Note: in this check the SSM half (DescribeInstanceInformation: empty list for a well-formed unknown id, InvalidInstanceId for a malformed filter value with the regex-constraint message, ValidationException for an unknown filter key) all passed; the EC2 refusal is the only divergence.
Traces: ec2__ssm.vscan-instance-information-filter-rejects-a-malformed-instance-id.

## Evidence
Cause 1 - check ec2.check-modify-volume-size-decrease-rejected, mismatch: code assertion (InvalidParameterValue) passed; AssertionError on the message assertion - expected "New size cannot be smaller than existing size", got "Invalid value '5' for size. The target size must be greater than or equal to the volume's existing size of 10 GiB" (ModifyVolume(VolumeId=<a 10 GiB gp3 volume>, Size=5)).
Cause 2 - check ec2__ssm.vscan-instance-information-filter-rejects-a-malformed-instance-id, mismatch: code assertion (InvalidInstanceID.Malformed) passed; AssertionError: expected the exact EC2 message 'Invalid id: "not-an-instance-id"', got "Invalid id: 'not-an-instance-id'" (DescribeInstances(InstanceIds=['not-an-instance-id'])).
The sizes, ids and names above come from one run and are examples; the requirements are the HTTP 400, the error codes InvalidParameterValue / InvalidInstanceID.Malformed, and the message RULE (AWS's constant shrink text; prefix + request value in double quotes).

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: CreateVolume(AvailabilityZone=<an available AZ>, Size=10, VolumeType='gp3'); wait for 'available'; ModifyVolume(VolumeId=<id>, Size=5) -> expect 400 InvalidParameterValue with message "New size cannot be smaller than existing size"; observe the emulator's own request-derived sentence instead.
3. Cause 2: DescribeInstances(InstanceIds=['not-an-instance-id']) -> expect 400 InvalidInstanceID.Malformed with message 'Invalid id: "not-an-instance-id"' (double quotes); observe single quotes around the value.
