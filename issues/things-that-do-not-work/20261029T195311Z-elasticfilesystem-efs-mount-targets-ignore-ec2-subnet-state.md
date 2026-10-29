Issue-Source: trace-sweep-divergence
Issue-Trace: efs-mount-target-replication-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: efs-mount-target-replication-error-paths efs-mount-target-security-groups-workflow
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: elasticfilesystem|CreateMountTarget|4xx->2xx|$.ErrorCode
Issue-Key: elasticfilesystem|CreateMountTarget|4xx->2xx|$.Message
Issue-Key: elasticfilesystem|CreateMountTarget|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: elasticfilesystem|CreateMountTarget|4xx->2xx|status
Issue-Key: elasticfilesystem|CreateMountTarget|2xx->2xx|$.VpcId
Issue-Key: elasticfilesystem|DescribeMountTargets|2xx->2xx|$.MountTargets[*]
Issue-Key: elasticfilesystem|DeleteFileSystem|2xx->4xx|status
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# EFS mount targets ignore EC2 subnet state

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMountTarget

## Description
CreateMountTarget never resolves the request's SubnetId against the emulator's EC2 subnet state: a subnet that does not exist is accepted, and the VPC the mount target reports is invented rather than taken from the subnet. Four causes, all in the mount-target path.

Cause 1 of 4: CreateMountTarget accepts a SubnetId that does not exist (fixture efs-mount-target-replication-error-paths, step task#1).
AWS: Smithy: CreateMountTarget.SubnetId is @required, length 15..47, pattern ^subnet-[0-9a-f]{8,40}$; modeled error SubnetNotFound, errorKind client, httpError 400, members ErrorCode (@required) + Message, documented "Returned if there is no subnet with ID SubnetId provided in the request." The recording shows status 400 with body ErrorCode "SubnetNotFound" and Message "The subnet ID '<SubnetId>' does not exist" - the fixed text is "The subnet ID '" ... "' does not exist" and the only variable part is the SubnetId copied from the request - plus an x-amzn-errortype header naming the error shape.
Emulator: answers 200 with a full MountTargetDescription; the body carries no ErrorCode and no Message, and no x-amzn-errortype header is sent.
Why: a client that handles SubnetNotFound to pick a valid subnet instead gets a success and believes it owns a mount target in a subnet that does not exist; every later describe, mount and delete decision is taken against a resource AWS would never have created, and the file system is left in an in-use state it should not be in (see Cause 4).
Fix: the CreateMountTarget handler's input validation in the efs service crate - look the SubnetId up in the emulator's EC2 subnet state before any state mutation and raise SubnetNotFound (400) when it misses.
Traces: efs-mount-target-replication-error-paths#task#1.

Cause 2 of 4: CreateMountTarget's reply reports a VpcId that is not the subnet's VPC (fixture efs-mount-target-security-groups-workflow, step task#1).
AWS: Smithy: MountTargetDescription.VpcId, documented "The virtual private cloud (VPC) ID that the mount target is configured in" - i.e. the VPC that owns the SubnetId named in the request. In the recording the returned VpcId is exactly the VPC that the requested subnet belongs to, as created earlier in the same fixture.
Emulator: returns a VpcId it synthesizes for itself, unrelated to the requested subnet (in the recorded run the subnet's VPC was the fixture-created vpc and the emulator answered an unrelated generated vpc id; the specific ids differ per run - the defect is that the value is not derived from the subnet at all).
Why: a client that uses the returned VpcId to place security groups, route tables or an ENI, or that asserts the mount target landed in the VPC it asked for, is handed an id that matches nothing in its own account.
Fix: the CreateMountTarget handler's response projection in the efs service crate - carry VpcId (and the availability-zone fields) over from the resolved EC2 subnet rather than generating them.
Traces: efs-mount-target-security-groups-workflow#task#1.

Cause 3 of 4: the stored mount target keeps that wrong VpcId, so DescribeMountTargets serves it too (fixture efs-mount-target-security-groups-workflow, step task#2).
AWS: Smithy: DescribeMountTargets returns DescribeMountTargetsResponse.MountTargets, a list of the same MountTargetDescription shape; the VpcId of a described mount target is the VPC of its subnet, stable across the create reply and every later describe.
Emulator: the MountTargets[0] element matches the recording on OwnerId, MountTargetId, FileSystemId, SubnetId, LifeCycleState and AvailabilityZoneName but carries the same invented VpcId as the create reply, so the whole element compares as a mismatch.
Why: the divergence is persisted, not just a one-off serialization slip - a client that re-reads its mount targets to discover the VPC gets the wrong answer for the lifetime of the resource.
Fix: the mount-target record in the efs service's state - persist the subnet-derived VpcId at create time so both CreateMountTarget and DescribeMountTargets project the same, correct value.
Traces: efs-mount-target-security-groups-workflow#task#2.

Cause 4 of 4: DeleteFileSystem refuses the teardown with 409 instead of 204 (fixture efs-mount-target-replication-error-paths, step teardown#200).
AWS: Smithy: DeleteFileSystem, DELETE /2015-02-01/file-systems/{FileSystemId}, success code 204 with an empty body; FileSystemInUse is errorKind client, httpError 409, documented "Returned if a file system has mount targets." In the recording AWS had refused the mount-target create (Cause 1), so at teardown the file system has no mount targets and the delete answers 204.
Emulator: answers 409. It created the mount target it should have refused, so its FileSystemInUse gate fires on a mount target that AWS never created.
Why: teardown cannot complete - the file system is never deleted, so a client's cleanup path fails and the resource leaks for the rest of the session.
Fix: follows from Cause 1 (the FileSystemInUse gate in the DeleteFileSystem handler is counting a mount target that should not exist), but it is its own replayable step and must be re-replayed to confirm the delete returns 204 with an empty body once the subnet validation is in place.
Traces: efs-mount-target-replication-error-paths#teardown#200.

No open issue covers this path: the open EFS issues are 20261022T154034Z-efs-control-plane-backup-policy-file-system-policy (backup/file-system-policy 501s), 20261029T173204Z-efs-updatefilesystem-badrequest-message-for-a-provisioned-mode (UpdateFileSystem wording) and the three efs__lambda issues; none of them concern mount-target subnet resolution.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (score 70.7%, 8 divergence clusters, all elasticfilesystem):

Cause 1 - efs-mount-target-replication-error-paths#task#1, CreateMountTarget: status expected 400, got 200; $.ErrorCode missing (expected 'SubnetNotFound'); $.Message missing (expected "The subnet ID 'subnet-00000000' does not exist"); $headers.x-amzn-errortype missing (expected the error shape name).

Cause 2 - efs-mount-target-security-groups-workflow#task#1, CreateMountTarget: $.VpcId value_mismatch - expected the requested subnet's VPC, got an unrelated generated vpc id ('vpc-f8c3abcdefd6cc7a' vs 'vpc-00000000000000022' in that run).

Cause 3 - efs-mount-target-security-groups-workflow#task#2, DescribeMountTargets: $.MountTargets[0] mismatch; every member agrees with the recording except VpcId, which repeats the invented value from Cause 2.

Cause 4 - efs-mount-target-replication-error-paths#teardown#200, DeleteFileSystem: status expected 204, got 409 (FileSystemInUse), because the mount target from Cause 1 exists only in the emulator.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture efs-mount-target-replication-error-paths step task#1 - CreateMountTarget with a FileSystemId that exists and a SubnetId that exists in no VPC; expect 400 SubnetNotFound with Message "The subnet ID '<SubnetId>' does not exist", observe 200 and a MountTargetDescription.
4. Cause 2: replay fixture efs-mount-target-security-groups-workflow step task#1 - create a VPC and subnet, then CreateMountTarget into that subnet; expect the reply's VpcId to be that VPC, observe a generated id.
5. Cause 3: replay the same fixture step task#2 - DescribeMountTargets for the file system; expect MountTargets[0].VpcId to equal the subnet's VPC, observe the value from step 4.
6. Cause 4: replay fixture efs-mount-target-replication-error-paths step teardown#200 - DeleteFileSystem for the file system whose mount-target create AWS refused; expect 204 with an empty body, observe 409.
