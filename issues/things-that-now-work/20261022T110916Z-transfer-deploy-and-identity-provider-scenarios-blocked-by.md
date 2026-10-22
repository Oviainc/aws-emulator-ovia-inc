Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.deploy-server-cfn-start-stop-behavior transfer.deploy-server-cfn-then-start-stop transfer.deploy-server-cloudformation-then-start-stop-behaves transfer.create-access-duplicate-external-id-rejected transfer.test-identity-provider-invokes-configured-endpoint transfer.webapp-customization-binary-data-plane-round-trip
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.deploy-server-cfn-start-stop-behavior|run#8|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Key: transfer.deploy-server-cfn-then-start-stop|<genexpr>#8|KeyError|server_id = next(o["OutputValue"] for o in outputs if o["OutputKey"] == "ServerId")
Issue-Key: transfer.deploy-server-cloudformation-then-start-stop-behaves|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: api-gateway|CreateRestApi|AmbiguousService
Issue-Key: transfer|DescribeInstance|NotImplemented
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# Transfer deploy and identity-provider scenarios blocked by platform-side defects: CloudFormation stack Outputs carry no OutputValue, apigateway refused AmbiguousService, sso-admin DescribeInstance NotImplemented (3 causes)

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Six transfer behavioral checks never reach a Transfer assertion because a surface they depend on diverges first. All three causes are reported here under the transfer service token because that is the leading token of every grounding check id; the defective component is named per cause.

Cause 1 of 3: CloudFormation stack Outputs are returned without OutputValue.
AWS: DescribeStacks returns each Output as {OutputKey, OutputValue, (Description, ExportName)} once the stack reaches CREATE_COMPLETE; for a template whose Outputs.ServerId is Value: !GetAtt Server.ServerId on an AWS::Transfer::Server, OutputValue is the created server's id (recorded behavior only — OutputValue is a modeled member of the Output shape, and the checks read stack['Outputs'] expecting it present).
Emulator: the Outputs list contains entries with OutputKey but no OutputValue key at all, so every consumer raises KeyError: 'OutputValue'. All three transfer CFN deploy checks die at the same dict comprehension, before any StartServer/StopServer behavior is exercised.
Why: a stack output is the only way a template hands a generated resource id to the caller; an output with no value makes every CFN-driven integration unusable even though the stack reports success. An existing open issue reports the same missing-Outputs symptom for a Lambda stack (20261022T081518Z-lambda-is-unreachable-router-ambiguousservice-across-four-lambda.md); this is the AWS::Transfer::Server !GetAtt path and is not covered by it.
Fix: CloudFormation's output resolution/serialization (the stack Outputs projection in the cloudformation service's DescribeStacks path and the !GetAtt resolver for AWS::Transfer::Server attributes).
Traces: transfer.deploy-server-cfn-start-stop-behavior, transfer.deploy-server-cfn-then-start-stop, transfer.deploy-server-cloudformation-then-start-stop-behaves.

Cause 2 of 3: apigateway requests are refused with the non-AWS AmbiguousService error, so API_GATEWAY identity providers cannot be set up.
AWS: CreateRestApi answers 201 with the api envelope; AmbiguousService is not an AWS error code at all.
Emulator: 'An error occurred (AmbiguousService) when calling the CreateRestApi operation (reached max retries: 3): 2 emulated services all claim requests for apigateway: apigateway, apigateway; the request is rejected rather than served by an arbitrary one' — a 5xx refusal from the router, retried to exhaustion. Both transfer checks that must stand up a real API Gateway endpoint to use as a Transfer identity provider die there.
Why: every Transfer scenario whose IdentityProviderType is API_GATEWAY is unreachable, and the client sees an error code no SDK models. The open issue 20261022T081518Z-apigateway-api-gateway-is-unreachable-the-router-refuses-every.md reports the router cause for apigateway's own checks; it does not carry these transfer replay keys, so they are recorded here.
Fix: the request router's service resolution (two registered services claim the same apigateway signing name/endpoint prefix); no change in the Transfer handlers.
Traces: transfer.create-access-duplicate-external-id-rejected, transfer.test-identity-provider-invokes-configured-endpoint.

Cause 3 of 3: sso-admin DescribeInstance is an unimplemented scaffold stub, blocking Transfer web-app checks.
AWS: DescribeInstance returns the IAM Identity Center instance (InstanceArn, IdentityStoreId, Status, ...); a Transfer web app requires that instance ARN in IdentityProviderDetails.IdentityCenterConfig.
Emulator: 'An error occurred (NotImplemented) when calling the DescribeInstance operation: operation DescribeInstance is scaffolded but not implemented in this emulator'. The check cannot resolve an Identity Center instance and aborts before UpdateWebAppCustomization/DescribeWebAppCustomization are exercised.
Why: the whole Transfer web-app surface depends on an Identity Center instance, so the binary customization round trip cannot be graded at all; and NotImplemented is a code no SDK models.
Fix: implement the DescribeInstance handler in the sso-admin (ssoadmin) service's ops directory.
Traces: transfer.webapp-customization-binary-data-plane-round-trip.

## Evidence
Cause 1: KeyError: 'OutputValue' at `outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}` after deploying a template with Outputs.ServerId = !GetAtt Server.ServerId — three checks, identical failure.
Cause 2: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateRestApi operation (reached max retries: 3): 2 emulated services all claim requests for 'apigateway': apigateway, apigateway.
Cause 3: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeInstance operation: operation DescribeInstance is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. transfer.deploy-server-cfn-start-stop-behavior / transfer.deploy-server-cfn-then-start-stop / transfer.deploy-server-cloudformation-then-start-stop-behaves — CreateStack with a template containing an AWS::Transfer::Server plus Outputs: ServerId: {Value: !GetAtt Server.ServerId}; wait for CREATE_COMPLETE; read DescribeStacks()['Stacks'][0]['Outputs'] and look for OutputValue.
2. transfer.create-access-duplicate-external-id-rejected / transfer.test-identity-provider-invokes-configured-endpoint — call apigateway CreateRestApi (used to build the identity-provider endpoint) against the emulator.
3. transfer.webapp-customization-binary-data-plane-round-trip — call sso-admin ListInstances/DescribeInstance to resolve an Identity Center instance ARN before CreateWebApp.
