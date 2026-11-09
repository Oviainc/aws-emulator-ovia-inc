Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ivs__mediatailor.deploy-ivs-ad-configuration-mediatailor-playback-configuration-round-trip-and-delete cloudformation__ivs__mediatailor.deploy-ivs-ad-configuration-wrong-insertion-mode-rejected-with-positive-control
Issue-Key: cloudformation__ivs__mediatailor|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation__ivs__mediatailor: CreateStack refuses AWS::IVS::AdConfiguration and AWS::MediaTailor::PlaybackConfiguration as unrecognized resource types, so the IVS server-side-ad stack cannot be deployed

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause behind both surviving cloudformation__ivs__mediatailor failures.

Cause 1 of 1: the CloudFormation resource-type registry knows neither AWS::IVS::AdConfiguration nor AWS::MediaTailor::PlaybackConfiguration, so CreateStack refuses the template outright and no stack is ever minted.

AWS: both resource types are supported by CloudFormation and provision through control planes this emulator models — Smithy com-amazonaws-ivs-amazoninteractivevideoservice binds CreateAdConfiguration / GetAdConfiguration / UpdateAdConfiguration / DeleteAdConfiguration / ListAdConfigurations, and com-amazonaws-mediatailor-mediatailor binds PutPlaybackConfiguration / GetPlaybackConfiguration / DeletePlaybackConfiguration. A template that wires an IVS ad configuration to a MediaTailor playback configuration is accepted and reaches CREATE_COMPLETE, and the resources are then readable through the IVS and MediaTailor APIs (recorded behavior only for the property mapping). For the negative check, AWS recognizes the type and fails the *resource*: an unacceptable ad insertion mode makes the stack reach CREATE_FAILED / ROLLBACK_COMPLETE carrying the IVS service's own validation refusal as that resource's ResourceStatusReason, while the positive control in the same check creates successfully — neither outcome is a template-format error.

Emulator: CreateStack answers error code ValidationError with the message `Template format error: Unrecognized resource types: [AWS::MediaTailor::PlaybackConfiguration, AWS::IVS::AdConfiguration]`. The fixed text is `Template format error: Unrecognized resource types: `; the bracketed list is the set of unknown types taken from the submitted template. Because the call raises, the stack id, the stack status polling, the Outputs, the IVS/MediaTailor read-back and the delete are all unreachable, and the negative check cannot distinguish its refusal branch from its positive control.

Why: a client cannot deploy the IVS server-side-ad-insertion pattern (IVS AdConfiguration + MediaTailor PlaybackConfiguration) as infrastructure as code at all; CreateStack reports the template as malformed rather than the provider as missing. A client testing invalid-input rollback additionally gets the wrong failure class and no per-resource ResourceStatusReason, and its positive control fails for the same reason as its negative case, so the test can no longer tell a refusal from an outage.

Fix: the CloudFormation resource-type registry and its provisioners — register AWS::IVS::AdConfiguration and AWS::MediaTailor::PlaybackConfiguration and wire each to its service's create/read/update/delete operation, publishing the resource ARN as the physical resource id so Ref/GetAtt resolve, and letting the service's own validation produce a resource-level CREATE_FAILED instead of a template-format refusal.

Traces: cloudformation__ivs__mediatailor.deploy-ivs-ad-configuration-mediatailor-playback-configuration-round-trip-and-delete, .deploy-ivs-ad-configuration-wrong-insertion-mode-rejected-with-positive-control.

Note: no open issue covers these types. 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref reports the same class of registry gap for AWS::SSM::MaintenanceWindowTarget only.

## Evidence
From the behavioral replay, both checks [error] on their first call, CreateStack:

- deploy-ivs-ad-configuration-mediatailor-playback-configuration-round-trip-and-delete: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MediaTailor::PlaybackConfiguration, AWS::IVS::AdConfiguration]
- deploy-ivs-ad-configuration-wrong-insertion-mode-rejected-with-positive-control: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MediaTailor::PlaybackConfiguration, AWS::IVS::AdConfiguration]

Expected: CreateStack returns a StackId; the round-trip check then waits for CREATE_COMPLETE, reads the ad configuration and playback configuration back and deletes the stack to DELETE_COMPLETE, and the negative check sees its positive control create while its wrong-insertion-mode variant ends CREATE_FAILED/ROLLBACK_COMPLETE with the IVS validation message as the failing resource's ResourceStatusReason.
Actual: ValidationError at CreateStack in both, no stack exists, nothing downstream is exercised.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the two checks in trace_ids.
3. Minimal repro: cloudformation.create_stack(StackName=..., TemplateBody=<template whose Resources contain {"Type": "AWS::IVS::AdConfiguration", ...} and {"Type": "AWS::MediaTailor::PlaybackConfiguration", ...}>) and observe the ValidationError "Template format error: Unrecognized resource types: [AWS::MediaTailor::PlaybackConfiguration, AWS::IVS::AdConfiguration]" instead of a StackId.
