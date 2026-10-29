Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.metrics-from-logs-fourth-dimension-rejected cloudwatch__lambda__logs.metrics-from-logs-literal-dimension-value-rejected cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow cloudwatch__lambda__logs.cli-lambda-missing-log-permission-negative cloudwatch__lambda__logs.cli-lambda-tail-logs-workflow
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-fourth-dimension-rejected|run#9|AssertionError|assert err == expected, (
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-literal-dimension-value-rejected|run#9|AssertionError|assert err == expected, (
Issue-Key: cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow|_run_shell#10|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__lambda__logs.cli-lambda-missing-log-permission-negative|_run_shell#13|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__lambda__logs.cli-lambda-tail-logs-workflow|_run_shell#7|AssertionError|raise AssertionError(
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# CloudWatch Logs metric-filter refusal texts are rewritten, the CFN log-group retention deploy never settles, and the CLI workflows are blocked by Lambda Invoke

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
This issue bundles 3 net-new root causes. All 3 must be closed before submitting.

Cause 1 of 3: PutMetricFilter's InvalidParameterException messages are the emulator's own prose instead of AWS's fixed texts.
AWS: a metric transformation may declare at most 3 dimensions; a fourth is refused with error code `InvalidParameterException` and the fixed message `Number of dimensions exceeds the maximum number of dimensions supported per metric filter` (no variable part). A dimension VALUE that is a literal rather than a field selector is refused with `InvalidParameterException` and the fixed message `Invalid metric transformation: dimension value must be valid selector`.
Emulator: the codes are right but the messages are rewritten — `a metric filter can include as many as 3 dimensions, counting the system fields emitted as dimensions` and `each dimension value must name a field of the log event, selected with '$' — '$.path' for a JSON log event and '$field' for a space-delimited one — never a literal value`.
Why: tooling and tests that assert on AWS's refusal text (and support runbooks that quote it) do not recognise the emulator's sentences, and the emulator's wording implies rules AWS does not state.
Fix: PutMetricFilter's validation messages in the CloudWatch Logs handler.
Traces: cloudwatch__lambda__logs.metrics-from-logs-fourth-dimension-rejected, cloudwatch__lambda__logs.metrics-from-logs-literal-dimension-value-rejected.

Cause 2 of 3: a CloudFormation stack declaring a Lambda function plus its AWS::Logs::LogGroup retention never reaches the state the check can read back.
AWS: after CREATE_COMPLETE, DescribeLogGroups returns the stack's `/aws/lambda/<function>` group with the template's retentionInDays.
Emulator: the check exhausts its 60s budget — the deployed log group with its retention never becomes observable.
Why: log-retention-as-code cannot be validated; a stack that claims to set retention may set nothing.
Fix: the CloudFormation providers for AWS::Lambda::Function and AWS::Logs::LogGroup (retention) and their write-through to the Logs state store.
Traces: cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive.

Cause 3 of 3: the three AWS-CLI workflow checks are refused at `lambda invoke` with an unmodelled `NotImplemented`, so the log-group/tail workflows behind it never run.
AWS: `Invoke` runs the function's deployment package and returns the function's own response payload (and, for RequestResponse, the report line); `NotImplemented` is not a modelled Lambda error.
Emulator: `aws lambda invoke` exits rc=254 with `An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:<region>:<account>:function:<name> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator`. This is the same defect as the open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md, surfacing through the CLI wrapper rather than botocore — these three replay keys are not on that issue.
Why: every CLI-driven "invoke then inspect logs" workflow (describe-log-groups after invoke, tail logs, the missing-log-permission negative) fails at the first step, so the Logs behaviour they grade is unverifiable.
Fix: the Lambda Invoke handler's execution path (as tracked by the open Invoke issue); these checks should be re-replayed once it lands.
Traces: cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow, cloudwatch__lambda__logs.cli-lambda-missing-log-permission-negative, cloudwatch__lambda__logs.cli-lambda-tail-logs-workflow.

## Evidence
Cause 1 —
cloudwatch__lambda__logs.metrics-from-logs-fourth-dimension-rejected: got ('InvalidParameterException', 'a metric filter can include as many as 3 dimensions, counting the system fields emitted as dimensions'), expected ('InvalidParameterException', 'Number of dimensions exceeds the maximum number of dimensions supported per metric filter')
cloudwatch__lambda__logs.metrics-from-logs-literal-dimension-value-rejected: got ('InvalidParameterException', "each dimension value must name a field of the log event, selected with '$' ... never a literal value"), expected ('InvalidParameterException', 'Invalid metric transformation: dimension value must be valid selector')

Cause 2 — cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive: `Error: Check timed out after 60.0s`.

Cause 3 — cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow:
  AssertionError: Command failed (rc=254):
  cmd: ['aws', '--output', 'json', 'lambda', 'invoke', '--function-name', '<fn>', '--payload', '{}', '/dev/null']
  stderr: aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: ... running its python3.12 deployment package is not implemented in this emulator ...
(identically for cli-lambda-tail-logs-workflow and cli-lambda-missing-log-permission-negative)

## Steps to Reproduce
Replay the trace_ids. The two metric-filter checks call PutMetricFilter with a 4-dimension metricTransformation and with a literal dimension value, asserting code and message. The CFN check deploys a stack with AWS::Lambda::Function + log-group retention and reads DescribeLogGroups. The three CLI checks shell out to `aws lambda invoke` and then to `aws logs describe-log-groups` / `aws logs tail`.
