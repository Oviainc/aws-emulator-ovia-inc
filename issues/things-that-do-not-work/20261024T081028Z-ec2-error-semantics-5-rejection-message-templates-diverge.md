Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.delete-vpc-with-subnet-fails-dependency-violation ec2.op-contract-deleteVpc ec2.security-group-delete-while-in-use-fails ec2.subnet-cidr-overlap-conflict-fails ec2.op-contract-create-placement-group ec2.detach-volume-not-attached-state-conflict-rejected cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected
Issue-Key: ec2.delete-vpc-with-subnet-fails-dependency-violation|run#15|AssertionError|assert refused.get("Error", {}).get("Message") == expected_message, f'expected the message {expected_message!r}, got {refused.get("Error", {}).get("Message")!r}
Issue-Key: ec2.op-contract-deleteVpc|run#10|AssertionError|assert err == (
Issue-Key: ec2.security-group-delete-while-in-use-fails|run#21|AssertionError|assert err.get("Message") == f"resource {gid} has a dependent object", (
Issue-Key: ec2.subnet-cidr-overlap-conflict-fails|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ec2.op-contract-create-placement-group|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ec2.detach-volume-not-attached-state-conflict-rejected|run#8|AssertionError|assert message == f"Volume '{vol_id}' is in the 'available' state.", (
Issue-Key: cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements|run#17|AssertionError|assert nf_code == _SDK_NOT_FOUND, (
Issue-Key: ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected|_drain#21|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# EC2 error semantics: 5 rejection-message templates diverge from AWS, DescribeInstances misclassifies a valid 8-character instance id, and CreateVolume under a disabled CMK never emits its failure notification

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVpc

## Description
Seven net-new root causes in the EC2 emulator, all grounded in failing behavioral checks. Error CODES are correct in causes 1-5; the MESSAGE text the emulator builds diverges from AWS's fixed, request-derived templates. Cause 6 is a wrong error code. Cause 7 is a missing asynchronous effect.

