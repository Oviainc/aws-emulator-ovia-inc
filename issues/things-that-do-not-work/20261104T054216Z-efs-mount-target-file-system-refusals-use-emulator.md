Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: efs.create-mount-target-duplicate-az-rejected efs.create-mount-target-lifecycle-transition efs.delete-file-system-in-use-rejected efs.delete-mount-target-lifecycle-transition
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: efs.create-mount-target-duplicate-az-rejected|run#19|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: efs.create-mount-target-lifecycle-transition|run#18|AssertionError|assert ip in cidr, f"{ip} not within subnet CIDR {cidr}"
Issue-Key: efs.delete-file-system-in-use-rejected|run#15|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: efs.delete-mount-target-lifecycle-transition|run#18|AssertionError|assert mutate_code == "MountTargetNotFound", (
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# EFS mount-target/file-system refusals use emulator-authored wording, a mount target's IpAddress comes from an invented CIDR, and ModifyMountTargetSecurityGroups answers 501 (4 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMountTarget

## Description
Four net-new root causes in the EFS mount-target and file-system-delete paths. All four are in the efs service crate; each has its own failing behavioral check and must be closed separately.

Cause 1 of 4: CreateMountTarget's MountTargetConflict message is emulator-authored prose instead of AWS's text (check efs.create-mount-target-duplicate-az-rejected).
AWS: Smithy: CreateMountTarget error MountTargetConflict, errorKind client, httpError 409, members ErrorCode (@required) + Message, documented "Returned if the mount target would violate one of the specified restrictions based on the file system's existing mount targets"; the operation doc states "there can be only one mount target per Availability Zone". The recorded behaviour shows status 409 with ErrorCode "MountTargetConflict" and Message exactly `mount target already exists in this AZ` — that whole sentence is FIXED text with NO variable part: it names neither the file system, nor the existing mount target, nor the Availability Zone. AWS returns the same message whether the retried subnet is the same subnet or a different subnet in the already-occupied AZ (the conflict is keyed on the AZ).
Emulator: the error class and status are right, but the Message is `File system <FileSystemId> already has mount target <MountTargetId> in Availability Zone <AvailabilityZoneName>` (observed: "File system fs-e220a8397b1dcdaf already has mount target fsmt-6e789e6aa1b965f4 in Availability Zone us-west-2a"). The ids/zone in that observation are per-run values; the defect is that the sentence itself is not AWS's.
Why: a client (or a test/retry policy) that matches on the refusal text to tell an AZ conflict from the other MountTargetConflict restrictions sees a sentence AWS never emits, and anything that logs or asserts the refusal diverges.
Fix: the CreateMountTarget handler's conflict refusal in the efs service crate — emit the fixed AWS sentence for the per-AZ restriction instead of a formatted one.
Traces: efs.create-mount-target-duplicate-az-rejected.

Cause 2 of 4: a mount target's IpAddress is allocated from a CIDR the emulator invents for the subnet, not from the requested subnet's real CIDR (check efs.create-mount-target-lifecycle-transition).
AWS: Smithy: MountTargetDescription.IpAddress, "Address at which the file system can be mounted by using the mount target", pattern ^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}$. The CreateMountTarget doc states the SubnetId determines "The IP address range from which Amazon EFS selects the IP address of the mount target (if you don't specify an IP address in the request)" and that EFS "assigns a free address in the subnet (in the same way that the Amazon EC2 CreateNetworkInterface call does ...)". The requirement: the returned/persisted IpAddress is an address inside the CidrBlock of the subnet named in the request, as EC2 reports that subnet.
Emulator: returns an address from a /24 the EFS crate synthesizes for the subnet id on first reference, unrelated to the subnet's EC2 CidrBlock — the mount target reached `available` with an IpAddress outside the subnet (observed 10.0.0.4 for a subnet whose EC2 CidrBlock is 172.31.0.0/20). The address never lies in the subnet, for any subnet, because the subnet record is fabricated rather than resolved.
Why: the IpAddress is exactly what a client uses to mount the file system ("You use this IP address when mounting the file system in an EC2 instance"); an address outside the subnet is unroutable from the instances in it, so every NFS mount, security-group rule and route built from the reply targets an address that belongs to no network the caller owns.
Fix: the mount-target subnet resolution in the efs service's state (crate::state::subnet / free_ipv4, used by the CreateMountTarget handler's address allocation) — resolve the SubnetId against the emulator's EC2 subnet record and allocate the free address out of that subnet's CidrBlock. Related but NOT the same cause as open issue 20261029T195311Z-elasticfilesystem-efs-mount-targets-ignore-ec2-subnet-state, which covers SubnetNotFound, VpcId and the delete it blocks; the IpAddress allocation is not among its four causes.
Traces: efs.create-mount-target-lifecycle-transition.

Cause 3 of 4: DeleteFileSystem's FileSystemInUse message is emulator-authored prose (check efs.delete-file-system-in-use-rejected).
AWS: Smithy: DeleteFileSystem, DELETE /2015-02-01/file-systems/{FileSystemId}, success 204; error FileSystemInUse, errorKind client, httpError 409, documented "Returned if a file system has mount targets." The recorded refusal is status 409, ErrorCode "FileSystemInUse", Message `The file system '<FileSystemId>' cannot be deleted because it has mount targets. You must delete the mount targets, and then try again.` — the fixed text is "The file system '" ... "' cannot be deleted because it has mount targets. You must delete the mount targets, and then try again." and the single variable part is the FileSystemId copied from the request. AWS does NOT report how many mount targets there are.
Emulator: answers 409 FileSystemInUse with Message `File system '<FileSystemId>' still has <n> mount target(s); delete them first` (observed: "File system 'fs-e220a8397b1dcdaf' still has 1 mount target(s); delete them first").
Why: a client that surfaces or matches the refusal gets a sentence AWS never emits, and the count it leaks is not part of AWS's contract.
Fix: the DeleteFileSystem handler's in-use refusal in the efs service crate — build the AWS sentence from the request's FileSystemId instead of formatting the mount-target count.
Traces: efs.delete-file-system-in-use-rejected.

