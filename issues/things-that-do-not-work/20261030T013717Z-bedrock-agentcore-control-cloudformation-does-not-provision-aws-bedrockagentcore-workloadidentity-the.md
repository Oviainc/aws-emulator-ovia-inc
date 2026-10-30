Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.deploy-workload-identity-cfn-behaves
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetWorkloadIdentity|ResourceNotFoundException
Issue-Commit: 80128cf5220e52d7fa07858b1a17d4e0aeba15d4

# CloudFormation does not provision AWS::BedrockAgentCore::WorkloadIdentity: the stack completes and exports a name/ARN no GetWorkloadIdentity call can resolve (1 cause)

> _Note: this issue was identified against the code base at commit `80128cf5220e52d7fa07858b1a17d4e0aeba15d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Deploying a workload identity through CloudFormation reports success while nothing is created in bedrock-agentcore-control, so the identity the stack exports does not exist. 1 cause must be fixed before this issue is closed.

Note: the open issue 20261029T212239Z-bedrock-agentcore-control-cloudformation-does-not-provision-agentcore-resources-aws-bedrockagentcore.md covers the AWS::BedrockAgentCore::Runtime and ::Memory resource types and the unregistered ::GatewayTarget type; it does NOT mention AWS::BedrockAgentCore::WorkloadIdentity, which is this cause. The open issue 20261030T002232Z-bedrock-agentcore-control-workload-identity-state-is-not-read-back-updates.md covers the API-level read-back/delete/message defects of the workload-identity store, not the CloudFormation provisioning path.

Cause 1 of 1: a stack containing AWS::BedrockAgentCore::WorkloadIdentity reaches CREATE_COMPLETE without creating the identity.
AWS: recorded behavior only for the CloudFormation resource type (it is not a Smithy operation). A stack whose Resources declare AWS::BedrockAgentCore::WorkloadIdentity (Name, Tags) reaches CREATE_COMPLETE only once the identity exists; its Ref resolves to the identity name and Fn::GetAtt WorkloadIdentityArn to the identity ARN, and GetWorkloadIdentity on that name then answers HTTP 200 echoing the same workloadIdentityArn (Smithy: GetWorkloadIdentity is @readonly, POST /identities/GetWorkloadIdentity, input member name -> WorkloadIdentityNameType @required @length 3..255 @pattern ^[A-Za-z0-9_.-]+$; output members name, workloadIdentityArn, allowedResourceOauth2ReturnUrls, createdTime, lastUpdatedTime; errors include ResourceNotFoundException errorKind client httpError 404). Only after the stack is deleted does GetWorkloadIdentity on that name answer 404 ResourceNotFoundException.
Emulator: the create waiter returns, DescribeStacks hands back the WorkloadIdentityName and WorkloadIdentityArn outputs, and the very first GetWorkloadIdentity on the exported name is refused 404 ResourceNotFoundException with message "Failed to retrieve workload identity because it doesn't exist. Retry the request with a different resource identifier." The CloudFormation resource type minted output values without registering anything in the workload-identity store. (That 404 message is itself the wrong wording — AWS uses "WorkloadIdentity with id <name> not found.", where only <name> comes from the request's name member — but that message defect is already filed as cause 2 of 20261030T002232Z-bedrock-agentcore-control-workload-identity-state-is-not-read-back-updates.md; the defect filed here is that the read misses at all.)
Why: infrastructure-as-code is the normal way an AgentCore workload identity is created. A client gets a green stack plus an identity name and ARN that no AgentCore API can resolve, so every dependent step — attaching the identity to a runtime, configuring OAuth2 return URLs, or simply confirming the deployment — fails against a resource CloudFormation reported as created. The stack-delete assertion is equally unreachable, so deletion semantics cannot be observed either.
Fix: the CloudFormation provisioning for the AWS::BedrockAgentCore::WorkloadIdentity resource type — it must call through to the CreateWorkloadIdentity path (Smithy: POST /identities/CreateWorkloadIdentity, http code 201, @idempotent) and register the resulting identity in bedrock-agentcore-control's workload-identity state, deriving Ref (the identity name) and Fn::GetAtt WorkloadIdentityArn from that created resource, and removing it on stack delete.
Traces: bedrock-agentcore-control.deploy-workload-identity-cfn-behaves.

## Evidence
Check bedrock-agentcore-control.deploy-workload-identity-cfn-behaves, verdict error, from the simparity-behavioral-aws-v1 replay.
The check creates an S3 bucket and code object, then CreateStack with a template whose Resources are ExplicitWorkloadIdentity (Type AWS::BedrockAgentCore::WorkloadIdentity, Properties Name/Tags), an AWS::IAM::Role and a dependent AWS::BedrockAgentCore::Runtime, with Outputs WorkloadIdentityName = !Ref ExplicitWorkloadIdentity and WorkloadIdentityArn = !GetAtt ExplicitWorkloadIdentity.WorkloadIdentityArn.
The stack_create_complete waiter returned and the Outputs were read successfully, then the first read of the identity failed:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetWorkloadIdentity operation: Failed to retrieve workload identity because it doesn't exist. Retry the request with a different resource identifier.
Expected: HTTP 200 from GetWorkloadIdentity with workloadIdentityArn equal to the stack's WorkloadIdentityArn output; only after cfn.delete_stack completes should GetWorkloadIdentity answer 404 ResourceNotFoundException with message 'WorkloadIdentity with id <name> not found.' (fixed text 'WorkloadIdentity with id ' / ' not found.'; the one variable part is the identity name from the request).
The generated stack name, identity name and ARN are per-run values; the requirement is that the name/ARN a CREATE_COMPLETE stack exports resolves through GetWorkloadIdentity until the stack is deleted.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (check bedrock-agentcore-control.deploy-workload-identity-cfn-behaves).
2. CreateStack with a template containing a resource of Type AWS::BedrockAgentCore::WorkloadIdentity (Properties: Name = <some name matching ^[A-Za-z0-9_.-]{3,255}$, Tags), and Outputs WorkloadIdentityName = !Ref <logical id>, WorkloadIdentityArn = !GetAtt <logical id>.WorkloadIdentityArn.
3. Wait for stack_create_complete, then DescribeStacks and read both Outputs.
4. Call bedrock-agentcore-control GetWorkloadIdentity(name=<WorkloadIdentityName output>): AWS answers 200 with workloadIdentityArn equal to the WorkloadIdentityArn output; the emulator answers 404 ResourceNotFoundException.
5. DeleteStack, wait for stack_delete_complete, then GetWorkloadIdentity on the same name: AWS answers 404 ResourceNotFoundException with 'WorkloadIdentity with id <name> not found.'
