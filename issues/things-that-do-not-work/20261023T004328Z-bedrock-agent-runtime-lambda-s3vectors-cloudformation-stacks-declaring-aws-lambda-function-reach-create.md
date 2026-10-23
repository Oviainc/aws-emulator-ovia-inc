Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-role-bedrock-agent-permission-positive bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-role-missing-bedrock-permission-negative bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-role-missing-s3vectors-permission-negative bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-role-s3vectors-permission-positive bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-s3vectors-policy-wrong-resource-arn-negative bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-missing-trust-policy-negative bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-agent-missing-retrieve-permission-negative bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-positive
Issue-Key: lambda|Invoke|ResourceNotFoundException
Issue-Key: lambda|GetFunctionConfiguration|ResourceNotFoundException
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-missing-trust-policy-negative|run#3|AssertionError|assert bad["status"] in {"CREATE_FAILED", "ROLLBACK_COMPLETE", "ROLLBACK_FAILED"}, bad
Issue-Key: bedrock-agent-runtime__lambda__s3vectors|CreateKnowledgeBase|NotImplemented
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# CloudFormation stacks declaring AWS::Lambda::Function reach CREATE_COMPLETE but nothing is registered in Lambda, an untrustable execution role never fails the stack, and CreateKnowledgeBase is a stub (3 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three root causes behind the cross-service (bedrock-agent-runtime + lambda + s3vectors) CloudFormation deployment checks. These are the failures in that suite that are NOT already owned by 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (Lambda Invoke not running the deployment package) or by 20261022T172625Z-codebuild-codepipeline-ecr-ci-chain-pipeline-executions-never.md (a CFN-created AWS::IAM::Role not visible to IAM GetRole).

=== Cause 1 of 3: a CFN-provisioned AWS::Lambda::Function does not exist in Lambda ===
AWS: recorded behavior only for the stack contract: CreateStack on a template declaring AWS::IAM::Role + AWS::Lambda::Function reaches CREATE_COMPLETE only once the function has actually been created, and the function's PhysicalResourceId is a real function name that lambda:GetFunctionConfiguration and lambda:Invoke then resolve. ResourceNotFoundException with 'Function not found: <function arn>' is AWS's answer only for a function that was never created.
Emulator: the stack reports CREATE_COMPLETE and hands back the function's physical id, but every subsequent Lambda call on that id answers ResourceNotFoundException 'Function not found: arn:aws:lambda:<region>:<account>:function:<physical id>' — from GetFunctionConfiguration and from Invoke alike. Stack creation is therefore reporting success for a resource it never materialised in the Lambda service; the same holds after UpdateStack (the add-permission update checks fail identically).
Why: every CloudFormation-deployed Lambda workload is broken end to end — a client that deploys a stack and then configures, waits on, or invokes the declared function gets a 404 for a resource CloudFormation told it exists, and CFN's own success signal becomes meaningless. Adjacent filed issues describe the same shape for other resource types (e.g. 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources, 20261022T234922Z-ecr-ecs-elbv2-...); none of them covers AWS::Lambda::Function.
Fix: the CloudFormation stack-provisioning path must create AWS::Lambda::Function resources in the Lambda service's state (and keep them in sync on UpdateStack), and use the created function's real name/ARN as the PhysicalResourceId.
Traces: 9 checks, see trace_ids (cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive, cfn-deploy-wrong-execution-role-mismatch-negative, cfn-lambda-role-bedrock-agent-permission-positive, cfn-lambda-role-missing-bedrock-permission-negative, cfn-lambda-role-missing-s3vectors-permission-negative, cfn-lambda-role-s3vectors-permission-positive, cfn-lambda-s3vectors-policy-wrong-resource-arn-negative, cfn-stack-update-add-bedrock-agent-permission-positive, cfn-stack-update-add-s3vectors-permission-positive).

