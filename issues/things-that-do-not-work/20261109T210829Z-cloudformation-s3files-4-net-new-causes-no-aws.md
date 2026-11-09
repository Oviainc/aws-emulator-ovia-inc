Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3files.deploy-s3files-filesystem-round-trip-and-delete cloudformation__s3files.deploy-s3files-filesystem-unversioned-bucket-rejected cloudformation__s3files.deploy-s3files-accesspoint-round-trip-and-delete cloudformation__s3files.deploy-s3files-filesystempolicy-round-trip-and-delete cloudformation__s3files.deploy-s3files-mounttarget-round-trip-and-delete
Issue-Key: cloudformation__s3files|CreateStack|ValidationError
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudformation__s3files: 4 net-new causes — no AWS::S3Files::* resource type is registered, so CreateStack refuses every FileSystem / AccessPoint / FileSystemPolicy / MountTarget template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
CloudFormation's template processor in this emulator knows none of the four AWS::S3Files::* resource types. Every behavioral check that deploys S3 Files through CloudFormation dies on the very first CreateStack call with a 400 ValidationError, so nothing downstream of the stack (Ref/GetAtt, stack Outputs, resource-handler refusals, delete-removes-the-resource) is exercised at all. The four causes below are the four resource types a fixer must register and implement a handler for; each is listed with the checks that are grounded in it.

Cause 1 of 4: AWS::S3Files::FileSystem is not a registered resource type.
AWS: CloudFormation registers AWS::S3Files::FileSystem (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-s3files-filesystem.html). A template declaring it passes template validation, CreateStack returns a StackId, and the resource handler calls the S3 Files control plane — Smithy: CreateFileSystem PUT /file-systems, required input `bucket` (BucketArn, pattern ^(arn:aws[a-zA-Z0-9-]*:s3:::.+)$) and `roleArn` (pattern ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$), plus `tags`. Ref yields the file system ARN, GetAtt exposes FileSystemId, FileSystemArn and Status, stack tags plus the resource's own Tags land on the file system, the file system reaches status `available` (Smithy LifeCycleState) and is the single entry ListFileSystems pages for that bucket; stack deletion deletes it so GetFileSystem then answers ResourceNotFoundException / "File system does not exist." A template whose bucket has versioning Suspended must let the stack rollback with the S3 Files refusal (recorded behavior: InvalidRequest, "Your bucket must have versioning enabled to create a file system. (Service: S3Files, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: <n>)" — fixed text is everything but the parenthesised Request ID and Attempt Count, which are per-run values).
Emulator: CreateStack is refused before any resource is provisioned: HTTP 400, error code `ValidationError`, message `Template format error: Unrecognized resource types: [AWS::S3Files::FileSystem]`. The fixed text there is "Template format error: Unrecognized resource types: " and the bracketed list is the set of types from the request.
Why: no S3 Files file system can be deployed by CloudFormation, CDK or SAM at all. A client gets a hard ValidationError at stack creation instead of a stack, and no amount of template correction helps; infrastructure-as-code adoption of S3 Files is impossible against the emulator.
Fix: the CloudFormation template processor's resource-type registry, plus a new AWS::S3Files::FileSystem resource handler (create/read/update/delete + Ref/GetAtt attribute mapping) delegating to the s3files control plane and surfacing its refusals as the resource's CREATE_FAILED reason.
Traces: cloudformation__s3files.deploy-s3files-filesystem-round-trip-and-delete, cloudformation__s3files.deploy-s3files-filesystem-unversioned-bucket-rejected (and both other checks below, which also declare an FS resource).

Cause 2 of 4: AWS::S3Files::AccessPoint is not a registered resource type.
AWS: CloudFormation registers AWS::S3Files::AccessPoint (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-s3files-accesspoint.html). Its handler calls CreateAccessPoint (Smithy: PUT /access-points; required `fileSystemId`, optional `posixUser`, `rootDirectory`, `tags`); the created access point is the one ListAccessPoints pages for its file system, with the declared POSIX identity, root-directory path and creation permissions, and stack deletion removes it so a read by id answers ResourceNotFoundException.
Emulator: CreateStack refused with HTTP 400 `ValidationError`, `Template format error: Unrecognized resource types: [AWS::S3Files::FileSystem, AWS::S3Files::AccessPoint]`.
Why: a client cannot declare an access point in a template; the whole access-point deployment surface (Ref = access point ARN, GetAtt AccessPointId) is unreachable.
Fix: resource-type registry + an AWS::S3Files::AccessPoint resource handler over the s3files CreateAccessPoint/DeleteAccessPoint operations.
Traces: cloudformation__s3files.deploy-s3files-accesspoint-round-trip-and-delete.