Cause 1 of 7: DeleteVpc DependencyViolation message appends the dependent resource id.
AWS: recorded behavior only (EC2's Query protocol errors are not modelled as Smithy shapes). DeleteVpc on a VPC that still contains a subnet is refused with HTTP 400, error code DependencyViolation, and a message built from the request naming only the VPC: "The vpc 'vpc-...' has dependencies and cannot be deleted." (trailing period, no dependent ids).
Emulator: same 400/DependencyViolation, but the message is "The vpc 'vpc-0000000000000001c' has dependencies and cannot be deleted: subnet-00000000000000020" - the trailing period is dropped and the internal dependent id is appended.
Why: clients and test harnesses that match AWS's documented DependencyViolation text (and tooling that parses it) break, and the emulator leaks an internal dependent id AWS does not disclose on this path.
Fix: the DeleteVpc handler's error construction in services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_vpc.rs - emit AWS's template and stop appending the dependency list.
Traces: ec2.delete-vpc-with-subnet-fails-dependency-violation, ec2.op-contract-deleteVpc.

Cause 2 of 7: DeleteSecurityGroup DependencyViolation uses an entirely different message template.
AWS: recorded behavior only. Deleting a security group still attached to a running instance is refused 400 DependencyViolation with the message "resource <sg-id> has a dependent object".
Emulator: 400 DependencyViolation with "The security group 'sg-00000000000000022' has dependencies and cannot be deleted: i-00000000000000001" - a VPC-style template re-used for security groups, plus the dependent instance id.
Why: the two AWS templates are different on purpose; a client that keys off the security-group text sees an unrecognised message, and the instance id is leaked.
Fix: the DeleteSecurityGroup handler's error construction in services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_security_group.rs - give security groups their own message rule rather than sharing the VPC one.
Traces: ec2.security-group-delete-while-in-use-fails.

Cause 3 of 7: CreateSubnet InvalidSubnet.Conflict message appends the conflicting subnet id.
AWS: recorded behavior only. A CreateSubnet whose CIDR overlaps an existing subnet in the same VPC is refused 400 InvalidSubnet.Conflict with a message built from the request CIDR alone: "The CIDR '<cidr>' conflicts with another subnet".
Emulator: 400 InvalidSubnet.Conflict with "The CIDR '10.4.1.128/25' conflicts with another subnet ('subnet-00000000000000020')" - the conflicting subnet id is appended in parentheses.
Why: the message no longer matches AWS's rule (request CIDR only), and it discloses which existing subnet conflicted.
Fix: the CreateSubnet validation/error path in services/service-com-amazonaws-ec2-amazonec2/src/ops/create_subnet.rs.
Traces: ec2.subnet-cidr-overlap-conflict-fails.

Cause 4 of 7: CreatePlacementGroup duplicate-name message drops AWS's trailing period.
AWS: recorded behavior only. Re-using an existing GroupName is refused 400 InvalidPlacementGroup.Duplicate with "The placement group '<name>' already exists." - the message is built from the request and ends in a period.
Emulator: 400 InvalidPlacementGroup.Duplicate with "The placement group '<name>' already exists" (no period).
Why: exact-match assertions on AWS's documented text fail; the code is right but the message rule is not implemented faithfully.
Fix: the CreatePlacementGroup duplicate-detection error in services/service-com-amazonaws-ec2-amazonec2/src/ops/create_placement_group.rs.
Traces: ec2.op-contract-create-placement-group.

Cause 5 of 7: DetachVolume IncorrectState message adds an explanatory clause AWS does not emit.
AWS: recorded behavior only. DetachVolume on an existing volume that is not attached is refused 400 IncorrectState with a message built from the request: "Volume '<vol-id>' is in the 'available' state."
Emulator: 400 IncorrectState with "Volume 'vol-0000000000000001c' is in the 'available' state, which is not a state it can be detached from".
Why: the message rule (volume id + current state, full stop) is the part a client can rely on; the emulator's extra clause breaks it.
Fix: the DetachVolume state-check error in services/service-com-amazonaws-ec2-amazonec2/src/ops/detach_volume.rs.
Traces: ec2.detach-volume-not-attached-state-conflict-rejected.

Cause 6 of 7: DescribeInstances classifies a well-formed but unknown 8-character instance id as InvalidInstanceID.Malformed instead of InvalidInstanceID.NotFound.
AWS: an instance id is valid in BOTH the legacy 8-hex-character and the 17-hex-character form - the SSM model states this rule explicitly for the same identifier space (Smithy, ssm DescribeInstanceInformation: pattern "(^i-(\\w{8}|\\w{17})$)|(^mi-\\w{17}$)"). DescribeInstances naming 'i-3e44e586' - well formed, 8 characters, not present in the account - is therefore refused 400 with error code InvalidInstanceID.NotFound, not a malformed-syntax error.
Emulator: returns error code InvalidInstanceID.Malformed for 'i-3e44e586'.
Why: this is a wrong error code, not a wording difference. A client distinguishing "you passed garbage" (never retry, fix the caller) from "that instance is gone" (expected after a terminate; retry/reconcile) takes the wrong branch, and CloudTrail consumers auditing client error codes record the wrong code. It also means the emulator only accepts 17-character ids, so every legacy-format id in a request is rejected outright.
Fix: the instance-id parser/validator shared by the EC2 describe paths (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instances.rs and the id-validation helper it calls) must accept the 8-character form and fall through to the not-found classification.
Note: an open issue 20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md covers other malformed-id misclassifications in EC2 and does NOT carry this check's replay key; this cause is filed separately and that issue is named here so the two can be fixed together if they share the helper.
Traces: cloudtrail__ec2.obs-ec2-3-rejected-ec2-calls-audit-client-error-codes-with-no-response-elements.

Cause 7 of 7: CreateVolume against a disabled KMS CMK never produces its failed "EBS Volume Notification", so the check never completes.
AWS: recorded behavior only. A CreateVolume naming a disabled customer master key is accepted or rejected and then, asynchronously, exactly ONE EventBridge event with source "aws.ec2" and detail-type "EBS Volume Notification" reporting the failed result is delivered; the check polls for it within a 300 s delivery budget.
Emulator: no such notification is ever delivered - the check makes no progress and the harness kills it with "Check timed out after 60.0s" (the only diagnostic the harness emits for a hang; no frame is reported).
Why: anything that reacts to EBS volume failures through EventBridge (alarms, remediation rules, cleanup lambdas) never fires against the emulator, and a caller waiting on the notification hangs instead of being told the volume failed.
Fix: the EBS volume state machine / event emission in the EC2 service state (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_volume.rs and the EventBridge publication path it should drive).
Traces: ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected.

## Evidence
Cause 1 - expected message "The vpc 'vpc-0000000000000001c' has dependencies and cannot be deleted.", got "The vpc 'vpc-0000000000000001c' has dependencies and cannot be deleted: subnet-00000000000000020" (code DependencyViolation matched in both checks).
Cause 2 - expected DependencyViolation message "resource sg-00000000000000022 has a dependent object", got "The security group 'sg-00000000000000022' has dependencies and cannot be deleted: i-00000000000000001".
Cause 3 - expected "The CIDR '10.4.1.128/25' conflicts with another subnet", got "The CIDR '10.4.1.128/25' conflicts with another subnet ('subnet-00000000000000020')" (code InvalidSubnet.Conflict matched).
Cause 4 - expected "The placement group 'sim2real-pg-9686f143fefa' already exists.", got "The placement group 'sim2real-pg-9686f143fefa' already exists" (code InvalidPlacementGroup.Duplicate matched).
Cause 5 - expected "Volume 'vol-0000000000000001c' is in the 'available' state.", got "Volume 'vol-0000000000000001c' is in the 'available' state, which is not a state it can be detached from" (code IncorrectState matched).
Cause 6 - AssertionError: the SDK reported 'InvalidInstanceID.Malformed' for DescribeInstances on unknown id 'i-3e44e586', expected 'InvalidInstanceID.NotFound'.
Cause 7 - Error: Check timed out after 60.0s (no exception frame); the check polls for one aws.ec2 / "EBS Volume Notification" event with a 300 s delivery budget and never receives it.
The resource ids, names and timestamps above are from one run and are examples, not requirements; the requirements are the status, the error code, and the message RULE (which members AWS's template is built from).

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: create a VPC, create a subnet in it, call DeleteVpc -> compare the DependencyViolation message to "The vpc '<id>' has dependencies and cannot be deleted."
3. Cause 2: run an instance in a security group, then DeleteSecurityGroup -> compare the message to "resource <sg-id> has a dependent object".
4. Cause 3: create a subnet 10.4.1.0/24, then CreateSubnet 10.4.1.128/25 in the same VPC -> compare the InvalidSubnet.Conflict message.
5. Cause 4: CreatePlacementGroup twice with the same GroupName -> compare the InvalidPlacementGroup.Duplicate message (trailing period).
6. Cause 5: CreateVolume, then DetachVolume on it while it is 'available' -> compare the IncorrectState message.
7. Cause 6: DescribeInstances(InstanceIds=['i-3e44e586']) -> expect error code InvalidInstanceID.NotFound, observe InvalidInstanceID.Malformed.
8. Cause 7: disable a KMS CMK, CreateVolume naming it, subscribe an SQS queue to an EventBridge rule on source aws.ec2 / detail-type "EBS Volume Notification", poll for the failure event -> none arrives.