=== Cause 2 of 3: a stack whose Lambda execution role does not trust lambda.amazonaws.com still reaches CREATE_COMPLETE ===
AWS: recorded behavior only: CreateFunction validates at creation time that Lambda can assume the supplied role. When the template's AWS::IAM::Role trusts ec2.amazonaws.com instead of lambda.amazonaws.com, the stack's AWS::Lambda::Function resource fails to create, the resource's status is not CREATE_COMPLETE, and the stack ends in CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED with the function never created (GetFunction on its PhysicalResourceId then answers ResourceNotFoundException).
Emulator: the stack reports status CREATE_COMPLETE. No resource failure, no rollback, no refusal of the untrustable role.
Why: an over-permissive negative class. A deployment that real AWS rejects is reported as a clean deploy, so a template with a broken trust policy ships and only fails at runtime — and a test suite proving CFN rejects it passes falsely.
Fix: the AWS::Lambda::Function provisioner must perform the CreateFunction role-trust validation and fail the resource, and the stack must transition to CREATE_FAILED/ROLLBACK rather than CREATE_COMPLETE when a declared resource fails.
Traces: bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-missing-trust-policy-negative.

=== Cause 3 of 3: bedrock-agent CreateKnowledgeBase is a scaffold stub, blocking the Lambda->Bedrock retrieval scenarios ===
AWS: recorded behavior only: CreateKnowledgeBase returns a knowledgeBaseId that the deployed Lambda then calls bedrock-agent-runtime Retrieve / RetrieveAndGenerate against (including the negative case where the function's role lacks bedrock:Retrieve and the call is denied).
Emulator: 'An error occurred (NotImplemented) when calling the CreateKnowledgeBase operation: operation CreateKnowledgeBase is scaffolded but not implemented in this emulator' during setup. (Same stub as reported for the single-service bedrock-agent-runtime suite; recorded here because these checks carry a different service token and must be re-replayed separately.)
Fix: implement CreateKnowledgeBase in services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_knowledge_base.rs.
Traces: bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-agent-missing-retrieve-permission-negative, bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-and-generate-positive, bedrock-agent-runtime__lambda__s3vectors.lambda-bedrock-retrieve-positive.

## Evidence
Cause 1 — expected: lambda GetFunctionConfiguration / Invoke on the PhysicalResourceId of the stack's AWS::Lambda::Function succeed after CREATE_COMPLETE. actual: 404 ResourceNotFoundException 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-stk-d739c8fe77a5-Fn-565C625B1CD5' (GetFunctionConfiguration) and 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-stk-0b04bcdd04b3-Fn-1291DC025250' (Invoke); identical after UpdateStack ('...sim2real-xsvc-stk-244333e9a870-Fn-1F3975EE88A0').

Cause 2 — expected: stack status in {CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED} and the Lambda::Function resource not CREATE_COMPLETE. actual: {'stack_name': 'sim2real-xsvc-stk-de1c8c089869', 'stack_id': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-xsvc-stk-de1c8c089869/4860176d-3826-8b67-b045-205fc7974c20', 'status': 'CREATE_COMPLETE'} for a template whose ExecRole trusts ec2.amazonaws.com.

Cause 3 — expected: CreateKnowledgeBase returns a knowledgeBaseId. actual: 501 'operation CreateKnowledgeBase is scaffolded but not implemented in this emulator'.

## Steps to Reproduce
Cause 1: cloudformation.create_stack with a template declaring {ExecRole: AWS::IAM::Role trusting lambda.amazonaws.com, Fn: AWS::Lambda::Function (Runtime python3.12, Handler index.handler, Role: !GetAtt ExecRole.Arn, inline ZipFile)}; wait for CREATE_COMPLETE; read the Fn resource's PhysicalResourceId from describe_stack_resources; then lambda.get_function_configuration(FunctionName=<that id>) or lambda.invoke(FunctionName=<that id>) -> 404 'Function not found: ...'. Repeat after update_stack adding a policy to the role.
Cause 2: same template but with the role's AssumeRolePolicyDocument Principal set to {'Service': 'ec2.amazonaws.com'}; create_stack and poll describe_stacks -> CREATE_COMPLETE instead of CREATE_FAILED/ROLLBACK_*.
Cause 3: bedrock-agent.create_knowledge_base(...) during the Lambda->Bedrock retrieval setup -> 501 NotImplemented.
