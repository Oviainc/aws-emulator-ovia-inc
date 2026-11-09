Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__lambda-core.deploy-lambda-network-connector-round-trip-and-delete cloudformation__ec2__lambda-core.deploy-lambda-network-connector-invalid-protocol-rejected-and-rolled-back
Issue-Key: cloudformation__ec2__lambda-core|CreateStack|ValidationError
Issue-Commit: e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe

# cloudformation__ec2__lambda-core: AWS::Lambda::NetworkConnector is not a registered resource type — CreateStack refuses every network-connector template with "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `e7dbaf2595c70ec955bc42fbbc1eb5a5cb48abfe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource-type registry does not know AWS::Lambda::NetworkConnector, so CreateStack rejects the whole template at parse/validation time and no stack (and no lambda-core network connector) is ever created. Failing checks: cloudformation__ec2__lambda-core.deploy-lambda-network-connector-round-trip-and-delete, cloudformation__ec2__lambda-core.deploy-lambda-network-connector-invalid-protocol-rejected-and-rolled-back.

AWS: CloudFormation supports AWS::Lambda::NetworkConnector, backed by the lambda-core network-connector API (Smithy: lambdacore CreateNetworkConnector, POST /2026-04-04/network-connectors, 202; input Name (required), Configuration (required, union NetworkConnectorConfiguration with member VpcEgressConfiguration), OperatorRole; output Arn/Name/Id/State with State from enum NetworkConnectorState [PENDING, ACTIVE, INACTIVE, FAILED, DELETING, DELETE_FAILED]). A template declaring the type alongside AWS::EC2::VPC/Subnet/SecurityGroup and AWS::IAM::Role is accepted by CreateStack, provisions the connector, resolves Ref Connector to the connector Arn and Fn::GetAtt Connector.State, and reaches CREATE_COMPLETE; the connector is then readable through lambda-core ListNetworkConnectors/GetNetworkConnector in state ACTIVE with the templated SubnetIds, SecurityGroupIds, NetworkProtocol and AssociatedComputeResourceTypes. When Configuration.VpcEgressConfiguration.NetworkProtocol is outside its enum (Smithy: NetworkProtocol values are exactly [IPv4, DualStack]), CloudFormation accepts the CreateStack call and the stack fails property validation and ends ROLLBACK_COMPLETE, with a FAIL validation row for logical id Connector of type PROPERTY_VALIDATION whose fixed text is "<value> is not a valid enum value. Supported values: [IPv4, DualStack]" (the leading "<value>" is the rejected request value, e.g. IPv6) at property path /Resources/Connector/Properties/Configuration/VpcEgressConfiguration/NetworkProtocol — and no connector of that name exists.

Emulator: CreateStack answers HTTP 400 ValidationError with message "Template format error: Unrecognized resource types: [AWS::Lambda::NetworkConnector]" (fixed text "Template format error: Unrecognized resource types: [" + the unknown type names from the template + "]"). The call raises botocore ClientError before any stack id is returned, for both the valid-IPv4 template and the out-of-enum IPv6 template, so neither the deploy round trip nor the rejection/rollback path can run at all.

Why: a client that provisions Lambda network connectors declaratively (CDK/CFN/SAM) cannot deploy them: the stack is refused outright instead of creating the connector, and the negative case is refused with the wrong error class too — AWS answers CreateStack successfully and surfaces the enum violation as a per-resource PROPERTY_VALIDATION failure plus ROLLBACK_COMPLETE, whereas the emulator surfaces an API-level ValidationError about the template format. Any infrastructure-as-code path to a VPC-egress connector is therefore unusable, and tests asserting rollback semantics see an exception at submit time instead.

Fix: register AWS::Lambda::NetworkConnector in the CloudFormation service's resource-type registry and add its resource handler: create/read/delete against lambda-core CreateNetworkConnector/GetNetworkConnector/DeleteNetworkConnector, Ref -> connector Arn, Fn::GetAtt for State (and the other modeled read-only members), and schema/enum validation of Configuration.VpcEgressConfiguration.NetworkProtocol performed as a PROPERTY_VALIDATION resource failure that rolls the stack back rather than as a template-format refusal.

Traces: see trace_ids — both checks fail on the same CreateStack refusal.

## Evidence
Expected (AWS): CreateStack accepts a template containing {"Connector": {"Type": "AWS::Lambda::NetworkConnector", "Properties": {"Name": ..., "Configuration": {"VpcEgressConfiguration": {"SubnetIds": [...], "SecurityGroupIds": [...], "NetworkProtocol": "IPv4", "AssociatedComputeResourceTypes": ["MicroVm"]}}, "OperatorRole": {"Fn::GetAtt": ["OperatorRole", "Arn"]}}}} and returns a StackId; the stack reaches CREATE_COMPLETE and lambda-core GetNetworkConnector(Identifier=<name>) reads State=ACTIVE with the templated configuration. With NetworkProtocol="IPv6" the stack ends ROLLBACK_COMPLETE with validation row ("Connector", "PROPERTY_VALIDATION", "FAILED", "FAIL", "IPv6 is not a valid enum value. Supported values: [IPv4, DualStack]", "/Resources/Connector/Properties/Configuration/VpcEgressConfiguration/NetworkProtocol") and no connector is listed.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Lambda::NetworkConnector] — raised for both templates, so no StackId, no stack events, no validation rows and no connector.

Model citation: lambdacore Smithy — CreateNetworkConnector (POST /2026-04-04/network-connectors, 202), union NetworkConnectorConfiguration{VpcEgressConfiguration}, enum NetworkProtocol = [IPv4, DualStack], enum NetworkConnectorState = [PENDING, ACTIVE, INACTIVE, FAILED, DELETING, DELETE_FAILED].

## Steps to Reproduce
1. Boot the emulator and create an EC2 VPC/Subnet/SecurityGroup + IAM role template that also declares a resource of Type "AWS::Lambda::NetworkConnector" with Properties Name, Configuration.VpcEgressConfiguration{SubnetIds, SecurityGroupIds, NetworkProtocol: "IPv4", AssociatedComputeResourceTypes: ["MicroVm"]} and OperatorRole = Fn::GetAtt of the role Arn.
2. Call cloudformation CreateStack(StackName=<name>, TemplateBody=<that template>, Capabilities=["CAPABILITY_NAMED_IAM"]).
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Lambda::NetworkConnector]" instead of a StackId.
4. Repeat with NetworkProtocol = "IPv6": expect the call to succeed and the stack to end ROLLBACK_COMPLETE with a PROPERTY_VALIDATION FAIL row naming the NetworkProtocol property path; observe the same template-format ValidationError instead.
