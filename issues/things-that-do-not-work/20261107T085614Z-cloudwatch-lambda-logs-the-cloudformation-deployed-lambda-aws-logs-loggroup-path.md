Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive cloudwatch__lambda__logs.cfn-deploy-lambda-missing-logs-policy-rejected
Issue-Key: cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive|_poll_until#9|Timeout|raise AssertionError(
Issue-Key: cloudwatch__lambda__logs.cfn-deploy-lambda-missing-logs-policy-rejected|_poll_until#16|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: ad889ea564c74ff66a8d5d7080e6fcd3e62d9684

# The CloudFormation-deployed Lambda + AWS::Logs::LogGroup path never settles: neither the declared group's retention nor the invocation's log events ever become readable

> _Note: this issue was identified against the code base at commit `ad889ea564c74ff66a8d5d7080e6fcd3e62d9684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogGroups

## Description
This issue carries 1 net-new root cause, grounded in 2 failing checks. It must be closed before submitting.

Cause 1 of 1: a CloudFormation stack that declares an execution role, an explicit AWS::Logs::LogGroup and an AWS::Lambda::Function, then invokes that function, never reaches a state the CloudWatch Logs reads can observe. Both checks exhaust the replay budget and are cut at 60s without reaching a verdict; the replay reports only `Check timed out after 60.0s`, so it does not distinguish which of the two reads below blocks first — a fixer must make both observable.

AWS (Smithy: service-com-amazonaws-cloudwatchlogs-logs-20140328 DescribeLogGroups / FilterLogEvents; service-com-amazonaws-lambda-awsgirapiservice Invoke): after the stack reaches CREATE_COMPLETE,
  (a) DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<function>") returns that log group with `retentionInDays` equal to the template's RetentionInDays, and that value is NOT changed by the function's first write — Lambda writes into the pre-existing, stack-managed group and does not reset its retention; and
  (b) a successful Invoke (StatusCode 200, no FunctionError) of a function whose role carries AWSLambdaBasicExecutionRole causes the handler's stdout to appear as log events in that group, so FilterLogEvents(logGroupName=...) returns at least one event; while a twin function in the same stack whose role has only a trust document and no logs grant invokes just as cleanly (200, no FunctionError) and produces NO events — that contrast is the whole subject of the second check.
No error is modelled for either read here: both are 200 responses whose CONTENT is the requirement.

Emulator: neither check reaches an assertion. The retention check polls DescribeLogGroups for the declared `/aws/lambda/<fn>` group (its own budget is 60s), invokes, then polls FilterLogEvents for >=1 event (another 60s); the permission check deploys the good/bad role pair, invokes both, and compares delivered events. Both are cut at 60s with nothing to compare, so the stack's declared retention and the role-gated log delivery are both unverifiable.

Why: a client cannot validate log-retention-as-code — a stack may claim RetentionInDays and set nothing, and nothing detects it — and cannot rely on the execution role actually gating log delivery: the "granted role writes, ungranted role does not" contrast that every least-privilege review depends on produces no observable difference, because neither side produces anything.

Related already-open issues, none of which carries these replay keys: 20261029T182452Z-cloudwatch-lambda-logs-cloudwatch-logs-metric-filter-refusal-texts-are-rewritten.md lists `cfn-deploy-lambda-loggroup-retention-positive` in its trace ids but its stored replay key is `_poll_until#9|Timeout|time.sleep(interval_s)`, which no longer matches, so the suite reports the check as unowned; 20261107T035448Z-cloudwatch-lambda-logs-a-synchronous-invoke-writes-no.md and 20261106T160934Z-cloudwatch-lambda-logs-a-successful-lambda-invoke-never.md cover a directly created function rather than this CloudFormation-deployed pair.

Fix: the CloudFormation providers for AWS::Logs::LogGroup (LogGroupName + RetentionInDays) and AWS::Lambda::Function and their write-through to the CloudWatch Logs state store, plus the Lambda Invoke execution path that delivers the handler's stdout into the function's log group and the authorization gate that withholds it when the execution role grants no logs:CreateLogStream/logs:PutLogEvents.

Traces: cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive, cloudwatch__lambda__logs.cfn-deploy-lambda-missing-logs-policy-rejected.

## Evidence
Cause 1 —

cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive: `Error: Check timed out after 60.0s`.
  Expected (AWS): after CREATE_COMPLETE of a stack holding AWS::Logs::LogGroup{LogGroupName: /aws/lambda/<fn>, RetentionInDays: 7} and AWS::Lambda::Function{Runtime: python3.12, DependsOn: the log group}, DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<fn>") returns that group with retentionInDays == 7; Invoke returns StatusCode 200 with no FunctionError; FilterLogEvents returns >= 1 event; and a second DescribeLogGroups still reports retentionInDays == 7.
  Actual (emulator): the check never gets past its polls and is cut at 60s — the declared group with its retention, and/or the events of the first invoke, never become observable.

cloudwatch__lambda__logs.cfn-deploy-lambda-missing-logs-policy-rejected: `Error: Check timed out after 60.0s`.
  Expected (AWS): one stack holds two functions — GoodFunction on a role with ManagedPolicyArns [arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole] and BadFunction on a role with the lambda.amazonaws.com trust document and no policies at all. BOTH invoke cleanly (StatusCode 200, no FunctionError); only the good one's stdout reaches CloudWatch Logs, the bad one's group stays empty/absent.
  Actual (emulator): the check never reaches that comparison and is cut at 60s.

Note: the replay detail for both is only the timeout line, so it names no single blocking frame; both observables above are unreached.

## Steps to Reproduce
Replay cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive and cloudwatch__lambda__logs.cfn-deploy-lambda-missing-logs-policy-rejected.

Retention check:
1. CreateStack with AWS::IAM::Role (lambda.amazonaws.com trust + AWSLambdaBasicExecutionRole), AWS::Logs::LogGroup{LogGroupName: "/aws/lambda/<fn>", RetentionInDays: 7}, AWS::Lambda::Function{FunctionName: <fn>, Runtime: python3.12, Handler: index.handler, Code.ZipFile printing one line, DependsOn: the log group}.
2. Poll DescribeStacks to CREATE_COMPLETE.
3. DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<fn>") — expect the group with retentionInDays == 7.
4. Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload={}) — expect StatusCode 200, no FunctionError.
5. Poll FilterLogEvents(logGroupName="/aws/lambda/<fn>") — expect >= 1 event.
6. DescribeLogGroups again — expect retentionInDays still == 7.

Permission check:
1. CreateStack with two roles (one with AWSLambdaBasicExecutionRole, one with only the lambda.amazonaws.com trust document and no ManagedPolicyArns/Policies) and two otherwise identical python3.12 functions.
2. Invoke both — expect StatusCode 200 and no FunctionError from each.
3. Read each function's "/aws/lambda/<fn>" group — expect the granted function's print to be delivered and the ungranted function's group to hold nothing.
