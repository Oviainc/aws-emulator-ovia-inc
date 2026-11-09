Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__mediapackagev2.deploy-mediapackagev2-channel-and-origin-endpoint-stack-round-trip-and-delete cloudformation__mediapackagev2.deploy-mediapackagev2-channel-invalid-input-type-rejected-and-rolled-back
Issue-Key: cloudformation__mediapackagev2|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__mediapackagev2: CloudFormation does not register any AWS::MediaPackageV2::* resource type

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::MediaPackageV2::ChannelGroup, Channel, ChannelPolicy, OriginEndpoint and OriginEndpointPolicy are not recognized resource types.
AWS: all five are public CloudFormation resource types, so CreateStack accepts templates declaring them and provisions the channel group and its children; an unsupported channel InputType then fails the resource and rolls the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [<the bracketed list of declared AWS::MediaPackageV2::* types>]`.
Why: no MediaPackage v2 estate can be deployed through CloudFormation, so the service is unreachable from the IaC path and the InputType refusal can never be observed.
Fix: register the AWS::MediaPackageV2::* types in the CloudFormation resource-type registry and add their resource handlers.
Traces: the two failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MediaPackageV2::ChannelGroup, AWS::MediaPackageV2::Channel, AWS::MediaPackageV2::ChannelPolicy, AWS::MediaPackageV2::OriginEndpoint, AWS::MediaPackageV2::OriginEndpointPolicy]; and the two-type list for the InputType check. Expected CreateStack accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring AWS::MediaPackageV2::ChannelGroup and AWS::MediaPackageV2::Channel.
3. Observe the ValidationError.
