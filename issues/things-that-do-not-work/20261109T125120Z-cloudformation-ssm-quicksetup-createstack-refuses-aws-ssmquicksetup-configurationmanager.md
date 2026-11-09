Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm-quicksetup.deploy-ssmquicksetup-configuration-manager-host-mgmt-round-trip-and-delete cloudformation__ssm-quicksetup.deploy-ssmquicksetup-configuration-manager-invalid-name-rejected-before-create
Issue-Key: cloudformation__ssm-quicksetup|CreateStack|ValidationError
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# cloudformation__ssm-quicksetup: CreateStack refuses AWS::SSMQuickSetup::ConfigurationManager as an unrecognized resource type, so no Quick Setup manager can be deployed from a template

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
1 net-new root cause.

Cause 1 of 1: the CloudFormation template-validation resource-type registry does not carry AWS::SSMQuickSetup::ConfigurationManager, so CreateStack refuses any template that declares one before a stack is ever created. Failing check ids: cloudformation__ssm-quicksetup.deploy-ssmquicksetup-configuration-manager-host-mgmt-round-trip-and-delete, cloudformation__ssm-quicksetup.deploy-ssmquicksetup-configuration-manager-invalid-name-rejected-before-create.

AWS: AWS::SSMQuickSetup::ConfigurationManager is a supported CloudFormation resource type (Properties Name, Description, ConfigurationDefinitions[{Type, Parameters, LocalDeploymentAdministrationRoleArn, LocalDeploymentExecutionRoleName}], Tags; Ref returns the manager's ManagerArn and Fn::GetAtt exposes CreatedAt/LastModifiedAt/StatusSummaries). CreateStack accepts such a template and answers 200 with a StackId, then provisions the manager through the ssm-quicksetup control plane. Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model): CreateStack models only AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException — the recorded refusal class here (ValidationError / 400, the service-common template-format error) is reserved for a template AWS genuinely cannot parse or whose types it genuinely does not know, and a registered type is not such a case. The registry is also what makes the per-resource property rules reachable: with the type registered, a Name that breaks the resource's pattern must fail PROPERTY_VALIDATION on /Resources/<logical id>/Properties/Name and roll the stack back to ROLLBACK_COMPLETE, not be refused at CreateStack.

Emulator: CreateStack answers 400 ValidationError with message "Template format error: Unrecognized resource types: [AWS::SSMQuickSetup::ConfigurationManager]". Fixed text of that message: "Template format error: Unrecognized resource types: [" … "]"; the variable part is the list of type names taken from the request's template Resources — here the single entry AWS::SSMQuickSetup::ConfigurationManager. No stack is created, so both checks reach no verdict on anything downstream (Ref/GetAtt outputs, the ssm-quicksetup read-back, deployment through stack sets, the delete, and the invalid-Name PROPERTY_VALIDATION row).

Why: a client cannot provision Quick Setup from infrastructure-as-code at all. Every CDK/CloudFormation template or module that declares a Quick Setup configuration manager fails at create time with a template-format error that tells the author the type does not exist, which on real AWS it does; and because the create never happens, the emulator also cannot exercise the resource's own property validation, so a template with an invalid Name is refused for the wrong reason (CreateStack ValidationError) rather than rolled back with a property-validation failure naming the offending property path.

Fix: CloudFormation's template validation / resource-type registry (the table of recognized AWS::<Service>::<Resource> type names consulted by CreateStack's template parse) must register AWS::SSMQuickSetup::ConfigurationManager, and the resource provider that backs it must create/read/update/delete a manager through the ssm-quicksetup state so Ref yields the ManagerArn, Fn::GetAtt yields CreatedAt, template Tags plus the stack's propagated tags land on the manager, and property validation of Name is applied to the declared value.

## Evidence
Expected (AWS): CreateStack(TemplateBody=<template declaring Resources.ConfigManager of Type AWS::SSMQuickSetup::ConfigurationManager>, Capabilities=[CAPABILITY_NAMED_IAM]) -> 200 with StackId; the stack settles CREATE_COMPLETE, Ref ConfigManager == the ManagerArn ListConfigurationManagers/GetConfigurationManager report for the templated Name, and Fn::GetAtt ConfigManager.CreatedAt is the manager's CreatedAt. With the one Name property set to a value that breaks the resource's pattern, the stack instead settles ROLLBACK_COMPLETE and the resource's validation row is ("ConfigManager", "PROPERTY_VALIDATION", "FAILED", "FAIL", "Property value [<name>] does not match pattern: <pattern>", "/Resources/ConfigManager/Properties/Name").

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SSMQuickSetup::ConfigurationManager]  — raised on the first CreateStack of both checks, so no stack id, no stack events, and no ssm-quicksetup resource ever exist.

## Steps to Reproduce
1. Build and boot the emulator.
2. cfn = client("cloudformation"); submit CreateStack with StackName=<unique>, Capabilities=["CAPABILITY_NAMED_IAM"] and a TemplateBody whose Resources contain an IAM admin role, an exec role, and:
   "ConfigManager": {"Type": "AWS::SSMQuickSetup::ConfigurationManager", "Properties": {"Name": "<=50-char name>", "ConfigurationDefinitions": [{"Type": "AWSQuickSetupType-HostMgmt", "Parameters": {"TargetType": "Tags", "TargetTagKey": "<k>", "TargetTagValue": "<v>", "TargetAccounts": {"Ref": "AWS::AccountId"}, "TargetRegions": {"Ref": "AWS::Region"}}, "LocalDeploymentAdministrationRoleArn": {"Fn::GetAtt": ["AdminRole", "Arn"]}, "LocalDeploymentExecutionRoleName": {"Ref": "ExecRole"}}]}}
   with Outputs ManagerArnRef = {"Ref": "ConfigManager"}.
3. Observe 400 ValidationError "Template format error: Unrecognized resource types: [AWS::SSMQuickSetup::ConfigurationManager]" instead of a StackId.
4. Repeat step 2 with the Name set to "bad!<name>": observe the same CreateStack refusal instead of a stack that settles ROLLBACK_COMPLETE with a PROPERTY_VALIDATION FAILED row on /Resources/ConfigManager/Properties/Name.
