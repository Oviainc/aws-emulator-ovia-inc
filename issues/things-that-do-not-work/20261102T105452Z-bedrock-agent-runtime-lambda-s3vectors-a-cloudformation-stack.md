Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-missing-trust-policy-negative
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-missing-trust-policy-negative|run#4|AssertionError|assert bad["status"] in {"CREATE_FAILED", "ROLLBACK_COMPLETE", "ROLLBACK_FAILED"}, bad
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# bedrock-agent-runtime__lambda__s3vectors: a CloudFormation stack whose Lambda execution role does not trust lambda.amazonaws.com still reaches CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE root cause. Cause 1 of 1: a CloudFormation stack declaring an AWS::Lambda::Function whose AWS::IAM::Role trust policy names a principal Lambda is not — ec2.amazonaws.com instead of lambda.amazonaws.com — is created successfully instead of failing the resource and rolling the stack back.

Re-confirmation note (not a new defect in a new place): open issue 20261023T004328Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-stacks-declaring-aws-lambda-function-reach-create.md describes this same cause as its Cause 2 and even lists this check id, but the replay report did NOT mark the check as OWNED (its recorded replay key no longer matches the current assertion site), so the check is still ungated. It is re-filed here so the failing check stays grounded; close this issue together with that one.

AWS: recorded behavior only — CloudFormation resource-provisioning semantics are not expressed in a Smithy model. CreateFunction validates at creation time that the Lambda service can assume the supplied execution role, so when the template's role trusts a non-Lambda service principal the stack's AWS::Lambda::Function resource FAILS to create: the resource's ResourceStatus is not CREATE_COMPLETE, the stack ends in CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED, and the function is never registered — lambda:GetFunction on the failed resource's PhysicalResourceId is then refused ResourceNotFoundException with message "Function not found: arn:aws:lambda:<region>:<accountId>:function:<functionName>" (fixed text "Function not found: " followed by the function ARN composed from the region, the caller's account id and the physical resource id). The identical template with Principal {"Service": "lambda.amazonaws.com"} deploys and the function is invokable.
Emulator: the stack with the ec2.amazonaws.com trust policy reports StackStatus CREATE_COMPLETE — no resource failure, no rollback event, no refusal of the untrustable role.
Why: this is an over-permissive negative class. A template real AWS rejects is reported as a clean deploy, so a broken trust policy ships and only surfaces much later as an invocation-time failure with no deployment signal; a client or test suite that proves CloudFormation rejects the mis-trusted role passes falsely. Because the stack never fails, the downstream contract (the failed resource's PhysicalResourceId resolving to no function, with AWS's ResourceNotFoundException sentence) is unreachable too.
Fix: the AWS::Lambda::Function provisioner in the CloudFormation emulator (services/service-com-amazonaws-cloudformation-cloudformation resource provisioners) — perform the CreateFunction role-trust validation when provisioning the resource, mark the resource CREATE_FAILED, and transition the stack to CREATE_FAILED/ROLLBACK_* instead of CREATE_COMPLETE when a declared resource fails.
Traces: bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-missing-trust-policy-negative.

## Evidence
Check bedrock-agent-runtime__lambda__s3vectors.cfn-lambda-missing-trust-policy-negative: a stack declaring {ExecRole: AWS::IAM::Role with AssumeRolePolicyDocument Principal {'Service': 'ec2.amazonaws.com'} and the AWSLambdaBasicExecutionRole managed policy, Fn: AWS::Lambda::Function (Runtime python3.12, Handler index.handler, Role !GetAtt ExecRole.Arn, inline ZipFile)} was expected to end with StackStatus in {CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED}; actual {'stack_name': 'sim2real-xsvc-stk-338bdb8430e5', 'stack_id': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-xsvc-stk-338bdb8430e5/77e606cd-1c26-b292-3e93-6b1bd6c9aa8f', 'status': 'CREATE_COMPLETE'} (the stack name and id are this run's generated values). AssertionError raised at check_bedrock_agent_runtime__lambda__s3vectors_cfn_lambda_missing_trust_policy_negative.py line 28. Because the stack succeeded, the later legs — DescribeStackResources showing the Lambda::Function resource NOT CREATE_COMPLETE, and lambda GetFunction on its PhysicalResourceId answering ResourceNotFoundException 'Function not found: arn:aws:lambda:<region>:<account>:function:<physical id>' — were never reached.

## Steps to Reproduce
0. git_clone, then run_behavioral_suite and read the '### bedrock-agent-runtime__lambda__s3vectors' section of the failing-check list; the grounding check is cfn-lambda-missing-trust-policy-negative.
1. cloudformation.create_stack(StackName=<fresh>, Capabilities=['CAPABILITY_IAM'], TemplateBody=<a template with ExecRole: AWS::IAM::Role whose AssumeRolePolicyDocument Statement has Principal {'Service': 'ec2.amazonaws.com'} and Action sts:AssumeRole, plus Fn: AWS::Lambda::Function with Runtime python3.12, Handler index.handler, Role: {'Fn::GetAtt': ['ExecRole','Arn']}, Timeout 20 and Code.ZipFile, and Outputs {FunctionName: {Ref: Fn}}>).
2. Poll describe_stacks(StackName=<that stack>) until the status is terminal -> observe CREATE_COMPLETE where CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED is required.
3. describe_stack_resources(StackName=<that stack>) and read the AWS::Lambda::Function resource -> its ResourceStatus must not be CREATE_COMPLETE; then lambda.get_function(FunctionName=<its PhysicalResourceId>) -> must be refused ResourceNotFoundException 'Function not found: arn:aws:lambda:<region>:<account>:function:<that physical id>'.
4. Positive control, same template with Principal {'Service': 'lambda.amazonaws.com'}: the stack deploys and invoking the function returns {"ok": true}.
