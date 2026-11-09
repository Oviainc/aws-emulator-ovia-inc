Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__route53resolver.deploy-resolver-config-enable-on-create-rejected cloudformation__route53resolver.deploy-resolver-query-logging-config-and-association-round-trip-and-delete cloudformation__route53resolver.deploy-resolver-query-logging-config-nonexistent-destination-rejected cloudformation__route53resolver.deploy-vpc-configs-cfn-reset-on-delete
Issue-Key: cloudformation__route53resolver|CreateStack|ValidationError
Issue-Commit: 4d9e3fb251011fe781d884d5ecb8db5bb1e34e6b

# cloudformation__route53resolver: no AWS::Route53Resolver::* resource type is registered — CreateStack refuses every Resolver/DNS-Firewall template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `4d9e3fb251011fe781d884d5ecb8db5bb1e34e6b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template resource-type registry knows none of the AWS::Route53Resolver::* resource types, so CreateStack refuses the template outright at parse/validation time and no Route 53 Resolver resource can ever be deployed, updated, rolled back or deleted through CloudFormation. Failing check ids: cloudformation__route53resolver.deploy-resolver-config-enable-on-create-rejected, cloudformation__route53resolver.deploy-resolver-query-logging-config-and-association-round-trip-and-delete, cloudformation__route53resolver.deploy-resolver-query-logging-config-nonexistent-destination-rejected, cloudformation__route53resolver.deploy-vpc-configs-cfn-reset-on-delete.

AWS: Smithy (cloudformation): CreateStack takes TemplateBody/TemplateURL and models only AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException as operation errors — a template whose resource types are all registered is ACCEPTED and answers 200 with a StackId; the generic ValidationError is the service-common refusal used for a template CloudFormation cannot parse. The resource-type registry itself is not in the Smithy model, so for the type list this is recorded behavior only: AWS registers AWS::Route53Resolver::ResolverConfig, AWS::Route53Resolver::ResolverQueryLoggingConfig, AWS::Route53Resolver::ResolverQueryLoggingConfigAssociation, AWS::Route53Resolver::FirewallConfig and AWS::Route53Resolver::ResolverDNSSECConfig, so CreateStack on a template declaring any of them returns 200 with a StackId and the stack then proceeds to a terminal status (CREATE_COMPLETE for a valid template; ROLLBACK_COMPLETE with a per-resource PROPERTY_VALIDATION/FAILED row when a property value is out of range, e.g. AutodefinedReverseFlag=ENABLE on create giving the reason "ENABLE is not a valid enum value. Supported values: [DISABLE]" at resource path /Resources/<LogicalId>/Properties/AutodefinedReverseFlag).

Emulator: every one of these four CreateStack calls is refused before any stack exists, with HTTP 400, error code ValidationError and the message "Template format error: Unrecognized resource types: [<type list>]". The fixed text is "Template format error: Unrecognized resource types: [" + "]"; the variable part is the list of unregistered types taken from the submitted template — observed instances were [AWS::Route53Resolver::ResolverConfig], [AWS::Route53Resolver::ResolverQueryLoggingConfig], [AWS::Route53Resolver::ResolverQueryLoggingConfig, AWS::Route53Resolver::ResolverQueryLoggingConfigAssociation] and [AWS::Route53Resolver::FirewallConfig, AWS::Route53Resolver::ResolverConfig, AWS::Route53Resolver::ResolverDNSSECConfig]. Because the whole template is rejected, nothing downstream is exercised: no stack id, no stack events, no Outputs, no Ref/Fn::GetAtt resolution, no Resolver-side state change and no reset-on-delete.

Why: a client (or CDK app) that manages Resolver VPC configuration, DNSSEC configuration, DNS Firewall VPC configuration or query-logging configs and associations as infrastructure-as-code cannot deploy at all — the stack is never created, so there is nothing to wait on, nothing to roll back and nothing to delete. Any code path that depends on CloudFormation accepting the template and then reporting a per-resource validation failure (the documented way to learn that AutodefinedReverseFlag only accepts DISABLE on create, or that a query-logging destination does not exist) is unreachable, and templates that mix Resolver resources with other services fail wholesale rather than partially.

