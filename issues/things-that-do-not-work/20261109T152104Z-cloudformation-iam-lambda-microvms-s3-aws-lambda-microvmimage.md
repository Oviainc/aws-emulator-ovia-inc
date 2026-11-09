Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda-microvms__s3.deploy-lambda-microvm-image-round-trip-and-delete cloudformation__iam__lambda-microvms__s3.deploy-lambda-microvm-image-invalid-architecture-rejected-and-rolled-back
Issue-Key: cloudformation__iam__lambda-microvms__s3|CreateStack|ValidationError
Issue-Commit: 8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6

# cloudformation__iam__lambda-microvms__s3: AWS::Lambda::MicrovmImage is not a registered resource type — CreateStack refuses every MicroVM-image template

> _Note: this issue was identified against the code base at commit `8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause, grounded in 2 failing behavioral checks of this suite.

=== Cause 1 of 1: CloudFormation's resource-type registry does not know AWS::Lambda::MicrovmImage, so template validation refuses every stack that declares one ===
Failing checks: cloudformation__iam__lambda-microvms__s3.deploy-lambda-microvm-image-round-trip-and-delete, cloudformation__iam__lambda-microvms__s3.deploy-lambda-microvm-image-invalid-architecture-rejected-and-rolled-back

AWS: CloudFormation supports AWS::Lambda::MicrovmImage as a registered resource type, so CreateStack with a template declaring one returns 200 with a StackId and the stack proceeds to CREATE_COMPLETE. The resource's handler is the lambda-microvms control plane (Smithy, services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json): on Create it calls CreateMicrovmImage with the template's resolved Name, BaseImageArn, BaseImageVersion, BuildRoleArn, Description, CodeArtifact, Logging, EgressNetworkConnectors, CpuConfigurations (Architecture member targets the Architecture enum, values ARM_64 | X86_64), Resources, AdditionalOsCapabilities, Hooks, EnvironmentVariables and Tags, and on Delete it calls DeleteMicrovmImage. Recorded behavior shows: Ref of the resource is the image ARN arn:<partition>:lambda:<region>:<account>:microvm-image:<Name> — the same value ListMicrovmImages reports as imageArn for the declared name — Fn::GetAtt "State" is the image state (CREATED once its build settles) and Fn::GetAtt "LatestActiveImageVersion" is the image's active version; GetMicrovmImageVersion on that version reads back every templated setting (baseImageArn, baseImageVersion resolved from the declared major to that major's current minor, buildRoleArn, description, codeArtifact, cpuConfigurations, resources, environmentVariables) with state SUCCESSFUL and status ACTIVE, ListMicrovmImageBuilds reports buildState SUCCESSFUL, and DeleteStack removes both the image (ListMicrovmImages no longer lists the name) and the IAM build role (GetRole then answers NoSuchEntity "The role with name <RoleName> cannot be found."). When CpuConfigurations[0].Architecture carries a value outside the enum, CloudFormation refuses it in early property validation rather than at the service call: the stack ends ROLLBACK_COMPLETE and carries one PROPERTY_VALIDATION FAIL row on the resource whose reason is "<REDACTED> is not a valid enum value. Supported values: [ARM_64, X86_64]" at property path /Resources/<LogicalId>/Properties/CpuConfigurations/0/Architecture (the offending value is redacted because CpuConfigurations is write-only; the fixed text is " is not a valid enum value. Supported values: [ARM_64, X86_64]", the variable parts being the logical id and the property path), and nothing is created — no image under the declared name and no build role.

Emulator: CreateStack is refused outright at submit time with HTTP 400, Error.Code=ValidationError and Error.Message="Template format error: Unrecognized resource types: [AWS::Lambda::MicrovmImage]" (fixed text "Template format error: Unrecognized resource types: [" + the unknown type names + "]"). No StackId is returned, no stack exists, so neither the happy-path round trip nor the invalid-architecture rollback path is reachable at all. Both checks die on their first CreateStack.

Why: a client cannot express a Lambda MicroVM image in infrastructure-as-code. Every CDK/CloudFormation template that builds a MicroVM image fails at submit time with a template-format error, which a client reads as "my template is malformed" rather than "this type is unsupported". Stacks that pair the image with its IAM build role and S3 build context cannot be deployed, updated, rolled back or deleted, and the enum-validation rejection contract (ROLLBACK_COMPLETE plus a redacted PROPERTY_VALIDATION row and nothing created) is never exercised.

Fix: CloudFormation's resource-type registry and template validation — the same place the types it does accept are registered — must register AWS::Lambda::MicrovmImage, together with a resource provider for it that maps Create to lambda-microvms CreateMicrovmImage and Delete to DeleteMicrovmImage, reports the image ARN as the physical resource id so Ref yields the ARN, exposes State and LatestActiveImageVersion as readable attributes for Fn::GetAtt, and validates CpuConfigurations[*].Architecture against the modeled Architecture enum in property validation so an out-of-enum value produces the PROPERTY_VALIDATION FAIL row and the stack rollback instead of being passed to the service.

Traces: cloudformation__iam__lambda-microvms__s3.deploy-lambda-microvm-image-round-trip-and-delete (happy-path: CREATE_COMPLETE, Ref=image ARN, version settings read-back, builds SUCCESSFUL, DeleteStack removes image and role) and cloudformation__iam__lambda-microvms__s3.deploy-lambda-microvm-image-invalid-architecture-rejected-and-rolled-back (positive control with ARM_64 plus the ARM64 rejection path). Both fail identically on CreateStack. Related open issues describe the same class for other services (e.g. 20261109T124403Z-cloudformation-s3vectors-aws-s3vectors-vectorbucketpolicy-is-not-a.md, 20261109T134927Z-cloudformation-sagemaker-stacks-createstack-refuses-15-aws-sagemaker.md); none registers AWS::Lambda::MicrovmImage, so neither covers this.

## Evidence
Both failing checks error identically on their first CreateStack:

  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Lambda::MicrovmImage]

Template submitted (abridged, both checks share it):
  Parameters: {Bucket: String}
  Resources:
    BuildRole: AWS::IAM::Role (accepted type) — s3:GetObject on arn:${AWS::Partition}:s3:::${Bucket}/app.zip plus logs:* 
    Image:     AWS::Lambda::MicrovmImage (REFUSED type) — Properties {Name, BaseImageArn: Fn::Sub arn:${AWS::Partition}:lambda:${AWS::Region}:aws:<base image>, BaseImageVersion, BuildRoleArn: Fn::GetAtt [BuildRole, Arn], Description, CodeArtifact {Uri: s3://${Bucket}/app.zip}, Logging {Disabled: true}, EgressNetworkConnectors: [], CpuConfigurations: [{Architecture: ARM_64|ARM64}], Resources: [{MinimumMemoryInMiB: 512}], AdditionalOsCapabilities: [], Hooks: {}, EnvironmentVariables, Tags}
  Outputs: ImageArn = Ref Image; ImageState = Fn::GetAtt [Image, State]; ImageLatestActiveVersion = Fn::GetAtt [Image, LatestActiveImageVersion]

Expected (AWS, recorded):
  - CreateStack returns a StackId; the stack reaches CREATE_COMPLETE.
  - Ref Image == arn:<partition>:lambda:<region>:<account>:microvm-image:<Name>, and ListMicrovmImages(nameFilter=<Name>) lists exactly that ARN with state CREATED.
  - GetMicrovmImage(imageIdentifier=<that ARN>) returns name/state/latestActiveImageVersion matching the stack's ImageState and ImageLatestActiveVersion outputs and the template's tags.
  - GetMicrovmImageVersion on the active version reads back baseImageArn, baseImageVersion (declared major resolved to <major>.<minor>), buildRoleArn (== IAM GetRole Arn of the templated role), description, codeArtifact, cpuConfigurations [{architecture: ARM_64}], resources [{minimumMemoryInMiB: 512}], environmentVariables; (state, status) == (SUCCESSFUL, ACTIVE); ListMicrovmImageBuilds buildState all SUCCESSFUL.
  - DeleteStack: the name is no longer listed by ListMicrovmImages, and IAM GetRole answers NoSuchEntity "The role with name <RoleName> cannot be found."
  - With Architecture "ARM64": stack ends ROLLBACK_COMPLETE; the single FAIL validation row is ("Image", "PROPERTY_VALIDATION", "FAILED", "FAIL", "<REDACTED> is not a valid enum value. Supported values: [ARM_64, X86_64]", "/Resources/Image/Properties/CpuConfigurations/0/Architecture"); no image under the declared name and GetRole on the templated role answers NoSuchEntity.

Actual (emulator): HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Lambda::MicrovmImage]". No stack is created, so none of the above is observable.

Model grounding: smithy_query(query="shape", shape="Architecture", model_path="services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json") — the enum the PROPERTY_VALIDATION message lists; CreateMicrovmImage / DeleteMicrovmImage / GetMicrovmImageVersion / ListMicrovmImageBuilds are the service calls the resource handler must make.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. s3.CreateBucket(<bucket>) and s3.PutObject(Bucket=<bucket>, Key="app.zip", Body=<zip with app.js + Dockerfile>).
3. cloudformation.CreateStack(StackName=<name>, Capabilities=["CAPABILITY_NAMED_IAM"], Parameters=[{"ParameterKey":"Bucket","ParameterValue":<bucket>}], TemplateBody=the template above with Resources.BuildRole (AWS::IAM::Role) and Resources.Image (AWS::Lambda::MicrovmImage, CpuConfigurations [{"Architecture":"ARM_64"}])).
   Emulator: HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Lambda::MicrovmImage]".
   AWS:      a StackId, then CREATE_COMPLETE with Ref Image = arn:<partition>:lambda:<region>:<account>:microvm-image:<Name>.
4. Repeat step 3 with CpuConfigurations [{"Architecture":"ARM64"}] -> same CreateStack refusal, so the expected ROLLBACK_COMPLETE plus the "<REDACTED> is not a valid enum value. Supported values: [ARM_64, X86_64]" PROPERTY_VALIDATION row is unreachable.
5. Replay: cloudformation__iam__lambda-microvms__s3.deploy-lambda-microvm-image-round-trip-and-delete and cloudformation__iam__lambda-microvms__s3.deploy-lambda-microvm-image-invalid-architecture-rejected-and-rolled-back.
