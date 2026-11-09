Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__cloudformation.deploy-browser-resources-cfn-behaves bedrock-agentcore-control__cloudformation.deploy-configuration-bundle-round-trip-then-stack-delete bedrock-agentcore-control__cloudformation.deploy-consent-portal-cfn-provisions-and-tombstones bedrock-agentcore-control__cloudformation.deploy-dataset-cfn-behaves bedrock-agentcore-control__cloudformation.deploy-dataset-missing-turns-field-rejected bedrock-agentcore-control__cloudformation.deploy-duplicate-gateway-rate-limit-dimension-keys-rejected-and-rolled-back bedrock-agentcore-control__cloudformation.deploy-evaluator-cfn-behaves bedrock-agentcore-control__cloudformation.deploy-evaluator-invalid-placeholder-rejected bedrock-agentcore-control__cloudformation.deploy-gateway-rate-limit-rule-and-resource-policy-cfn-provisions-and-tombstones bedrock-agentcore-control__cloudformation.deploy-oauth2-credential-provider-cfn-provisions-and-tombstones bedrock-agentcore-control__cloudformation.deploy-online-evaluation-config-cfn-behaves bedrock-agentcore-control__cloudformation.deploy-policy-wildcard-resource-statement-rejected bedrock-agentcore-control__cloudformation.deploy-policyengine-and-policy-round-trip-then-child-and-stack-delete bedrock-agentcore-control__cloudformation.deploy-resource-policy-mismatched-arn-rejected-and-rolled-back bedrock-agentcore-control__cloudformation.deploy-runtime-endpoint-cfn-behaves
Issue-Key: bedrock-agentcore-control__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# bedrock-agentcore-control__cloudformation: no AWS::BedrockAgentCore::* resource type is registered — CreateStack refuses every AgentCore template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation template resource-type registry contains none of the AWS::BedrockAgentCore::* resource types, so every template that declares one is refused at submit and no AgentCore resource can ever be provisioned by a stack.

AWS: recorded behavior only — AWS CloudFormation recognizes AWS::BedrockAgentCore::BrowserCustom, ::BrowserProfile, ::ConfigurationBundle, ::ConsentPortal, ::Dataset, ::Evaluator, ::Gateway, ::GatewayTarget, ::GatewayRateLimit, ::GatewayRule, ::OAuth2CredentialProvider, ::OnlineEvaluationConfig, ::Policy, ::PolicyEngine, ::ResourcePolicy and ::RuntimeEndpoint. CreateStack with such a template answers 200 with a StackId, provisions each resource through the corresponding Bedrock AgentCore control-plane API, and the stack reaches CREATE_COMPLETE. The "Template format error: Unrecognized resource types: [...]" 400 ValidationError is reserved for types that genuinely do not exist.

Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [<the AWS::BedrockAgentCore::* types the template declares>]`. The fixed text and the bracketed list formatting match AWS; what diverges is that these types ARE real AWS types, so the refusal itself is the defect. Nothing is created, so neither the happy-path round-trips nor the per-resource validation refusals (a Dataset missing its turns field, duplicate GatewayRateLimit dimension keys, an Evaluator with an invalid placeholder, a Policy with a wildcard resource statement, a ResourcePolicy with a mismatched ARN) are ever reached.

Why: a client cannot deploy any Bedrock AgentCore estate from a template — the whole infrastructure-as-code path for the service is closed. Every CDK/SAM/CLI deployment that includes an AgentCore resource fails at `cdk deploy`/`create-stack` time with an error that tells the user the resource type does not exist, which is false.

Fix: register each AWS::BedrockAgentCore::* type in the CloudFormation service's template resource-type registry (the table CreateStack validates Resources[*].Type against) and add a resource handler per type that calls the bedrock-agentcore-control crate's create/read/update/delete operations, mints the PhysicalResourceId AWS mints, and supplies the Ref/GetAtt attributes each type documents.

Traces: the 15 checks listed in trace_ids, all failing on the same CreateStack refusal.

## Evidence
Expected: CreateStack 200 + StackId, stack reaches CREATE_COMPLETE, each AWS::BedrockAgentCore::* resource readable through the bedrock-agentcore-control API.

Actual (every one of the 15 checks): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [...], e.g.
  [AWS::BedrockAgentCore::BrowserCustom, AWS::BedrockAgentCore::BrowserProfile]
  [AWS::BedrockAgentCore::ConfigurationBundle]
  [AWS::BedrockAgentCore::OAuth2CredentialProvider, AWS::BedrockAgentCore::ConsentPortal]
  [AWS::BedrockAgentCore::Dataset]
  [AWS::BedrockAgentCore::GatewayRateLimit]
  [AWS::BedrockAgentCore::Evaluator]
  [AWS::BedrockAgentCore::GatewayTarget, AWS::BedrockAgentCore::GatewayRateLimit, AWS::BedrockAgentCore::GatewayRule, AWS::BedrockAgentCore::ResourcePolicy]
  [AWS::BedrockAgentCore::OnlineEvaluationConfig]
  [AWS::BedrockAgentCore::PolicyEngine, AWS::BedrockAgentCore::Policy]
  [AWS::BedrockAgentCore::ResourcePolicy]
  [AWS::BedrockAgentCore::RuntimeEndpoint]
The bracketed type list is the only part that varies between checks; the status (400), the code (ValidationError) and the message text are identical.

## Steps to Reproduce
1. Submit any CloudFormation template declaring an AWS::BedrockAgentCore::* resource, e.g. Resources: {Ds: {Type: 'AWS::BedrockAgentCore::Dataset', Properties: {...}}}.
2. Call CreateStack.
3. Observe 400 ValidationError "Template format error: Unrecognized resource types: [AWS::BedrockAgentCore::Dataset]" instead of a 200 StackId and a stack that reaches CREATE_COMPLETE.
4. Re-replay the 15 checks in trace_ids to confirm.
