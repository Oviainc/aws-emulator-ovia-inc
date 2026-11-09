Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__connectcases.deploy-cfn-cases-template-case-relateditem-against-live-customer-profile
Issue-Key: cloudformation__connectcases__customer-profiles|CreateStack|ValidationError
Issue-Commit: 45bd6d867b1a7177de267d075c8182289448ffad

# cloudformation__connectcases__customer-profiles: CreateStack refuses AWS::Cases::Domain and AWS::CustomerProfiles::Domain as unrecognized resource types, so a Cases estate backed by a live customer profile cannot be deployed

> _Note: this issue was identified against the code base at commit `45bd6d867b1a7177de267d075c8182289448ffad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause (1 of 1) in this service pairing.

Cause 1 of 1: the CloudFormation template parser knows neither AWS::Cases::Domain nor AWS::CustomerProfiles::Domain, so the first stack of a Cases-over-Customer-Profiles deployment is rejected at CreateStack and nothing in the scenario (profile creation, the Cases template/case/related-item stack, the tombstone and cleanup reads) can run.

AWS: CreateStack accepts a template declaring AWS::Cases::Domain (Properties.Name) alongside AWS::CustomerProfiles::Domain (Properties.DomainName, DefaultExpirationDays) — both are public CloudFormation registry types — and answers 200 with a StackId; the stack reaches CREATE_COMPLETE. Smithy (com-amazonaws-cloudformation-cloudformation): CreateStack input TemplateBody @length min 1; modeled errors are AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException only — no modeled refusal for a resource type the service knows; "Template format error: Unrecognized resource types: [...]" is the generic ValidationError AWS emits only for types absent from the registry. Against AWS the Cases domain is then readable through ConnectCases ListDomains/GetDomain, Ref and GetAtt DomainArn both equal GetDomain's domainArn, and the Customer Profiles domain's Ref is its own DomainName with GetDomain reporting DefaultExpirationDays 30. The follow-on stack in the same scenario declares AWS::Cases::Template, AWS::Cases::Case and AWS::Cases::RelatedItem, whose Ref values are the template/case/related-item ARNs — that second stack is never submitted here because the first CreateStack is refused, so those three types are part of the same registry gap rather than a separately observed divergence. (Registry membership is recorded behavior only — the Smithy model does not enumerate resource types.)

Emulator: CreateStack is refused with HTTP 400, error code ValidationError and the message "Template format error: Unrecognized resource types: [AWS::Cases::Domain, AWS::CustomerProfiles::Domain]". Fixed text: "Template format error: Unrecognized resource types: [" … "]"; the variable part is the list of types the submitted template declared, taken from the request. No stack is created, so no status, Outputs, Ref/GetAtt or cross-service read-back is produced.

Why: a client that provisions Cases and Customer Profiles together as infrastructure-as-code gets its template rejected as malformed; the whole deploy path — cross-service wiring (a case naming a profile ARN minted by the Profiles domain), stack Outputs used as inputs to a second stack, and stack-deletion cleanup — is unavailable against the emulator.

Fix: the CloudFormation crate's template resource-type registry must register AWS::Cases::Domain (and, for the rest of this scenario, AWS::Cases::Template, AWS::Cases::Case, AWS::Cases::RelatedItem) and AWS::CustomerProfiles::Domain, with resource handlers that create/read/delete through the connectcases and customer-profiles service state and report the documented primary identifier as Ref (resource ARN for Cases resources, DomainName for a Customer Profiles domain) plus the modeled GetAtt attributes.

Related open issue, which does NOT cover this check: 20261102T122410Z-customer-profiles-cloudformation-does-not-recognize-the-aws-customerprofiles-domain.md names the AWS::CustomerProfiles::Domain half only; the AWS::Cases::* half, and this check, are not carried by it.

Traces: cloudformation__connectcases.deploy-cfn-cases-template-case-relateditem-against-live-customer-profile (listed by the suite under service heading cloudformation__connectcases__customer-profiles).

## Evidence
Check cloudformation__connectcases.deploy-cfn-cases-template-case-relateditem-against-live-customer-profile (suite heading: cloudformation__connectcases__customer-profiles).

Expected (AWS): CreateStack with the stack-A template
  Resources:
    CasesDomain: {Type: AWS::Cases::Domain, Properties: {Name: !Ref CasesDomainName}}
    ProfilesDomain: {Type: AWS::CustomerProfiles::Domain, Properties: {DomainName: !Ref ProfilesDomainName, DefaultExpirationDays: 30}}
-> HTTP 200 {"StackId": "arn:<partition>:cloudformation:<region>:<account>:stack/<name>/<uuid>"}, then DescribeStacks StackStatus CREATE_COMPLETE with Outputs CasesDomainId/CasesDomainRef/CasesDomainArn/ProfilesDomainRef.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Cases::Domain, AWS::CustomerProfiles::Domain]  (HTTP 400, error code ValidationError).

Fixed text in that message: "Template format error: Unrecognized resource types: [<types>]"; <types> is request-derived (the template's declared types), not a per-run value.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cfn = client('cloudformation'); cfn.create_stack(StackName=<unique>, TemplateBody=<<<
   AWSTemplateFormatVersion: '2010-09-09'
   Parameters: {CasesDomainName: {Type: String}, ProfilesDomainName: {Type: String}}
   Resources:
     CasesDomain: {Type: AWS::Cases::Domain, Properties: {Name: !Ref CasesDomainName}}
     ProfilesDomain: {Type: AWS::CustomerProfiles::Domain, Properties: {DomainName: !Ref ProfilesDomainName, DefaultExpirationDays: 30}}
   Outputs: {CasesDomainId: {Value: !GetAtt CasesDomain.DomainId}, ProfilesDomainRef: {Value: !Ref ProfilesDomain}}
   >>>, Parameters=[CasesDomainName, ProfilesDomainName], Tags=[{'Key':'sim2real','Value':<unique>}]).
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Cases::Domain, AWS::CustomerProfiles::Domain]" instead of a 200 StackId reaching CREATE_COMPLETE.
4. After the registry gap is closed, the same scenario submits a second stack declaring AWS::Cases::Template, AWS::Cases::Case and AWS::Cases::RelatedItem — those types must be registered too for the check to pass.
5. Re-replay check cloudformation__connectcases.deploy-cfn-cases-template-case-relateditem-against-live-customer-profile to confirm the fix.
