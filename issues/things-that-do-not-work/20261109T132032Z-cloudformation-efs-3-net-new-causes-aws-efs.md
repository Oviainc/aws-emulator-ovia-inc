Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__efs.deploy-cfn-efs-file-system-resource-lifecycle cloudformation__efs.deploy-cfn-efs-file-system-provisioned-throughput-missing-rejected cloudformation__efs.deploy-cfn-efs-access-point-and-mount-target-round-trip-and-delete cloudformation__efs.deploy-cfn-efs-mount-target-second-in-same-az-rejected
Issue-Key: cloudformation__efs|DescribeFileSystems|ValidationException
Issue-Key: cloudformation__efs.deploy-cfn-efs-access-point-and-mount-target-round-trip-and-delete|stack_outputs#7|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__efs.deploy-cfn-efs-mount-target-second-in-same-az-rejected|run#6|AssertionError|assert status == "CREATE_FAILED", f"expected stack status CREATE_FAILED, got {status!r}"
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudformation__efs: 3 net-new causes — AWS::EFS::* resources are never provisioned in EFS and Ref returns a synthesized physical id, stack Outputs carry no OutputValue, and a duplicate-AZ MountTarget stack reaches CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three distinct causes, all reached through CloudFormation templates that declare EFS resources. Every one is grounded in a failing behavioral check listed under '### cloudformation__efs'.

Cause 1 of 3: an AWS::EFS::FileSystem (and the AWS::EFS::AccessPoint / AWS::EFS::MountTarget that reference it) reaches CREATE_COMPLETE without being created in EFS, and its PhysicalResourceId / Ref is a CloudFormation-synthesized name instead of the EFS FileSystemId.
AWS: CloudFormation provisions AWS::EFS::FileSystem by calling elasticfilesystem:CreateFileSystem; the resource's physical id — what Ref and DescribeStackResource/ListStackResources.PhysicalResourceId return — IS the EFS FileSystemId, i.e. 'fs-' plus 8..40 hex digits. Smithy (efs): CreateMountTarget/CreateAccessPoint/DescribeFileSystems input member FileSystemId carries pattern ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$, and DescribeFileSystems answers with that file system's FileSystemDescription (Encrypted/PerformanceMode/ThroughputMode/FileSystemArn echoing the template).
Emulator: the stack reaches CREATE_COMPLETE, but the physical id handed back is of the form '<stack-name>-<LogicalId>-<hex>' (recorded shapes: 'sim2real-behav-stack-336055fd3c88-Fs-6D048948DF25' from the Ref output, 'sim2real-behav-stack-ok-95d6dc551be9-Fs-95039FE8A6FC' from DescribeStackResource.PhysicalResourceId — the stack name, logical id and hex suffix are per-run values, the '<stack>-<logical-id>-<suffix>' template is the defect). Feeding it to EFS therefore fails input validation: DescribeFileSystems answers 400 ValidationException with message '1 validation error detected: Value '<the synthesized id>' at 'fileSystemId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$' (fixed text is AWS's standard validation wording; the variable parts are the value from the request, the member name 'fileSystemId' and the modeled pattern). Hypothesis: the AWS::EFS::* resource types are registered but their handlers only mint a physical id and never call the EFS service, so no file system exists to describe.
Why: a client cannot use a CloudFormation-deployed EFS estate at all — it cannot read back the file system it just deployed, cannot pass Ref/GetAtt to any other service, and cannot assert the stack's effect. Deleting the stack likewise removes nothing from EFS.
Fix: the CloudFormation AWS::EFS::FileSystem / ::AccessPoint / ::MountTarget resource handlers (create/read/delete), which must call the EFS service and record the returned FileSystemId / AccessPointId / MountTargetId as the resource's PhysicalResourceId.
Traces: cloudformation__efs.deploy-cfn-efs-file-system-resource-lifecycle, cloudformation__efs.deploy-cfn-efs-file-system-provisioned-throughput-missing-rejected.

Cause 2 of 3: DescribeStacks returns a CREATE_COMPLETE stack's Outputs entries with OutputKey but no OutputValue.
AWS: Smithy (cloudformation): Output has members OutputKey, OutputValue ('the value associated with the output'), Description, ExportName. For a stack in CREATE_COMPLETE every declared Output is resolved and present with its OutputValue; an Output whose Value cannot be resolved makes the stack fail rather than being serialized value-less.
Emulator: DescribeStacks on a CREATE_COMPLETE stack whose Outputs are !Ref / !GetAtt of EFS resources (FsIdRef, ApIdRef, ApArnGetAtt, MtIdRef, MtIpGetAtt) returns Outputs rows that omit OutputValue entirely, so a client reading o['OutputValue'] raises KeyError.
Why: every consumer of stack outputs breaks — outputs are the documented contract for passing deployed ids to the next step, and an entry without a value is indistinguishable from an empty stack while the status claims success. Note: open issues 20261109T113512Z-cloudformation-ec2-network-firewall-describestacks-returns-a-create.md and 20261109T115509Z-cloudformation-ec2-memorydb-describestacks-returns-a-create-complete.md report the same symptom under other service groups; neither covers the EFS checks here.
Fix: the CloudFormation output-resolution step (Ref/GetAtt resolution for the EFS resource types) and the DescribeStacks serializer, which must never emit an Output row without OutputValue.
Traces: cloudformation__efs.deploy-cfn-efs-access-point-and-mount-target-round-trip-and-delete.

