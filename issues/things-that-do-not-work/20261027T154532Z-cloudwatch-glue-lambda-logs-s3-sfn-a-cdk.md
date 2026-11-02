Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow
Issue-Key: cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow|verdict#19|AssertionError|raise AssertionError(self._summary(leg, evidence))
Issue-Commit: 48b00a2ec8a92094223b124c2e7d5eacc8c9104d

# cloudwatch__glue__lambda__logs__s3__sfn: a CDK-deployed stack's StateMachineArn output resolves to a synthesized logical name, not the state machine ARN, so sfn:StartExecution refuses it with InvalidArn (1 cause)

> _Note: this issue was identified against the code base at commit `48b00a2ec8a92094223b124c2e7d5eacc8c9104d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new root cause, reached through a bootstrapped `cdk deploy` of a Step Functions ETL app. The toolkit bootstrap, the stack deploy and the asset publish all pass (legs bootstrap/deploy/assets/toolkit PASS); the workflow dies on the first hand-off from the deployed stack to Step Functions, which blocks the remaining 7 graded legs (logs, metrics, alarm, denial, drift, rebuild, teardown all BLOCKED).

Cause 1 of 1: a CloudFormation stack output whose value is a Ref/GetAtt on an AWS::StepFunctions::StateMachine resolves to a synthesized logical-name string instead of the resource's physical id (its ARN).
AWS: Ref on AWS::StepFunctions::StateMachine returns the state machine's ARN (and Fn::GetAtt <Logical>.Arn likewise) — that is what CDK's `stateMachine.stateMachineArn` synthesizes to and what DescribeStacks hands back as OutputValue. An ARN of the form arn:<partition>:states:<region>:<account>:stateMachine:<name> is also exactly what sfn:StartExecution requires: Smithy: StartExecution.stateMachineArn -> Arn, @required, @length min 1 max 256, with modeled error InvalidArn (client, HTTP 400). Passing anything that is not an ARN is correctly refused by AWS with InvalidArn.
Emulator: the stack reaches a healthy terminal status and DescribeStacks returns the output, but its OutputValue is the emulator's synthesized '<stack>-<LogicalId>-<hex>'-shaped name rather than an ARN. Handing that value straight to StartExecution — the only thing a client can do with it — is refused: InvalidArn, "Invalid Arn: 'Invalid ARN prefix: <the output value>'". In the replay the value begins with the stack-name prefix 'sim2real-xsv…' (the per-run stack prefix), so no ARN prefix is present at all. The refusal itself is AWS-faithful: the fixed text is "Invalid Arn: 'Invalid ARN prefix: " + the caller-supplied value + "'" and the value is taken from the request, so the defect is NOT in sfn — it is the value CloudFormation produced.
Why: a client that deploys a Step Functions workflow with CDK/CloudFormation and reads its documented stack output cannot start a single execution: nothing the stack created can be addressed afterwards, and the failure surfaces inside Step Functions, which makes a CloudFormation output-resolution defect look like an sfn defect. Every downstream contract that depends on an execution (execution logs in CloudWatch Logs, the AWS/States datapoint, the alarm transition, the curated S3 object and the Glue partition) is unreachable.
Related open issue 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md names this same family (its Cause 2, physical-id resolution) for AWS::EC2::Volume and AWS::EC2::Instance and explicitly does not carry this check or the AWS::StepFunctions::StateMachine resource type; that issue does not cover this failure.
Fix: physical-id assignment and Ref/Fn::GetAtt resolution for AWS::StepFunctions::StateMachine in the CloudFormation stack engine's resource provisioning, and the Outputs resolution the DescribeStacks handler serializes. No change is needed in the sfn StartExecution handler's ARN validation.
Traces: cloudwatch__glue__lambda__logs__s3__sfn.cdk-bootstrapped-sfn-etl-workflow (leg `wiring`).

## Evidence
Replay verdict for cloudwatch__glue__lambda__logs__s3__sfn.cdk-bootstrapped-sfn-etl-workflow:

  AssertionError: LEGS 4/12 PASS | bootstrap=PASS deploy=PASS assets=PASS wiring=FAIL logs=BLOCKED metrics=BLOCKED alarm=BLOCKED denial=BLOCKED drift=BLOCKED rebuild=BLOCKED teardown=BLOCKED toolkit=PASS
  WORKFLOW FAILED at wiring: An error occurred (InvalidArn) when calling the StartExecution operation: Invalid Arn: 'Invalid ARN prefix: sim2real-xsv…

The failing call is `_start(ctx, outputs["StateMachineArn"], …)` inside `_assert_pipeline_curates`, where `outputs` is `{o["OutputKey"]: o["OutputValue"] for o in describe_stacks(StackName=<stack>)["Stacks"][0]["Outputs"]}`.

Expected (AWS): StateMachineArn's OutputValue is arn:aws:states:<region>:<account>:stateMachine:<name>, and StartExecution on it returns 200 with executionArn + startDate.
Actual (emulator): OutputValue is a synthesized logical name carrying the stack-name prefix (no 'arn:' prefix), and StartExecution answers HTTP 400 InvalidArn with message "Invalid Arn: 'Invalid ARN prefix: <value>'". Fixed text of that message: "Invalid Arn: 'Invalid ARN prefix: " … "'"; the variable part is the stateMachineArn value copied from the request (the per-run stack prefix, logical id and hex suffix differ every run and are not part of the requirement).

## Steps to Reproduce
1. Replay cloudwatch__glue__lambda__logs__s3__sfn.cdk-bootstrapped-sfn-etl-workflow.
2. Minimal repro without CDK: deploy a CloudFormation template declaring an AWS::StepFunctions::StateMachine with an Output whose Value is { "Ref": "<LogicalId>" } (or { "Fn::GetAtt": ["<LogicalId>", "Arn"] }).
3. Wait for CREATE_COMPLETE, then call cloudformation:DescribeStacks and read Stacks[0].Outputs[] for that OutputKey — the OutputValue is a synthesized '<stack>-<LogicalId>-<hex>' name instead of an arn:…:states:…:stateMachine:… ARN.
4. Pass that OutputValue to states:StartExecution as stateMachineArn — it is refused with HTTP 400 InvalidArn "Invalid Arn: 'Invalid ARN prefix: <value>'" instead of starting an execution.
