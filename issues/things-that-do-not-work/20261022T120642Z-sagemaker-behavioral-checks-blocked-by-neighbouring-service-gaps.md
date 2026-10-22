Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.attach-cluster-node-volume-cluster-not-found sagemaker.attach-cluster-node-volume-missing-node-not-found sagemaker.create-app-lifecycle-transitions sagemaker.delete-image-with-versions-accepted sagemaker.image-delete-with-active-version-accepted sagemaker.image-version-create-describe-delete sagemaker.model-package-lifecycle-transitions sagemaker.deploy-notebook-instance-cfn-behaves
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateVolume|NotImplemented
Issue-Key: ec2|DescribeVpcs|InvalidParameterValue
Issue-Key: sagemaker|UploadLayerPart|NotImplemented
Issue-Key: sagemaker.deploy-notebook-instance-cfn-behaves|run#18|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# SageMaker behavioral checks blocked by neighbouring-service gaps: EC2 CreateVolume and ECR UploadLayerPart are stubs, DescribeVpcs rejects the isDefault filter, and CloudFormation stack Outputs omit OutputValue

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVolume (bundle also covers DescribeVpcs, UploadLayerPart, DescribeStacks)

## Description
Four root causes that the sagemaker behavioral checks surface but whose defective handlers live in neighbouring service crates. All four are grounded in sagemaker.* checks, so they are filed under service sagemaker; each is its own root cause and its own work item, and this issue bundles FOUR causes that must all be closed before submitting. None of them is named by the stub issues the replay report filed for me (that set includes EC2 CreateVpc, but not CreateVolume, DescribeVpcs, UploadLayerPart or DescribeStacks).

Cause 1 of 4: EC2 CreateVolume is a scaffolded stub.
AWS: recorded behavior only for these checks' usage - EC2 CreateVolume(AvailabilityZone, Size) returns 200 with a VolumeId matching vol-[a-f0-9]{8,17} and the volume becomes 'available'; SageMaker's AttachClusterNodeVolume checks (Smithy: AttachClusterNodeVolume.VolumeId @pattern ^vol-[a-f0-9]{8}(?:[a-f0-9]{9})?$) need a genuine volume id to exercise their precondition.
Emulator: CreateVolume answers error code NotImplemented ("operation CreateVolume is scaffolded but not implemented in this emulator"), so the checks abort before any SageMaker call.
Why: any client that provisions an EBS volume - directly or as a dependency of a HyperPod volume attach - cannot proceed, and gets an unmodeled error code instead of a volume.
Fix: the operation's handler services/service-com-amazonaws-ec2-amazonec2/src/ops/create_volume.rs plus EBS volume state.
Traces: sagemaker.attach-cluster-node-volume-cluster-not-found, sagemaker.attach-cluster-node-volume-missing-node-not-found.

Cause 2 of 4: EC2 DescribeVpcs rejects the documented 'isDefault' filter.
AWS: AWS's EC2 DescribeVpcs accepts Filters with Name='isDefault' (the supported filter names are documented in the operation's Filters prose; the model itself does not enumerate them, so: recorded/documented behavior only). The recorded call returns 200 with the default VPC.
Emulator: returns 400 InvalidParameterValue "The filter 'isDefault' is invalid".
Why: the standard way to discover the default VPC fails, so anything that bootstraps networking (here, a SageMaker Studio domain/app lifecycle) cannot start.
Fix: the filter handling in DescribeVpcs' handler, services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpcs.rs (accept the documented filter names and evaluate isDefault against VPC state).
Traces: sagemaker.create-app-lifecycle-transitions.

Cause 3 of 4: ECR UploadLayerPart is a scaffolded stub, so no image can be pushed.
AWS: recorded behavior only - ECR's push sequence InitiateLayerUpload -> UploadLayerPart -> CompleteLayerUpload -> PutImage returns 200 for each part and yields a pullable image; the SageMaker image/model-package checks push a tiny image to get a real ECR image URI.
Emulator: UploadLayerPart answers error code NotImplemented, aborting the push.
Why: every flow that needs a real container image in the emulated registry (SageMaker Image/ImageVersion, model packages) is unreachable, and a client sees an unmodeled error mid-push.
Fix: the operation's handler services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/upload_layer_part.rs plus layer-upload state.
Traces: sagemaker.delete-image-with-versions-accepted, sagemaker.image-delete-with-active-version-accepted, sagemaker.image-version-create-describe-delete, sagemaker.model-package-lifecycle-transitions.

Cause 4 of 4: CloudFormation stack Outputs entries are returned without OutputValue.
AWS: recorded behavior only - DescribeStacks returns each Outputs member with both OutputKey and OutputValue populated once the stack is CREATE_COMPLETE (the Output shape pairs the key with its resolved value).
Emulator: the Outputs entries carry OutputKey but no OutputValue, so a consumer indexing the pair raises KeyError: 'OutputValue'.
Why: every IaC consumer reads stack outputs to learn the names/ARNs of what it just deployed; with the value missing, a successful deploy is unusable.
Fix: the Outputs serialization/resolution for DescribeStacks in the cloudformation crate (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stacks.rs and the stack-output resolution in its state).
Traces: sagemaker.deploy-notebook-instance-cfn-behaves.

## Evidence
Cause 1: sagemaker.attach-cluster-node-volume-cluster-not-found and ...-missing-node-not-found both end in "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVolume operation: operation CreateVolume is scaffolded but not implemented in this emulator".
Cause 2: sagemaker.create-app-lifecycle-transitions: "botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the DescribeVpcs operation: The filter 'isDefault' is invalid".
Cause 3: sagemaker.delete-image-with-versions-accepted, sagemaker.image-delete-with-active-version-accepted, sagemaker.image-version-create-describe-delete, sagemaker.model-package-lifecycle-transitions: "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UploadLayerPart operation: operation UploadLayerPart is scaffolded but not implemented in this emulator".
Cause 4: sagemaker.deploy-notebook-instance-cfn-behaves: "File ... line 95, in run / o[\"OutputKey\"]: o[\"OutputValue\"] / KeyError: 'OutputValue'" - the DescribeStacks Outputs entry has no OutputValue member.

## Steps to Reproduce
1. ec2.create_volume(AvailabilityZone='<region>a', Size=1) -> expect 200 with a vol-... id; observe error code NotImplemented.
2. ec2.describe_vpcs(Filters=[{'Name':'isDefault','Values':['true']}]) -> expect 200 listing the default VPC; observe 400 InvalidParameterValue "The filter 'isDefault' is invalid".
3. ecr.create_repository, initiate_layer_upload, then upload_layer_part(...) -> expect 200; observe error code NotImplemented.
4. cloudformation.create_stack with a template declaring Outputs, wait for CREATE_COMPLETE, then describe_stacks and read Stacks[0].Outputs[*] -> each entry has OutputKey but no OutputValue.
