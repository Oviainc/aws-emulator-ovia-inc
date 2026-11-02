Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs__logs.logs-subscription-filter-without-permission-does-not-deliver
Issue-Key: cloudwatch__ecr__ecs__logs.logs-subscription-filter-without-permission-does-not-deliver|run#17|AssertionError|assert not _poll_stream_for_token(ctx, bad_stream_name, bad_stream_token, timeout_s=0), (
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# A CloudWatch Logs subscription filter delivers to Kinesis through a role with no kinesis:PutRecord permission (1 cause)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1 — subscription-filter delivery ignores the roleArn's permissions, so events reach the destination stream that the role may not write to.
AWS: Smithy (com.amazonaws.cloudwatchlogs) PutSubscriptionFilter, members logGroupName, filterName, filterPattern, destinationArn and roleArn. Recorded behavior only for the delivery semantics: CloudWatch Logs assumes the roleArn to put records into the destination Kinesis stream, so when that role's policy does not allow kinesis:PutRecord/PutRecords on the stream, delivery fails and NO log event appears in the stream; the filter itself stays in place and the log group keeps accepting events.
Emulator: delivers the log event into the Kinesis stream anyway, through a role that holds no kinesis:PutRecord permission.
Why: the delivery role is the entire authorization boundary for cross-service log fan-out. Delivering without it means an emulated environment cannot show that a missing grant stops delivery — the exact failure mode operators debug — and makes any least-privilege test pass vacuously, so a role that is in fact unauthorized looks correct.
Fix: evaluate the subscription filter's roleArn against the destination before delivering, in the subscription-filter delivery path of the logs crate (the destination dispatcher that writes to Kinesis).
Traces: cloudwatch__ecr__ecs__logs.logs-subscription-filter-without-permission-does-not-deliver

One cause must be closed before submitting.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

logs-subscription-filter-without-permission-does-not-deliver:
  AssertionError: log event was delivered to Kinesis via a role (arn:aws:iam::<account>:role/<role name>) with no kinesis:PutRecord permission — the subscription filter should have failed to deliver

The role name in that message is this run's value; the requirement is that no record reaches the stream while the role lacks the grant.

## Steps to Reproduce
Boot the emulator from this checkout.

1. aws kinesis create-stream --stream-name s --shard-count 1
2. Create an IAM role trusted by logs.amazonaws.com with NO kinesis:PutRecord/PutRecords permission.
3. aws logs create-log-group --log-group-name g ; aws logs put-subscription-filter --log-group-name g --filter-name f --filter-pattern '' --destination-arn <stream arn> --role-arn <role arn>
4. aws logs put-log-events --log-group-name g --log-stream-name s1 --log-events timestamp=<now>,message=hello
5. aws kinesis get-records on the stream's shard iterator -> the record is present. AWS delivers nothing until the role is granted kinesis:PutRecord.

Then re-replay: cloudwatch__ecr__ecs__logs.logs-subscription-filter-without-permission-does-not-deliver
