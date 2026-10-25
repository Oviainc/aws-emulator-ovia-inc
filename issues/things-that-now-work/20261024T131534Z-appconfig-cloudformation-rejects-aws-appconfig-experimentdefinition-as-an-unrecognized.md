Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.deploy-appconfig-experiment-via-cloudformation
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|CreateStack|ValidationError
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# CloudFormation rejects AWS::AppConfig::ExperimentDefinition as an unrecognized resource type, so no AppConfig experiment stack can be created

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause, grounded in a replayed behavioral check.

AWS: AppConfig resources are provisionable through CloudFormation (appconfig's model carries the cloudFormationName 'AppConfig'), and a template declaring AWS::AppConfig::Application, ::Environment, ::ConfigurationProfile, ::HostedConfigurationVersion and ::ExperimentDefinition is ACCEPTED by CreateStack: the stack proceeds to CREATE_IN_PROGRESS and on to CREATE_COMPLETE, after which DescribeStacks Outputs resolve Ref/Fn::GetAtt for those resources. A template-format ValidationError naming an unrecognized resource type is only produced for types CloudFormation does not support (recorded behavior only; the Smithy appconfig model does not describe CloudFormation support beyond the cloudFormationName trait).
Emulator: CreateStack is refused outright with HTTP 400 code 'ValidationError', message 'Template format error: Unrecognized resource types: [AWS::AppConfig::ExperimentDefinition]' — the AppConfig experiment-definition resource type is not registered with the emulator's CloudFormation template validator, so the stack is never created and the check cannot reach any of its later assertions.
Why: every infrastructure-as-code path that provisions an AppConfig experiment is unusable: the stack is rejected at submit time, so a client cannot create, update or delete AppConfig experiment estates through CloudFormation at all. This is strictly earlier than (and different from) the already-filed stack-outputs defect: the stack no longer reaches CREATE_COMPLETE.
Fix: the CloudFormation resource-type registry/template validator must recognize AWS::AppConfig::ExperimentDefinition (alongside the other AWS::AppConfig::* types) and route it to a provider that creates the resource through appconfig's own CreateExperimentDefinition path.
Related but NOT covering: the open issue 20261023T032202Z-appconfig-getconfiguration-and-getexperimentdefinition-answer-501-and-cloudformation.md lists this check in its trace ids for a DIFFERENT, later-stage cause (its Cause 4: DescribeStacks Outputs entries missing OutputValue after CREATE_COMPLETE, a KeyError). The current replay never gets that far — CreateStack itself refuses the template — so that issue's cause does not describe or cover this refusal, and the replay report did not mark this check as owned by it.
Traces: appconfig.deploy-appconfig-experiment-via-cloudformation.

## Evidence
Behavioral replay (simparity-behavioral-ga-prod-v1), check appconfig.deploy-appconfig-experiment-via-cloudformation [error]:

botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AppConfig::ExperimentDefinition]

The template declares the AppConfig application, environment, configuration profile, hosted configuration version and experiment definition, with Outputs mixing Ref and Fn::GetAtt (ExperimentDefinition.Id, HostedConfigurationVersion.VersionNumber). Expected: CreateStack accepted (no ValidationError), stack reaching CREATE_COMPLETE. Actual: HTTP 400 ValidationError at submit time; nothing is created. The template's parameter values and generated stack name are the run's values and are examples; the requirement is that the resource type is recognized.

## Steps to Reproduce
cloudformation.create_stack(StackName=<any>, TemplateBody=<template declaring AWS::AppConfig::Application, AWS::AppConfig::Environment, AWS::AppConfig::ConfigurationProfile, AWS::AppConfig::HostedConfigurationVersion and AWS::AppConfig::ExperimentDefinition>, Parameters=[...]) against the emulator. The call is refused with 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::AppConfig::ExperimentDefinition]'. Against AWS the stack is accepted and reaches CREATE_COMPLETE, and get_waiter('stack_create_complete') returns. Re-replay appconfig.deploy-appconfig-experiment-via-cloudformation to confirm.
