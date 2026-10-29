Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.deploy-agent-runtime-cfn-behaves bedrock-agentcore-control.deploy-memory-cfn-behaves bedrock-agentcore-control.deploy-gateway-cfn-behaves
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetAgentRuntime|ResourceNotFoundException
Issue-Key: bedrock-agentcore-control|GetMemory|ResourceNotFoundException
Issue-Key: bedrock-agentcore-control|CreateStack|ValidationError
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# CloudFormation does not provision AgentCore resources: AWS::BedrockAgentCore::Runtime and ::Memory stacks complete without creating anything, and ::GatewayTarget is an unrecognized resource type (3 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Deploying AgentCore resources through CloudFormation diverges three ways: two resource types report success while creating nothing in bedrock-agentcore-control, and a third is rejected as unknown at template-validation time. The checks are bedrock-agentcore-control checks (the assertions are AgentCore reads), but the defect sits in the CloudFormation resource-type provisioning for this service. 3 causes must all be fixed before this issue is closed.

Cause 1 of 3: an AWS::BedrockAgentCore::Runtime stack reaches CREATE_COMPLETE and exports an AgentRuntimeId that does not exist.
AWS: recorded behavior only (the resource type is a CloudFormation type, not a Smithy operation). A stack whose Resources contain an AWS::BedrockAgentCore::Runtime (AgentRuntimeName, Description, RoleArn, container artifact) reaches CREATE_COMPLETE only once the runtime exists; its outputs' AgentRuntimeId/AgentRuntimeArn resolve, GetAgentRuntime on that id returns the runtime and it converges to READY; a stack update bumps agentRuntimeVersion, and only after stack delete does GetAgentRuntime report not-found.
Emulator: the stack completes and the outputs carry an AgentRuntimeId, but GetAgentRuntime on that id answers 404 ResourceNotFoundException — the CFN resource minted an identifier without creating any runtime in the service's state.
Why: infrastructure-as-code is the normal way these resources are created; a client gets a green stack and an id that no AgentCore API call can resolve, so every downstream step (readiness wait, update, invoke) fails against a resource that was reported created.
Fix: the CloudFormation provisioning for the AWS::BedrockAgentCore::Runtime resource type — it must call through to the CreateAgentRuntime path and register the resulting runtime in bedrock-agentcore-control state, and derive its outputs from that resource.
Traces: bedrock-agentcore-control.deploy-agent-runtime-cfn-behaves.

Cause 2 of 3: an AWS::BedrockAgentCore::Memory stack reaches CREATE_COMPLETE and exports a memory id that does not exist.
AWS: recorded behavior only. A stack containing an AWS::BedrockAgentCore::Memory completes with the memory created; GetMemory on the stack's exported id returns it.
Emulator: the stack completes, then GetMemory on the exported id answers 404 ResourceNotFoundException ('no memory with id <id> exists in this account').
Why: same as cause 1 for the memory family — the stack is a false success and the exported id is unusable.
Fix: the CloudFormation provisioning for the AWS::BedrockAgentCore::Memory resource type, wired to the CreateMemory path and the service's memory state.
Traces: bedrock-agentcore-control.deploy-memory-cfn-behaves.

Cause 3 of 3: AWS::BedrockAgentCore::GatewayTarget is not a known resource type, so the gateway template cannot even be submitted.
AWS: recorded behavior only. CreateStack accepts a template whose Resources include AWS::BedrockAgentCore::Gateway and AWS::BedrockAgentCore::GatewayTarget and provisions both.
Emulator: CreateStack is refused 400 ValidationError with 'Template format error: Unrecognized resource types: [AWS::BedrockAgentCore::GatewayTarget]' — the type is absent from the resource-type registry, so the whole deployment is unreachable rather than merely inert.
Why: a template AWS accepts is rejected outright, so no gateway/target deployment path exists at all; the error also tells the caller their template is malformed when it is not.
Fix: register the AWS::BedrockAgentCore::GatewayTarget resource type (and provision it via the CreateGatewayTarget path) in the CloudFormation resource-type registry used by template validation.
Traces: bedrock-agentcore-control.deploy-gateway-cfn-behaves.

## Evidence
Cause 1 — bedrock-agentcore-control.deploy-agent-runtime-cfn-behaves: after the stack's create waiter returned and outputs were read, 'An error occurred (ResourceNotFoundException) when calling the GetAgentRuntime operation: no agent runtime sim2realCfnRuntime6ea1c2e29bb9-9ca4f5c18d in this account'. Template resource: AgentRuntime, Type AWS::BedrockAgentCore::Runtime, with AgentRuntimeName/Description/RoleArn.
Cause 2 — bedrock-agentcore-control.deploy-memory-cfn-behaves: 'An error occurred (ResourceNotFoundException) when calling the GetMemory operation: no memory with id sim2realCfnMem851792650e71-0e83bfa5cf exists in this account' after the stack completed.
Cause 3 — bedrock-agentcore-control.deploy-gateway-cfn-behaves: 'An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::BedrockAgentCore::GatewayTarget]'.
The stack names, runtime ids and memory ids are per-run generated values; the requirement is that the id a completed stack exports resolves through the AgentCore read APIs, and that the template is accepted.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: CreateStack with a template containing an AWS::BedrockAgentCore::Runtime (plus its AWS::IAM::Role), wait for stack_create_complete, read the AgentRuntimeId output, then call GetAgentRuntime with it (check deploy-agent-runtime-cfn-behaves).
3. Cause 2: the same with an AWS::BedrockAgentCore::Memory, then call GetMemory with the exported id (check deploy-memory-cfn-behaves).
4. Cause 3: CreateStack with a template containing AWS::BedrockAgentCore::Gateway and AWS::BedrockAgentCore::GatewayTarget and observe the CreateStack refusal (check deploy-gateway-cfn-behaves).