Cause 3 of 4: AWS::S3Files::FileSystemPolicy is not a registered resource type.
AWS: CloudFormation registers AWS::S3Files::FileSystemPolicy (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-s3files-filesystempolicy.html). Its handler calls PutFileSystemPolicy (Smithy: PUT /file-systems/{fileSystemId}/policy; required `fileSystemId` and `policy`), GetFileSystemPolicy then reads back the whole declared document, and an UpdateStack that drops the resource leaves the file system with no policy.
Emulator: CreateStack refused with HTTP 400 `ValidationError`, `Template format error: Unrecognized resource types: [AWS::S3Files::FileSystem, AWS::S3Files::FileSystemPolicy]`.
Why: file-system resource policies cannot be managed as code; the stack-update-removes-policy lifecycle is unavailable.
Fix: resource-type registry + an AWS::S3Files::FileSystemPolicy handler over PutFileSystemPolicy/DeleteFileSystemPolicy, with delete-on-removal semantics.
Traces: cloudformation__s3files.deploy-s3files-filesystempolicy-round-trip-and-delete.

Cause 4 of 4: AWS::S3Files::MountTarget is not a registered resource type.
AWS: CloudFormation registers AWS::S3Files::MountTarget (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-s3files-mounttarget.html). Its handler calls CreateMountTarget (Smithy: PUT /mount-targets; required `fileSystemId` and `subnetId`, optional `securityGroups`), the created mount target is the one DescribeMountTargets/ListMountTargets reports for its file system with the declared subnet, security groups and the subnet's VPC, and stack deletion removes it.
Emulator: CreateStack refused with HTTP 400 `ValidationError`, `Template format error: Unrecognized resource types: [AWS::S3Files::FileSystem, AWS::S3Files::MountTarget]`.
Why: mount targets — the only way a file system is reachable from a VPC — cannot be declared in a template.
Fix: resource-type registry + an AWS::S3Files::MountTarget handler. Note the underlying s3files CreateMountTarget is still an unimplemented scaffold stub, already filed as 20261029T191929Z-s3files-createmounttarget-stub.md; this cause is the separate CloudFormation-side gap and the handler cannot be completed until that stub is implemented.
Traces: cloudformation__s3files.deploy-s3files-mounttarget-round-trip-and-delete.

## Evidence
All five checks fail identically at the first CreateStack, before any resource is provisioned:

Expected (AWS): CreateStack accepts the template and returns a StackId; the stack reaches CREATE_COMPLETE and the declared S3 Files resources exist in the s3files control plane.

Actual (emulator):
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::S3Files::FileSystem]
  ... (ValidationError) ... Template format error: Unrecognized resource types: [AWS::S3Files::FileSystem, AWS::S3Files::AccessPoint]
  ... (ValidationError) ... Template format error: Unrecognized resource types: [AWS::S3Files::FileSystem, AWS::S3Files::FileSystemPolicy]
  ... (ValidationError) ... Template format error: Unrecognized resource types: [AWS::S3Files::FileSystem, AWS::S3Files::MountTarget]

Status 400, error code ValidationError. The bracketed type list is taken from the request's Resources block; the rest of the message is the emulator's fixed text. Because the call is refused, none of the Ref/GetAtt, stack-Outputs, tag-propagation, rollback-reason or delete-removes-the-resource assertions in these checks were ever reached — the per-service rate is cloudformation__s3files 0/5 (0%).

The templates use only modelled properties: FileSystem{Bucket, RoleArn, Tags}, AccessPoint{FileSystemId, PosixUser{Uid,Gid}, RootDirectory{Path, CreationPermissions{OwnerUid,OwnerGid,Permissions}}, Tags}, FileSystemPolicy{FileSystemId, Policy}, MountTarget{FileSystemId, SubnetId, SecurityGroups}.

## Steps to Reproduce
1. Boot the emulator.
2. Call CloudFormation CreateStack with Capabilities=["CAPABILITY_IAM"] and a template containing a versioned AWS::S3::Bucket, an AWS::IAM::Role trusted by elasticfilesystem.amazonaws.com, and:
     "FS": {"Type": "AWS::S3Files::FileSystem", "Properties": {"Bucket": {"Fn::GetAtt": ["Bucket", "Arn"]}, "RoleArn": {"Fn::GetAtt": ["Role", "Arn"]}}}
   -> observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::S3Files::FileSystem]" instead of a StackId.
3. Repeat with an additional resource of type AWS::S3Files::AccessPoint, AWS::S3Files::FileSystemPolicy, or AWS::S3Files::MountTarget -> the same ValidationError, with that type added to the bracketed list.
4. Replay the checks in trace_ids to confirm the full round-trips (stack CREATE_COMPLETE, Outputs, ListFileSystems/GetFileSystem agreement, tag propagation, the versioning-Suspended rollback reason, and delete-removes-the-resource) once the types are registered.
