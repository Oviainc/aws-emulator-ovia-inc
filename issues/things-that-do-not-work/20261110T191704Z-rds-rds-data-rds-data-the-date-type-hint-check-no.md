Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data.date-type-hint-rejects-unparseable-date-format
Issue-Key: rds__rds-data.date-type-hint-rejects-unparseable-date-format|_poll#4|Timeout|time.sleep(interval_s)
Issue-Commit: 122d2e23ed306dec374f1768a21e001304c75e9d

# rds-data: the DATE type-hint check no longer reaches its assertion — the Aurora/Data API provisioning chain never converges and the check is killed at 60 s

> _Note: this issue was identified against the code base at commit `122d2e23ed306dec374f1768a21e001304c75e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
1 net-new root cause, and it is a NEW failure point for a check that previously got much further.

Note on overlap: open issue 20261109T135215Z-rds-rds-data-rds-data-3-net-new-causes-date-type.md lists this same check id, but under the replay key `rds__rds-data.date-type-hint-rejects-unparseable-date-format|_refused#4|AssertionError|assert _message(exc) == message` — the ExecuteStatement DATE refusal message. The suite does not mark this failure as OWNED, because the check no longer reaches that assertion at all: it is now killed during provisioning. That earlier cause is not re-filed here; this issue is only about the flow never getting that far.

Cause 1 of 1: the Aurora + Data API provisioning chain this check needs never reaches its terminal statuses, so no ExecuteStatement is ever issued.
AWS: a DB cluster converges to Status `available` and its member DB instance to DBInstanceStatus `available` on their own, the cluster's managed admin-user secret becomes retrievable, and the Data API endpoint then answers ExecuteStatement — a client polls DescribeDBClusters / DescribeDBInstances until it sees those terminal values and then calls the Data API. For the statement itself the modeled behaviour is the one the overlapping issue already describes: a parameter carrying typeHint DATE whose stringValue is not a parseable date is refused BadRequestException with the fixed message `Parse Error for Date: input contains invalid characters` (fixed text, no request value interpolated).
Emulator: the check is killed with `Check timed out after 60.0s` while still polling the provisioning chain (cluster available → member available → secret retrievable → Data API reachable, polled at 15–20 s intervals); no ExecuteStatement response — correct or not — is ever produced. Hypothesis: the same deferred-transition machinery as the rds control plane (`PendingStatus::in_a_moment` scheduling each change `STATUS_TRANSITION_DELAY` ahead and `Store::settle_due_transitions` materialising it only when a later request arrives, services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs:507) makes each transition cost a whole client poll interval, so a four-transition chain cannot finish inside the budget. The model and the recordings state the statuses, not this latency, hence Hypothesis.
Why: a client that provisions an Aurora cluster and then uses the Data API against it can never observe the cluster and its member become usable, so the whole Data API surface is unreachable behind the provisioning wait — and the DATE refusal the overlapping issue asks for cannot be verified either.
Fix: the pending-status schedule and settling of DB-cluster and member DB-instance records in the rds state store (and the managed-secret/Data-API readiness that follows them), not the rds-data statement path.
Traces: rds__rds-data.date-type-hint-rejects-unparseable-date-format.

## Evidence
Failing check detail from the behavioral suite: `Error: Check timed out after 60.0s`.

Expected (AWS): DescribeDBClusters reports Status `available` and DescribeDBInstances reports DBInstanceStatus `available` for the member, the managed admin secret is retrievable, and rds-data ExecuteStatement then answers — for this check, by refusing the unparseable DATE parameter with BadRequestException and the fixed message `Parse Error for Date: input contains invalid characters`.

Actual (emulator): the check never leaves provisioning; it is killed at 60 s while polling (check constants `_MEMBER_WAIT_S=1200`, `_CLUSTER_WAIT_S=600`, `_SECRET_WAIT_S=300`, `_DATA_API_WAIT_S=300`, poll interval 15 s). No ExecuteStatement call is made, so no status code, error code or message is produced at all.

The same check id appears in open issue 20261109T135215Z-rds-rds-data-rds-data-3-net-new-causes-date-type.md under a different replay key (the DATE message assertion), which the check can no longer reach.

## Steps to Reproduce
Replay rds__rds-data.date-type-hint-rejects-unparseable-date-format against the emulator:

1. CreateDBCluster (Aurora, EnableHttpEndpoint for the Data API, managed master user secret) plus a member CreateDBInstance.
2. Poll DescribeDBClusters for Status `available` and DescribeDBInstances for DBInstanceStatus `available` (15 s interval).
3. Wait for the cluster's managed admin-user secret to be retrievable, then for the Data API to answer.
4. ExecuteStatement with parameters=[{name:'d', value:{stringValue:'<not a date>'}, typeHint:'DATE'}] and read the BadRequestException message.

The run is killed at the 60 s per-check budget during step 2/3, so step 4 is never reached.
