Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__cloudformation__secretsmanager.deploy-api-key-credential-provider-cfn-secret-and-tombstones
Issue-Key: bedrock-agentcore-control__cloudformation__secretsmanager|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# bedrock-agentcore-control__cloudformation__secretsmanager: AWS::BedrockAgentCore::ApiKeyCredentialProvider is not a registered resource type, so its Secrets Manager secret is never minted

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::BedrockAgentCore::ApiKeyCredentialProvider is missing from the CloudFormation template resource-type registry.

AWS: recorded behavior only — CreateStack accepts a template declaring AWS::BedrockAgentCore::ApiKeyCredentialProvider, provisions the provider, and the provider's API key lands in a Secrets Manager secret that the check then reads; deleting the stack tombstones both.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::BedrockAgentCore::ApiKeyCredentialProvider]`. The provider is never created, so no secret is minted and the cross-service secret/tombstone contract is never exercised.
Why: a client cannot provision an AgentCore API-key credential provider from a template at all, and the Secrets Manager secret its consumers resolve by name does not exist.
Fix: register AWS::BedrockAgentCore::ApiKeyCredentialProvider in the CloudFormation template resource-type registry and add its resource handler, wiring creation through the bedrock-agentcore-control crate and the secret it writes through secretsmanager.
Traces: bedrock-agentcore-control__cloudformation__secretsmanager.deploy-api-key-credential-provider-cfn-secret-and-tombstones

## Evidence
Expected: CreateStack 200, stack CREATE_COMPLETE, the provider's secret readable from Secrets Manager, both tombstoned on stack delete.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::BedrockAgentCore::ApiKeyCredentialProvider]

## Steps to Reproduce
1. CreateStack with a template declaring AWS::BedrockAgentCore::ApiKeyCredentialProvider.
2. Observe the 400 ValidationError "Unrecognized resource types" instead of a StackId.
3. Re-replay bedrock-agentcore-control__cloudformation__secretsmanager.deploy-api-key-credential-provider-cfn-secret-and-tombstones.
