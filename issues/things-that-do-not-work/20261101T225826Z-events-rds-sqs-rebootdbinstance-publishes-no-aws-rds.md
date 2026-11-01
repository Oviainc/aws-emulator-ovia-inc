Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives
Issue-Key: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives|_receive#20|Timeout|return [m["Body"] for m in msgs]
Issue-Commit: 82a9e97b43fb811c245308aabae5044dc3bd73ae

# events__rds__sqs: RebootDBInstance publishes no aws.rds `RDS DB Instance Event`, so the exactly-matching rule's positive control never fires and the near-miss withholding claim cannot be graded (1 cause)

> _Note: this issue was identified against the code base at commit `82a9e97b43fb811c245308aabae5044dc3bd73ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootDBInstance

## Description
One net-new root cause, grounded in one failing behavioral check.

Cause 1 of 1: rebooting a DB instance produces no `aws.rds` / `RDS DB Instance Event` envelope on the default bus, so the rule whose pattern matches that event exactly delivers nothing to its SQS target. The check's positive control therefore fails, and the three near-miss rules (one field off: wrong EventID, wrong SourceArn, snapshot detail-type) can never be shown to be silent because of their pattern rather than because nothing was published at all. Failing check: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives.

AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19): RebootDBInstance takes DBInstanceIdentifier (required) and optional ForceFailover, returns a DBInstance, and models DBInstanceNotFoundFault (404, awsQueryError code DBInstanceNotFound), InvalidDBInstanceStateFault (400, InvalidDBInstanceState) and KMSKeyNotAccessibleFault (400). The model does not describe EventBridge emission, so for the eventing half this is recorded behavior only: a reboot of an available instance causes RDS to publish an envelope on the default bus with "source": "aws.rds" and "detail-type": "RDS DB Instance Event" (both fixed text), whose detail.EventID is the RDS event identifier for the transition — RDS-EVENT-0006 for "DB instance restarted" — whose detail.SourceArn is the ARN of the rebooted instance (arn:<partition>:rds:<region>:<account>:db:<DBInstanceIdentifier>) and whose detail.Date is the event's timestamp, at or after the reboot call. A rule with EventPattern {"source":["aws.rds"],"detail-type":["RDS DB Instance Event"],"detail":{"EventID":["RDS-EVENT-0006"],"SourceArn":[<that instance ARN>]}} therefore delivers it to its SQS target; EventBridge pattern matching is exact, so a rule differing in any one of those three fields receives nothing from the same publication. All values come from the request and the service's state, never a literal from one run.

Emulator: no envelope is ever delivered. The replay reports only `Check timed out after 60.0s`: after creating the instance and waiting for it to be available, the check reboots it and polls the matching rule's queue for an RDS-EVENT-0006 event dated at/after the reboot — that poll never succeeds, so the check is still polling when the harness bound expires and never reaches its near-miss assertions or the repair leg (where the near-miss rules are rewritten to the matching pattern and must then receive after a second reboot). No error code or non-2xx status is involved: RebootDBInstance answers 200 and the divergence is the absent event.

Why: a client cannot observe RDS instance lifecycle through EventBridge. Every reboot/failover notification, runbook trigger and availability alarm keyed on `RDS DB Instance Event` with a specific EventID stays silent, and because nothing is ever published, a client also cannot tell a correctly-withheld near-miss (precise pattern matching, which is the behavior AWS guarantees) from a pipeline that publishes nothing — both look identical.

Fix: the rds crate's DB-instance lifecycle/event publication — the component that must emit an `aws.rds` / `RDS DB Instance Event` envelope with detail.EventID, detail.SourceArn and detail.Date when an instance transition (here, RebootDBInstance -> restarted, RDS-EVENT-0006) occurs — and, downstream of it, the EventBridge rule -> SQS target path for aws.rds envelopes.

Related but NOT covering this check: the open issue 20261029T080433Z-events-rds-sqs-rebootdbinstance-still-publishes-no-aws.md owns the sibling events__rds__sqs checks (raw SQS target and InputTransformer) for this same absence; it does not carry this check's replay key, which is why the near-miss check is filed here.

Traces: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives.

## Evidence
Expected (AWS): with four rules on the default bus each targeting its own SQS queue — one exactly matching {"source":["aws.rds"],"detail-type":["RDS DB Instance Event"],"detail":{"EventID":["RDS-EVENT-0006"],"SourceArn":[<db ARN>]}} and three near-misses differing in exactly one field (EventID RDS-EVENT-0087; SourceArn pointing at a nonexistent instance; detail-type "RDS DB Snapshot Event") — a RebootDBInstance on the available instance delivers one RDS-EVENT-0006 envelope dated at/after the reboot to the matching queue, while each near-miss queue reports ApproximateNumberOfMessages "0" and ApproximateNumberOfMessagesNotVisible "0"; after PutRule rewrites each near-miss to the matching pattern and the instance is rebooted again, each of those same queues receives an envelope whose detail.SourceArn == the db ARN and detail.EventID == "RDS-EVENT-0006".

Actual (emulator): the matching queue never receives anything after the reboot, so the positive control fails and no near-miss or repair assertion is ever reached. The suite's only observable is `Error: Check timed out after 60.0s`. RebootDBInstance itself answers 200 — the divergence is that no aws.rds event is published.

## Steps to Reproduce
1. Create the exactly-matching EventBridge rule on the default bus with EventPattern {"source":["aws.rds"],"detail-type":["RDS DB Instance Event"],"detail":{"EventID":["RDS-EVENT-0006"],"SourceArn":["arn:<partition>:rds:<region>:<account>:db:<db-id>"]}} and target it at an SQS queue whose policy admits events.amazonaws.com.
2. Create three sibling rules with the same pattern but one field changed each (EventID RDS-EVENT-0087; SourceArn of a nonexistent instance; detail-type "RDS DB Snapshot Event"), each with its own SQS target.
3. Create a db.t4g.micro DB instance named <db-id> and wait until it is available.
4. Call rds RebootDBInstance(DBInstanceIdentifier=<db-id>) and note the response Date header as the cutoff.
5. Poll the matching rule's queue for an envelope with detail.EventID == "RDS-EVENT-0006" and detail.Date >= cutoff.
6. Observe that step 5 never succeeds: no aws.rds `RDS DB Instance Event` is ever published, so the check polls until the harness bound (60 s) expires and the near-miss/repair legs never run.
