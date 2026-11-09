Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue__iam__kinesisanalyticsv2__logs.deploy-kda-studio-application-and-logging-option-round-trip cloudformation__glue__iam__kinesisanalyticsv2__logs.deploy-kda-streaming-mode-on-zeppelin-runtime-rejected
Issue-Key: cloudformation__glue__iam__kinesisanalyticsv2__logs|CreateStack|ValidationError
Issue-Key: cloudformation__glue__iam__kinesisanalyticsv2__logs.deploy-kda-streaming-mode-on-zeppelin-runtime-rejected|run#14|AssertionError|assert [(a["ApplicationName"], a["ApplicationMode"]) for a in listed] == [
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# CloudFormation does not know AWS::KinesisAnalyticsV2::ApplicationCloudWatchLoggingOption, and a deployed KDA application is not listed by ListApplications (2 causes)

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 2: a modelled CloudFormation resource type is unrecognized.
AWS: CreateStack accepts a template declaring AWS::KinesisAnalyticsV2::ApplicationCloudWatchLoggingOption and creates the logging option; the recorded run deploys that template to CREATE_COMPLETE and reads the logging option back.
Emulator: CreateStack is refused with HTTP 400 'ValidationError' and message 'Template format error: Unrecognized resource types: [AWS::KinesisAnalyticsV2::ApplicationCloudWatchLoggingOption]'. The fixed text is 'Template format error: Unrecognized resource types: [...]'; the bracketed list is taken from the template.
Why: no stack that wires a KDA application's CloudWatch logging can be deployed, so the whole logging round trip is unreachable.
Fix: the CloudFormation resource-type registry and the handler for AWS::KinesisAnalyticsV2::ApplicationCloudWatchLoggingOption.
Traces: cloudformation__glue__iam__kinesisanalyticsv2__logs.deploy-kda-studio-application-and-logging-option-round-trip.

Cause 2 of 2: an AWS::KinesisAnalyticsV2::Application that the stack reports as created does not exist in the service.
AWS: once the stack reaches CREATE_COMPLETE, kinesisanalyticsv2 ListApplications lists the application named by the resource's Ref, and a streaming-mode property invalid for the Zeppelin runtime is refused by the handler.
Emulator: ListApplications returns no entries for the Ref the stack published — 'ListApplications entries [] for Ref <application name>' — so the stack reports a resource the service does not have.
Why: a client that deploys a KDA application by template and then addresses it through the service API gets ResourceNotFound for a resource CloudFormation says exists.
Fix: the AWS::KinesisAnalyticsV2::Application resource handler in the CloudFormation crate (it must create the application in the kinesisanalyticsv2 state, not only record a stack resource).
Traces: cloudformation__glue__iam__kinesisanalyticsv2__logs.deploy-kda-streaming-mode-on-zeppelin-runtime-rejected.

## Evidence
Cause 1 — expected: CREATE_COMPLETE. Actual: 'botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::KinesisAnalyticsV2::ApplicationCloudWatchLoggingOption]'.
Cause 2 — expected: exactly one application listed for the stack's Ref. Actual: 'AssertionError: ListApplications entries [] for Ref 'sim2real-kdabadmode-ok-<id>''.

## Steps to Reproduce
Replay cloudformation__glue__iam__kinesisanalyticsv2__logs.deploy-kda-studio-application-and-logging-option-round-trip (CreateStack with the logging-option resource) and .deploy-kda-streaming-mode-on-zeppelin-runtime-rejected (deploy a control application, then ListApplications for the published Ref).