Cause 4 of 4: ModifyMountTargetSecurityGroups is an unimplemented scaffold stub and answers 501 NotImplemented for every request (check efs.delete-mount-target-lifecycle-transition).
AWS: Smithy: ModifyMountTargetSecurityGroups, PUT /2015-02-01/mount-targets/{MountTargetId}/security-groups, success 204 with an empty body; errors BadRequest 400, IncorrectMountTargetState 409, MountTargetNotFound 404 ("Returned if there is no mount target with the specified ID found in the caller's Amazon Web Services account"), SecurityGroupLimitExceeded 400, SecurityGroupNotFound 400. For a mount target that has been deleted, the recorded refusal is 404 with ErrorCode "MountTargetNotFound" and Message `Mount target '<MountTargetId>' does not exist.` — fixed text "Mount target '" ... "' does not exist." with the MountTargetId taken from the request (the same sentence DescribeMountTargets already produces correctly for a deleted id).
Emulator: answers 501 with error code `NotImplemented` for the operation unconditionally — so a deleted mount target is not reported as dead on the mutation path, and a live mount target's security groups can never be changed at all.
Why: a client cannot modify a mount target's security groups, and a client probing whether a deleted mount-target id is really gone gets a server error class (501) instead of the client-side 404 MountTargetNotFound it retries or branches on; 501 is not a modeled error of this operation, so SDK error handling falls through to generic failure.
Fix: implement the handler in the efs service crate (ops/modify_mount_target_security_groups.rs still returns crate::scaffolded::Scaffolded::not_implemented) — resolve MountTargetId in state, refuse a missing one with MountTargetNotFound (404, the sentence above), validate the security groups, otherwise update the record and return 204. No open issue names this operation.
Traces: efs.delete-mount-target-lifecycle-transition.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: efs 19/28 (68%), 9 failed (5 of them already owned by open issues).

Cause 1 — efs.create-mount-target-duplicate-az-rejected [mismatch]: AssertionError: expected message 'mount target already exists in this AZ', got 'File system fs-e220a8397b1dcdaf already has mount target fsmt-6e789e6aa1b965f4 in Availability Zone us-west-2a'. ErrorCode MountTargetConflict and the 409 status matched; only the Message diverged. Source: services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/create_mount_target.rs, the MountTargetConflict arm around lines 388-401.

Cause 2 — efs.create-mount-target-lifecycle-transition [mismatch]: AssertionError: 10.0.0.4 not within subnet CIDR 172.31.0.0/20. The mount target did reach LifeCycleState available and DescribeMountTargets by FileSystemId returned exactly one entry; only the address is wrong. Source: ops/create_mount_target.rs (the `None => match state.free_ipv4(&subnet)` branch, around line 437) over services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/state.rs `subnet()` (~line 656, which mints an AZ and a /24 by allocation ordinal for any unseen subnet id) and `free_ipv4()` (~line 784).

Cause 3 — efs.delete-file-system-in-use-rejected [mismatch]: AssertionError: expected message "The file system 'fs-e220a8397b1dcdaf' cannot be deleted because it has mount targets. You must delete the mount targets, and then try again.", got "File system 'fs-e220a8397b1dcdaf' still has 1 mount target(s); delete them first". Code FileSystemInUse matched. Source: src/ops/delete_file_system.rs lines 197-209.

Cause 4 — efs.delete-mount-target-lifecycle-transition [mismatch]: the DescribeMountTargets-after-delete assertions passed (MountTargetNotFound with "Mount target 'fsmt-...' does not exist."), then AssertionError: expected a mutation on the deleted mount target to also fail with MountTargetNotFound, got 'NotImplemented'. Source: src/ops/modify_mount_target_security_groups.rs line 198, `Err(crate::scaffolded::Scaffolded::not_implemented("ModifyMountTargetSecurityGroups"))`.

## Steps to Reproduce
1. git clone the repo; build and boot the emulator from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 (the four check ids in trace_ids).
3. Cause 1: CreateFileSystem, wait available, CreateMountTarget in subnet A (wait available), then CreateMountTarget again in subnet A and again in a different subnet of the SAME Availability Zone. Expect 409 MountTargetConflict with Message exactly 'mount target already exists in this AZ' both times; observe the formatted "File system ... already has mount target ... in Availability Zone ..." sentence.
4. Cause 2: CreateFileSystem, wait available, CreateMountTarget into a subnet whose EC2 DescribeSubnets CidrBlock is known (e.g. 172.31.0.0/20), poll DescribeMountTargets until available, and check IpAddress; expect an address inside that CidrBlock, observe an address from an unrelated /24 (10.0.0.4).
5. Cause 3: with one mount target attached and available, call DeleteFileSystem; expect 409 FileSystemInUse with Message "The file system '<FileSystemId>' cannot be deleted because it has mount targets. You must delete the mount targets, and then try again.", observe "File system '<FileSystemId>' still has 1 mount target(s); delete them first".
6. Cause 4: DeleteMountTarget, poll until DescribeMountTargets raises MountTargetNotFound, then call ModifyMountTargetSecurityGroups on that id; expect 404 MountTargetNotFound with "Mount target '<MountTargetId>' does not exist.", observe 501 NotImplemented. Also call ModifyMountTargetSecurityGroups on a LIVE mount target: expect 204 and the new security groups on the next describe, observe 501.
