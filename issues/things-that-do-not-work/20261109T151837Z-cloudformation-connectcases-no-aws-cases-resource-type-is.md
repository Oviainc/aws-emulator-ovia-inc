Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__connectcases.deploy-cfn-cases-domain-field-caserule-layout-provisions-live-resources
Issue-Key: cloudformation__connectcases|CreateStack|ValidationError
Issue-Commit: 45bd6d867b1a7177de267d075c8182289448ffad

# cloudformation__connectcases: no AWS::Cases::* resource type is registered — CreateStack refuses every Cases template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `45bd6d867b1a7177de267d075c8182289448ffad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause (1 of 1) in this service pairing.

Cause 1 of 1: the CloudFormation template parser knows no AWS::Cases::* resource type, so a stack that declares a Cases domain, field, case rule or layout is rejected at CreateStack and no ConnectCases estate can ever be deployed by CloudFormation.

AWS: CreateStack accepts a template whose Resources declare AWS::Cases::Domain, AWS::Cases::Field, AWS::Cases::CaseRule and AWS::Cases::Layout (all four are public CloudFormation registry types) and answers 200 with a StackId. Smithy (com-amazonaws-cloudformation-cloudformation): CreateStack input TemplateBody @length min 1; modeled errors are AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException only — nothing models a refusal for a resource type the service knows, and "Template format error: Unrecognized resource types: [...]" is the generic ValidationError AWS emits ONLY for types genuinely absent from the registry. The stack then reaches CREATE_COMPLETE and every resource really exists in ConnectCases: the domain is returned by ListDomains/GetDomain with domainStatus Active, the field by ListFields/BatchGetField with type Text and namespace Custom, the case rule by ListCaseRules/BatchGetCaseRule with the declared rule body, the layout by ListLayouts/GetLayout with the declared content; Ref of each resource is that resource's own ARN and GetAtt exposes DomainId/DomainArn/FieldId/Namespace/CaseRuleId/LayoutId. DeleteStack emits a DELETE_COMPLETE event per resource and the ConnectCases reads then fail ResourceNotFoundException. (Registry membership itself is recorded behavior only — the Smithy model does not enumerate resource types.)

Emulator: CreateStack is refused with HTTP 400, error code ValidationError and the message "Template format error: Unrecognized resource types: [AWS::Cases::Domain, AWS::Cases::Field, AWS::Cases::CaseRule, AWS::Cases::Layout]". The fixed text of that message is "Template format error: Unrecognized resource types: [" … "]"; its variable part is the list of the types the template declared, taken from the request — so the same refusal appears for any subset of the Cases types. The call never creates a stack, so nothing downstream (stack status, Outputs, Ref/GetAtt, the ConnectCases read-backs, stack deletion) is exercised.

Why: a client that deploys its Cases estate as infrastructure-as-code (CloudFormation, CDK or SAM) cannot deploy at all against the emulator: the template is rejected before any resource is attempted, so provisioning, Ref/GetAtt wiring into other resources, and stack-deletion cleanup of Cases resources are all unavailable, and the failure is reported as the customer's template being malformed rather than as missing emulator coverage.

Fix: the CloudFormation crate's template resource-type registry (the table consulted when validating Resources[*].Type) must register AWS::Cases::Domain, AWS::Cases::Field, AWS::Cases::CaseRule and AWS::Cases::Layout, and each needs a resource handler that creates/reads/deletes through the connectcases service state and reports the primary identifier (the resource ARN) as Ref plus the modeled GetAtt attributes.

Not a cause of this issue: the other failing check under this heading, cloudformation__connectcases.deploy-cfn-cases-domain-name-conflict-rolls-back-stack, never reaches CreateStack — it dies earlier on connectcases TagResource answering 501 NotImplemented, which is already filed as 20261022T144510Z-connectcases-tagresource-stub.md.

Traces: cloudformation__connectcases.deploy-cfn-cases-domain-field-caserule-layout-provisions-live-resources.

## Evidence
Check cloudformation__connectcases.deploy-cfn-cases-domain-field-caserule-layout-provisions-live-resources.

Expected (AWS): CreateStack(TemplateBody=<template declaring Domain: AWS::Cases::Domain, Field: AWS::Cases::Field, CaseRule: AWS::Cases::CaseRule, Layout: AWS::Cases::Layout>, Parameters=[DomainName, FieldName, RuleName, LayoutName], Tags=[{sim2real}]) -> HTTP 200 {"StackId": "arn:<partition>:cloudformation:<region>:<account>:stack/<name>/<uuid>"}; DescribeStacks then reports StackStatus CREATE_COMPLETE and Outputs DomainId/DomainArn/DomainRef/FieldId/FieldRef/FieldNamespace/CaseRuleId/CaseRuleRef/LayoutId/LayoutRef, each resolving against ConnectCases.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Cases::Domain, AWS::Cases::Field, AWS::Cases::CaseRule, AWS::Cases::Layout]  (HTTP 400, error code ValidationError).

Fixed text in that message: "Template format error: Unrecognized resource types: [<types>]"; <types> is the set of types the submitted template declared (request-derived), so it is not specific to this run's generated names.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cfn = client('cloudformation'); submit a template:
   Resources:
     Domain: {Type: AWS::Cases::Domain, Properties: {Name: !Ref DomainName}}
     Field:  {Type: AWS::Cases::Field,  Properties: {DomainId: !GetAtt Domain.DomainId, Name: !Ref FieldName, Type: Text}}
     CaseRule: {Type: AWS::Cases::CaseRule, Properties: {DomainId: !GetAtt Domain.DomainId, Name: !Ref RuleName, Rule: {...}}}
     Layout: {Type: AWS::Cases::Layout, Properties: {DomainId: !GetAtt Domain.DomainId, Name: !Ref LayoutName, Content: {...}}}
   cfn.create_stack(StackName=<unique>, TemplateBody=<template>, Parameters=[...], Tags=[{'Key':'sim2real','Value':<unique>}]).
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Cases::Domain, AWS::Cases::Field, AWS::Cases::CaseRule, AWS::Cases::Layout]" instead of a 200 StackId followed by CREATE_COMPLETE.
4. Re-replay check cloudformation__connectcases.deploy-cfn-cases-domain-field-caserule-layout-provisions-live-resources to confirm the fix.
