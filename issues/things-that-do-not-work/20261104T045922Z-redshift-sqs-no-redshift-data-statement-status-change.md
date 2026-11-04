Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.cli-redshift-data-execute-statement-eventbridge-sqs-notify redshift__sqs.cli-redshift-data-failed-statement-error-event-to-sqs-positive redshift__sqs.cli-sqs-receive-redshift-completion-message-positive redshift__sqs.cli-eventbridge-put-rule-wrong-target-arn-negative redshift__sqs.cli-sqs-purge-queue-then-no-stale-redshift-event-negative
Issue-Key: redshift__sqs.cli-redshift-data-execute-statement-eventbridge-sqs-notify|_poll_for_statement_message#10|Timeout|time.sleep(interval_s)
Issue-Key: redshift__sqs.cli-redshift-data-failed-statement-error-event-to-sqs-positive|_poll_for_statement_message#11|Timeout|time.sleep(interval_s)
Issue-Key: redshift__sqs.cli-sqs-receive-redshift-completion-message-positive|_poll_for_statement_message_cli#11|Timeout|time.sleep(interval_s)
Issue-Key: redshift__sqs.cli-eventbridge-put-rule-wrong-target-arn-negative|_poll_for_statement_message#17|Timeout|time.sleep(interval_s)
Issue-Key: redshift__sqs.cli-sqs-purge-queue-then-no-stale-redshift-event-negative|_poll_for_statement_message#11|Timeout|time.sleep(interval_s)
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# redshift__sqs: no 'Redshift Data Statement Status Change' event ever reaches an SQS target, so every CLI eventing workflow (positive controls included) never completes (1 cause)

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Cause 1 of 1 — a Redshift Data API statement run with event emission enabled publishes no EventBridge event, so an EventBridge rule targeting an SQS queue never delivers anything and every workflow built on that delivery stalls. AWS: `redshift-data execute-statement --with-event` makes the service publish, on each statement state change, an event with source 'aws.redshift-data', detail-type 'Redshift Data Statement Status Change' and a detail carrying statementId and state (FINISHED or FAILED, with the engine's error text on failure) plus a resources entry naming the cluster ("cluster:<cluster-id>"); a rule matching that source/detail-type with an SQS target delivers the event to the queue within seconds, and PurgeQueue only removes what was already enqueued (recorded behavior only — the event envelope is not part of the Redshift Smithy model). Emulator: the statement itself is accepted and reaches a terminal state, but no event is ever published, so the SQS poll never returns a message. Each affected check spends its delivery poll (180 s for the positive control, 60 s for the absence probe) and is killed by the harness ceiling at 60 s without ever reaching its assertions — including the two negative checks, whose positive control (a correctly-wired target receiving the event) must land before the absence claim can be made. Why: no client can observe statement completion asynchronously — completion fan-out, failure alerting, dead-letter wiring and even the negative controls that prove a mis-wired target gets nothing are all unobservable, and a consumer waiting on the queue hangs indefinitely. Fix: the Redshift Data API statement lifecycle must publish the statement-status-change event to EventBridge (source, detail-type, detail.statementId/state, resources naming the cluster) when event emission is requested, and the EventBridge->SQS delivery path must carry it to the target queue. Traces: redshift__sqs.cli-redshift-data-execute-statement-eventbridge-sqs-notify, redshift__sqs.cli-redshift-data-failed-statement-error-event-to-sqs-positive, redshift__sqs.cli-sqs-receive-redshift-completion-message-positive, redshift__sqs.cli-eventbridge-put-rule-wrong-target-arn-negative, redshift__sqs.cli-sqs-purge-queue-then-no-stale-redshift-event-negative. Related open issues 20261024T115115Z-redshift-sqs-redshift-data-api-executestatement-with-withevent-true-publishes.md and 20261029T082153Z-redshift-sqs-a-data-api-statement-submitted-with.md record the same missing publication from other recorded units; neither carries any of the five checks above, which exercise the CLI workflow end to end (rule + queue policy + target + statement + receive).

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator. All five checks end as "Error: Check timed out after 60.0s".

Each check performs the same sequence with the aws CLI and the SDK: create a cluster and wait for 'available'; PutRule for source 'aws.redshift-data' / detail-type 'Redshift Data Statement Status Change'; create the SQS queue and set a queue policy allowing the rule's ARN; PutTargets (FailedEntryCount asserted 0); run the statement; wait for its status; then poll the queue for a message whose detail.statementId equals the submitted statement id (timeout_s=180.0 in the positive controls). The poll never returns a message, so the assertions that follow it — "no EventBridge notification for the statement reached SQS", source == 'aws.redshift-data', detail-type == 'Redshift Data Statement Status Change', detail.statementId == <statement id>, detail.state == 'FINISHED', a resources entry containing "cluster:<cluster-id>" — are never reached. The two negative checks (cli-eventbridge-put-rule-wrong-target-arn-negative, cli-sqs-purge-queue-then-no-stale-redshift-event-negative) block on the SAME positive control ("positive control: correctly-wired target never got the event") before they can assert the intended/purged queue stays empty.

Other checks in this run create clusters and run Data API statements to completion against this emulator, so the stall is the absent event delivery, not cluster provisioning. Statement ids, queue names, rule names and cluster ids are per-run values; the requirement is that the event be published and delivered with the envelope and detail described above.

## Steps to Reproduce
1. Create a Redshift cluster and wait for 'available'.
2. `aws events put-rule` for source 'aws.redshift-data' / detail-type 'Redshift Data Statement Status Change'; create an SQS queue, set its queue policy to allow the rule ARN, and `aws events put-targets` with the queue ARN (FailedEntryCount is 0).
3. `aws redshift-data execute-statement --with-event` with 'SELECT 1' on that cluster and wait until DescribeStatement reports FINISHED.
4. Poll `aws sqs receive-message` on the queue: no message with detail.statementId == the statement id ever arrives (poll for 180 s).
5. Repeat with a statement that fails to confirm the FAILED-state event is equally absent, and with a second, deliberately mis-wired target to see that the positive control never lands either.
6. Re-replay the behavioral checks listed in trace_ids.
