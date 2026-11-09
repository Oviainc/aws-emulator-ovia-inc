Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__cloudformation__ec2.deploy-ec2-client-vpn-endpoint-family-round-trip-and-delete acm__cloudformation__ec2.deploy-ec2-client-vpn-endpoint-invalid-client-cidr-rejected
Issue-Key: acm__cloudformation__ec2|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# acm__cloudformation__ec2: AWS::EC2::{ClientVpnEndpoint,ClientVpnAuthorizationRule} are not registered resource types

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::EC2::ClientVpnEndpoint and AWS::EC2::ClientVpnAuthorizationRule are missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts both types, provisions the Client VPN endpoint (with its ACM server/client certificates) and its authorization rule through EC2, and reaches CREATE_COMPLETE; an endpoint whose ClientCidrBlock is smaller than the documented minimum (/22) instead rolls the stack back with EC2's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::EC2::ClientVpnEndpoint, AWS::EC2::ClientVpnAuthorizationRule]` (and `[AWS::EC2::ClientVpnEndpoint]` for the single-resource template), so nothing is created and the CIDR-minimum refusal is never reached.
Why: no Client VPN estate can be deployed from a template, and the ACM-certificate-to-Client-VPN wiring is untestable; the user is told these AWS resource types do not exist.
Fix: register both types in the CloudFormation template resource-type registry and add a resource handler per type over the ec2 crate, including the ClientCidrBlock minimum-prefix check on create.
Traces: the 2 checks in trace_ids.

## Evidence
Expected: CreateStack 200 + StackId and CREATE_COMPLETE (ROLLBACK_COMPLETE citing EC2's ClientCidrBlock refusal for the /28 leg).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EC2::ClientVpnEndpoint, AWS::EC2::ClientVpnAuthorizationRule] / [AWS::EC2::ClientVpnEndpoint]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::EC2::ClientVpnEndpoint (optionally with AWS::EC2::ClientVpnAuthorizationRule).
2. Observe the 400 ValidationError "Unrecognized resource types".
3. Re-replay the 2 checks in trace_ids.
