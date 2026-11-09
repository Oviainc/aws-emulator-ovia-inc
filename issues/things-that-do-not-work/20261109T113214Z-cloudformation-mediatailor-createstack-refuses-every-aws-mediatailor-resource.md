Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__mediatailor.deploy-mediatailor-channel-channel-policy-program-round-trip-and-delete cloudformation__mediatailor.deploy-mediatailor-channel-invalid-playback-mode-rejected-and-rolled-back cloudformation__mediatailor.deploy-mediatailor-function-round-trip-and-delete cloudformation__mediatailor.deploy-mediatailor-playback-configuration-invalid-name-rejected-and-rolled-back cloudformation__mediatailor.deploy-mediatailor-playback-configuration-prefetch-schedule-round-trip-and-delete cloudformation__mediatailor.deploy-mediatailor-source-location-live-source-vod-source-round-trip-and-delete
Issue-Key: cloudformation__mediatailor|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation__mediatailor: CreateStack refuses every AWS::MediaTailor::* resource type as unrecognized, so no MediaTailor template can be deployed (6 checks)

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause behind all six surviving cloudformation__mediatailor failures.

Cause 1 of 1: the CloudFormation resource-type registry does not know any AWS::MediaTailor::* type, so CreateStack is refused up-front with a template-format ValidationError and the stack is never created.

AWS: these resource types are supported by CloudFormation and provision through MediaTailor's own control plane, all of which this emulator models (Smithy, com-amazonaws-mediatailor-mediatailor: CreateChannel, PutChannelPolicy, CreateProgram, CreateSourceLocation, CreateVodSource, CreateLiveSource, PutPlaybackConfiguration, CreatePrefetchSchedule, PutFunction/GetFunction/ListFunctions/DeleteFunction all exist as bound operations). A template declaring AWS::MediaTailor::Channel, ::ChannelPolicy, ::Program, ::SourceLocation, ::VodSource, ::LiveSource, ::PlaybackConfiguration, ::PrefetchSchedule or ::Function is accepted by CreateStack and reaches CREATE_COMPLETE, and the declared resources are then readable through the MediaTailor API (recorded behavior only for the per-type property mapping). For the two negative checks, AWS accepts the template and fails the *resource*: the stack reaches CREATE_FAILED / ROLLBACK_COMPLETE with the MediaTailor service's own validation refusal carried in the resource's ResourceStatusReason — it is not a template-format error, because the type itself is recognized.

Emulator: CreateStack answers the ValidationError error code with the message `Template format error: Unrecognized resource types: [<types>]`. The fixed text is `Template format error: Unrecognized resource types: ` followed by the bracketed list of the template's unknown types taken from the request; the types observed across the six checks are AWS::MediaTailor::Channel, ::ChannelPolicy, ::Program, ::SourceLocation, ::VodSource, ::LiveSource, ::PlaybackConfiguration, ::PrefetchSchedule and ::Function. No stack is minted, so DescribeStacks/DescribeStackEvents, the Outputs, the round-trip reads and the deletes are all unreachable.

Why: a client cannot deploy MediaTailor infrastructure as code at all. Every CDK/CloudFormation estate that contains a MediaTailor channel, source location, playback configuration, prefetch schedule or function fails at CreateStack with a template-format error, which a client reads as "my template is malformed" rather than "this provider is missing". The two negative checks are additionally mis-shaped: a template-format refusal at CreateStack is indistinguishable from a resource-level validation failure, so a client that tests invalid-input rollback sees the wrong failure class, the wrong status path (no CREATE_FAILED/ROLLBACK_COMPLETE at all) and no per-resource ResourceStatusReason.

Fix: the CloudFormation resource-type registry and its provisioners — register the AWS::MediaTailor::* types listed above and wire each to the corresponding MediaTailor control-plane operation for create/read/update/delete, publishing the MediaTailor ARN/name as the physical resource id so Ref/GetAtt resolve.

Traces: cloudformation__mediatailor.deploy-mediatailor-channel-channel-policy-program-round-trip-and-delete, .deploy-mediatailor-channel-invalid-playback-mode-rejected-and-rolled-back, .deploy-mediatailor-function-round-trip-and-delete, .deploy-mediatailor-playback-configuration-invalid-name-rejected-and-rolled-back, .deploy-mediatailor-playback-configuration-prefetch-schedule-round-trip-and-delete, .deploy-mediatailor-source-location-live-source-vod-source-round-trip-and-delete.

Note: no open issue covers this. The closest, 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref, reports the same class of gap for AWS::SSM::MaintenanceWindowTarget only and does not cover any MediaTailor type.

## Evidence
From the behavioral replay, all six checks [error] on the first call, CreateStack:

- deploy-mediatailor-channel-channel-policy-program-round-trip-and-delete: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MediaTailor::SourceLocation, AWS::MediaTailor::Channel, AWS::MediaTailor::VodSource, AWS::MediaTailor::ChannelPolicy, AWS::MediaTailor::Program]
- deploy-mediatailor-channel-invalid-playback-mode-rejected-and-rolled-back: ... Template format error: Unrecognized resource types: [AWS::MediaTailor::Channel]
- deploy-mediatailor-function-round-trip-and-delete: ... Template format error: Unrecognized resource types: [AWS::MediaTailor::Function]
- deploy-mediatailor-playback-configuration-invalid-name-rejected-and-rolled-back: ... Template format error: Unrecognized resource types: [AWS::MediaTailor::PlaybackConfiguration]
- deploy-mediatailor-playback-configuration-prefetch-schedule-round-trip-and-delete: ... Template format error: Unrecognized resource types: [AWS::MediaTailor::PlaybackConfiguration, AWS::MediaTailor::PrefetchSchedule]
- deploy-mediatailor-source-location-live-source-vod-source-round-trip-and-delete: ... Template format error: Unrecognized resource types: [AWS::MediaTailor::SourceLocation, AWS::MediaTailor::VodSource, AWS::MediaTailor::LiveSource]

Expected: CreateStack returns a StackId; the round-trip checks then wait for CREATE_COMPLETE and read the resources back through MediaTailor, and the two invalid-input checks observe the stack reach CREATE_FAILED/ROLLBACK_COMPLETE with the MediaTailor validation refusal as the failing resource's ResourceStatusReason.
Actual: ValidationError at CreateStack, no stack exists, nothing downstream is exercised.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the checks in trace_ids.
3. Minimal repro: cloudformation.create_stack(StackName=..., TemplateBody=<a template whose Resources contain a single {"Type": "AWS::MediaTailor::Channel", ...}>) and observe the ValidationError "Template format error: Unrecognized resource types: [AWS::MediaTailor::Channel]" instead of a StackId. Repeat for ::ChannelPolicy, ::Program, ::SourceLocation, ::VodSource, ::LiveSource, ::PlaybackConfiguration, ::PrefetchSchedule and ::Function.