Fix: the CloudFormation resource-type registry and the resource handlers it dispatches to — register the AWS::Route53Resolver::* types listed above (handlers covering create/update/delete plus Ref and Fn::GetAtt attribute resolution, routed to the route53resolver service state) so CreateStack accepts the template, and let per-resource property validation produce the PROPERTY_VALIDATION/FAILED event rows and the ROLLBACK_COMPLETE outcome instead of a whole-template parse refusal.

Traces: cloudformation__route53resolver.deploy-resolver-config-enable-on-create-rejected (AWS::Route53Resolver::ResolverConfig), cloudformation__route53resolver.deploy-resolver-query-logging-config-and-association-round-trip-and-delete (ResolverQueryLoggingConfig + ResolverQueryLoggingConfigAssociation), cloudformation__route53resolver.deploy-resolver-query-logging-config-nonexistent-destination-rejected (ResolverQueryLoggingConfig), cloudformation__route53resolver.deploy-vpc-configs-cfn-reset-on-delete (FirewallConfig + ResolverConfig + ResolverDNSSECConfig). No open issue carries these checks; the many open "unrecognized resource type" issues filed for other services (e.g. 20261109T130344Z-cloudformation-devicefarm-no-aws-devicefarm-resource-type-is.md) describe the same class of gap for different type namespaces and do not cover AWS::Route53Resolver::*.

## Evidence
All four checks fail at the first CreateStack, before a stack id exists.

cloudformation__route53resolver.deploy-resolver-config-enable-on-create-rejected:
  expected: CreateStack accepted (200, StackId); control stack with AutodefinedReverseFlag=DISABLE reaches CREATE_COMPLETE with Outputs Ref=<VpcId> and Fn::GetAtt Id=<resolver config id>, GetResolverConfig(ResourceId=<vpc>) AutodefinedReverse=DISABLED; the ENABLE variant reaches ROLLBACK_COMPLETE with validation row ("ResolverConfig", "PROPERTY_VALIDATION", "FAILED", "FAIL", "ENABLE is not a valid enum value. Supported values: [DISABLE]", "/Resources/ResolverConfig/Properties/AutodefinedReverseFlag") and the VPC's AutodefinedReverse unchanged.
  actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Route53Resolver::ResolverConfig]

cloudformation__route53resolver.deploy-resolver-query-logging-config-and-association-round-trip-and-delete:
  actual: (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::Route53Resolver::ResolverQueryLoggingConfig, AWS::Route53Resolver::ResolverQueryLoggingConfigAssociation]

cloudformation__route53resolver.deploy-resolver-query-logging-config-nonexistent-destination-rejected:
  actual: (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::Route53Resolver::ResolverQueryLoggingConfig]

cloudformation__route53resolver.deploy-vpc-configs-cfn-reset-on-delete:
  actual: (ValidationError) CreateStack: Template format error: Unrecognized resource types: [AWS::Route53Resolver::FirewallConfig, AWS::Route53Resolver::ResolverConfig, AWS::Route53Resolver::ResolverDNSSECConfig]

In every case AWS answers 200 with a StackId for these templates; the ValidationError/"Unrecognized resource types" refusal is emulator-only.

## Steps to Reproduce
1. ec2:CreateVpc a VPC (10.0.0.0/16) and wait for it to become available.
2. cloudformation:CreateStack with TemplateBody:
   AWSTemplateFormatVersion: '2010-09-09'
   Parameters: { VpcId: { Type: String } }
   Resources:
     ResolverConfig:
       Type: AWS::Route53Resolver::ResolverConfig
       Properties: { ResourceId: !Ref VpcId, AutodefinedReverseFlag: DISABLE }
   Outputs: { Ref: { Value: !Ref ResolverConfig }, Id: { Value: !GetAtt ResolverConfig.Id } }
   and Parameters=[{ParameterKey: VpcId, ParameterValue: <vpc>}].
   The call raises ValidationError "Template format error: Unrecognized resource types: [AWS::Route53Resolver::ResolverConfig]" instead of returning a StackId.
3. Repeat with a template declaring AWS::Route53Resolver::ResolverQueryLoggingConfig (+ ...ConfigAssociation), and with one declaring AWS::Route53Resolver::FirewallConfig, ::ResolverConfig and ::ResolverDNSSECConfig — each is refused with the same code and message, listing the types it does not recognize.
4. Replay: run_behavioral_suite and re-run the four checks in trace_ids.
