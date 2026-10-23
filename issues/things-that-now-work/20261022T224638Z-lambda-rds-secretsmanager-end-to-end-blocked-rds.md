Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.cli-lambda-rds-secret-end-to-end-setup lambda__rds__secretsmanager.cli-lambda-rds-secretsmanager-end-to-end-invoke lambda__rds__secretsmanager.deploy-cfn-lambda-rds-secretsmanager-stack
Issue-Key: lambda__rds__secretsmanager.cli-lambda-rds-secret-end-to-end-setup|_run_shell#16|AssertionError|raise AssertionError(
Issue-Key: lambda__rds__secretsmanager.cli-lambda-rds-secretsmanager-end-to-end-invoke|_run_shell#7|AssertionError|raise AssertionError(
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-lambda-rds-secretsmanager-stack|_deploy_cfn#1|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c

# Lambda+RDS+SecretsManager end-to-end blocked: RDS CreateDBCluster is a 501 stub and CloudFormation stack Outputs omit OutputValue

> _Note: this issue was identified against the code base at commit `ead37dd00922f5e8ae35cfd4d29cc6e26f23d78c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster (bundle also covers DescribeStacks)

## Description
The lambda__rds__secretsmanager cross-service checks that are not already owned by an open issue fail for TWO distinct root causes, both in neighbouring crates but grounded in lambda__rds__secretsmanager.* check ids. This issue bundles TWO causes that must both be closed before submitting.

Cause 1 of 2: RDS CreateDBCluster is an unimplemented scaffold stub, so the CLI-driven provisioning of an Aurora Serverless v2 (Data API) cluster fails immediately.
AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model): CreateDBCluster requires DBClusterIdentifier and Engine, accepts DatabaseName, MasterUsername/MasterUserPassword, EnableHttpEndpoint, ServerlessV2ScalingConfiguration and Tags, and returns CreateDBClusterResult {DBCluster} on HTTP 200 with DBClusterArn populated; modeled failures are the awsQuery RDS faults (DBClusterAlreadyExistsFault 400, DBSubnetGroupNotFoundFault 404, InvalidParameterCombination-class 400, ...). NotImplemented is not a modeled error, and the AWS CLI exits 0 here.
Emulator: answers HTTP 501 with error code NotImplemented ("operation CreateDBCluster is scaffolded but not implemented in this emulator"); the CLI therefore exits rc=254. Handler: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_cluster.rs (SCAFFOLDED marker present).
Why: the canonical serverless-app topology — Lambda reading its DB credentials from a Secrets Manager secret and querying Aurora over the RDS Data API — cannot be stood up at all, by SDK or by CLI, and the caller sees an unmodeled error code with a non-retryable 501 status instead of a cluster.
Fix: the operation's handler plus DB-cluster state in the rds crate (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_cluster.rs and that crate's cluster store).
Traces: lambda__rds__secretsmanager.cli-lambda-rds-secret-end-to-end-setup, lambda__rds__secretsmanager.cli-lambda-rds-secretsmanager-end-to-end-invoke.
NOTE: the replay report auto-filed a CreateDBCluster stub issue against the single rds.* check it blocked; that issue does not carry these two replay keys.

Cause 2 of 2: CloudFormation DescribeStacks returns Outputs entries without OutputValue.
AWS: recorded behavior only (the Output shape pairs OutputKey with its resolved value) — once a stack reaches CREATE_COMPLETE, DescribeStacks returns HTTP 200 and every Stacks[*].Outputs[*] member carries BOTH OutputKey and OutputValue, the value being the resolved result of the template's Outputs Value expression (here !Ref/!GetAtt of the Lambda function name).
Emulator: the Outputs entries carry OutputKey but no OutputValue member, so a consumer zipping the pair raises KeyError: 'OutputValue'.
Why: every IaC consumer reads stack outputs to learn the identifiers of what it just deployed (the function name to invoke, the secret ARN, the cluster ARN). With the value absent, a stack that reports CREATE_COMPLETE is unusable and the client cannot address any deployed resource.
Fix: the Outputs resolution/serialization for DescribeStacks in the cloudformation crate (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stacks.rs plus the stack-output resolution in that crate's state).
Traces: lambda__rds__secretsmanager.deploy-cfn-lambda-rds-secretsmanager-stack.
NOTE: the open issue 20261022T120642Z-sagemaker-behavioral-checks-blocked-by-neighbouring-service-gaps.md describes the same CloudFormation defect as its Cause 4, but its replay keys are sagemaker.* only (Issue-Key sagemaker.deploy-notebook-instance-cfn-behaves|run#18|KeyError), so it does not cover this check; it may be closed together with that cause.

## Evidence
Cause 1: lambda__rds__secretsmanager.cli-lambda-rds-secret-end-to-end-setup — "AssertionError: Command failed (rc=254): cmd: ['aws','--output','json','rds','create-db-cluster','--db-cluster-identifier','...','--engine','aurora-postgresql',...,'--enable-http-endpoint',...] stderr: aws: [ERROR]: An error occurred (NotImplemented) when calling the CreateDBCluster operation: operation CreateDBCluster is scaffolded but not implemented in this emulator". The same failure, with --engine aurora-mysql, for lambda__rds__secretsmanager.cli-lambda-rds-secretsmanager-end-to-end-invoke. Expected: rc=0 / HTTP 200 with DBCluster.DBClusterArn.
Cause 2: lambda__rds__secretsmanager.deploy-cfn-lambda-rds-secretsmanager-stack — after the stack is polled to CREATE_COMPLETE, 'outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}' raises "KeyError: 'OutputValue'", i.e. the DescribeStacks Outputs entry has an OutputKey but no OutputValue.
The three other deploy-cfn checks in this service fail on the already-owned EC2 DescribeVpcs 'isDefault' filter cause and are not part of this issue.

## Steps to Reproduce
Cause 1: aws rds create-db-cluster --db-cluster-identifier c1 --engine aurora-mysql --master-username dbadmin --master-user-password '<pw>' --database-name appdb --enable-http-endpoint --serverless-v2-scaling-configuration MinCapacity=0.5,MaxCapacity=2 -> expect rc=0 and HTTP 200 with DBCluster.DBClusterArn; observe rc=254 and HTTP 501 error code NotImplemented.
Cause 2: cloudformation.create_stack with a template whose Outputs section exports a value (e.g. Outputs.FunctionName: Value !Ref of an AWS::Lambda::Function), poll describe_stacks until StackStatus == CREATE_COMPLETE, then read Stacks[0].Outputs[*] -> expect each entry to have both OutputKey and OutputValue; observe entries with OutputKey only (KeyError: 'OutputValue').
