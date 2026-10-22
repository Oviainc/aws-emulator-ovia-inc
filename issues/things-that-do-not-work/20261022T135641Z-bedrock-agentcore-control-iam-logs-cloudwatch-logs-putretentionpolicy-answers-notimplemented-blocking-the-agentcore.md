Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__iam__logs.e2e-bedrock-agentcore-runtime-missing-logs-lifecycle
Issue-Key: bedrock-agentcore-control__iam__logs|PutRetentionPolicy|NotImplemented
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# CloudWatch Logs PutRetentionPolicy answers NotImplemented, blocking the AgentCore runtime/logs lifecycle deploy phase

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRetentionPolicy

## Description
Cause 1 of 1: the deploy phase of the AgentCore runtime/logs lifecycle check cannot set a log-group retention, because CloudWatch Logs PutRetentionPolicy is an unimplemented scaffold in the emulator.
AWS: Smithy (com-amazonaws-cloudwatchlogs-logs-20140328): PutRetentionPolicy input requires `logGroupName` (length 1-512, pattern ^[\.\-_/#A-Za-z0-9]+$) and `retentionInDays`; output is empty (Unit), i.e. a 200 with no body on success. Its only modeled errors are InvalidParameterException, OperationAbortedException, ResourceNotFoundException (all 400) and ServiceUnavailableException (500). After a successful call DescribeLogGroups reports that retentionInDays on the group.
Emulator: the call fails with error code `NotImplemented`; recorded message example: "operation PutRetentionPolicy is scaffolded but not implemented in this emulator". The suite did NOT mark this failure as already filed, and no open issue names logs PutRetentionPolicy.
Why: any client that provisions a log group and then sets its retention (the standard deploy sequence for an AgentCore runtime estate) fails with a code that is not in the model, and no retention is ever recorded, so later reads of retentionInDays are wrong too.
Fix: implement the handler for PutRetentionPolicy in the cloudwatch-logs service (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/) — validate logGroupName/retentionInDays, store retentionInDays on the group state, return the empty 200 output, and raise ResourceNotFoundException for an unknown group.
Traces: bedrock-agentcore-control__iam__logs.e2e-bedrock-agentcore-runtime-missing-logs-lifecycle (fails in phase 1/5 'deploy'). Note: the sibling check bedrock-agentcore-control__iam__logs.agentcore-team-session-list-agent-runtimes-denied-while-granted-twin-lists was marked ALREADY FILED (FilterLogEvents stub) and is deliberately excluded here.

## Evidence
Expected: PutRetentionPolicy(logGroupName=<group>, retentionInDays=7) -> HTTP 200 with empty body; DescribeLogGroups then reports retentionInDays=7. Actual: _PhaseError: [phase 1/5 deploy] ClientError: An error occurred (NotImplemented) when calling the PutRetentionPolicy operation: operation PutRetentionPolicy is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Boot the emulator.
2. `logs = boto3.client('logs'); logs.create_log_group(logGroupName='/aws/bedrock-agentcore/runtimes/test-DEFAULT')`.
3. `logs.put_retention_policy(logGroupName='/aws/bedrock-agentcore/runtimes/test-DEFAULT', retentionInDays=7)` -> NotImplemented instead of an empty 200.
4. Replay bedrock-agentcore-control__iam__logs.e2e-bedrock-agentcore-runtime-missing-logs-lifecycle.