Cause 3 of 3: a stack declaring a second AWS::EFS::MountTarget in a subnet that shares the first mount target's Availability Zone reaches CREATE_COMPLETE instead of CREATE_FAILED.
AWS: EFS permits at most one mount target per file system per Availability Zone. Smithy (efs): CreateMountTarget declares error MountTargetConflict (client, HTTP 409); CloudFormation surfaces the service refusal on the MtTwo resource as a CREATE_FAILED event whose ResourceStatusReason is the resource-handler wrapping 'Resource handler returned message: "mount target already exists in this AZ" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)' — the quoted message and HandlerErrorCode AlreadyExists are the fixed requirement, the request token is a per-run value — and the stack ends CREATE_FAILED (DisableRollback=True), with EFS listing only the first mount target.
Emulator: the stack with both mount targets reaches CREATE_COMPLETE; no CREATE_FAILED event and no refusal are produced for MtTwo.
Why: a client validating its template against the one-mount-target-per-AZ rule is told the deployment succeeded, so an invalid topology is reported as live and the AlreadyExists failure path can never be exercised. (This is the CloudFormation-level face of the EFS refusal; the direct-API refusal is owned by open issue 20261104T054216Z-efs-mount-target-file-system-refusals-use-emulator.md, which does not cover this stack-level check.)
Fix: the AWS::EFS::MountTarget create handler, which must route the request through EFS's CreateMountTarget validation and map the MountTargetConflict refusal onto a CREATE_FAILED resource event with HandlerErrorCode AlreadyExists.
Traces: cloudformation__efs.deploy-cfn-efs-mount-target-second-in-same-az-rejected.

## Evidence
Cause 1 — Expected (AWS): DescribeStackResource/Ref for the AWS::EFS::FileSystem returns an fs-<hex> id, and DescribeFileSystems(FileSystemId=that) returns the file system with Encrypted=true, PerformanceMode=generalPurpose, ThroughputMode=bursting. Actual (emulator): botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the DescribeFileSystems operation: 1 validation error detected: Value 'sim2real-behav-stack-336055fd3c88-Fs-6D048948DF25' at 'fileSystemId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:file-system/fs-[0-9a-f]{8,40}|fs-[0-9a-f]{8,40})$ (same with 'sim2real-behav-stack-ok-95d6dc551be9-Fs-95039FE8A6FC' from DescribeStackResource.PhysicalResourceId).

Cause 2 — Expected (AWS): DescribeStacks()['Stacks'][0]['Outputs'] rows each carry OutputKey and OutputValue. Actual (emulator): KeyError: 'OutputValue' when reading {o['OutputKey']: o['OutputValue'] for o in stack.get('Outputs', [])} on a CREATE_COMPLETE stack.

Cause 3 — Expected (AWS): stack status CREATE_FAILED with the MtTwo CREATE_FAILED reason ('mount target already exists in this AZ', HandlerErrorCode 'AlreadyExists'). Actual (emulator): AssertionError: expected stack status CREATE_FAILED, got 'CREATE_COMPLETE'.

## Steps to Reproduce
1. CreateStack with a template declaring Fs: AWS::EFS::FileSystem (Encrypted: true, PerformanceMode: generalPurpose, ThroughputMode: bursting) and Outputs FileSystemIdRef: !Ref Fs, FileSystemArnGetAtt: !GetAtt Fs.Arn. Wait for CREATE_COMPLETE.
2. Read DescribeStackResource(LogicalResourceId='Fs').StackResourceDetail.PhysicalResourceId and the Ref output: both are '<stack>-Fs-<hex>', not fs-<hex>. Call elasticfilesystem:DescribeFileSystems(FileSystemId=that) and observe the 400 ValidationException on 'fileSystemId'.
3. CreateStack with a VPC, subnet, security group, an AWS::EFS::FileSystem, an AWS::EFS::AccessPoint and an AWS::EFS::MountTarget, with Outputs that !Ref each resource and !GetAtt Ap.Arn / Mt.IpAddress. After CREATE_COMPLETE, call DescribeStacks and observe Outputs rows with OutputKey and no OutputValue.
4. CreateStack (DisableRollback=True) with one file system and two AWS::EFS::MountTarget resources whose subnets share one Availability Zone (SubnetTwo's AvailabilityZone: !GetAtt SubnetOne.AvailabilityZone, MtTwo DependsOn MtOne). Observe CREATE_COMPLETE instead of CREATE_FAILED and the absent 'mount target already exists in this AZ' / AlreadyExists event for MtTwo.
