Issue-Source: trace-sweep-divergence
Issue-Trace: rds-delete-groups-in-use-by-instance
Issue-Step: teardown#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-delete-groups-in-use-by-instance rds-read-replica-and-instance-promotion
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DeleteDBInstance|2xx->4xx|status
Issue-Key: rds|DescribeDBInstances|4xx->2xx|$
Issue-Related: 20261022T092913Z-rds-writes-and-reads-skip-existence-and-parameter.md
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# RDS DeleteDBInstance refuses a newly created instance, which then never disappears

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDBInstance

## Description
Two causes, one fault in the DB-instance state machine.

Cause 1 of 2 — DeleteDBInstance on an instance that has just been created. AWS: the delete is accepted with HTTP 200 and the instance moves to 'deleting'; InvalidDBInstanceStateFault (awsQueryError InvalidDBInstanceState, 400) is modeled for genuinely undeletable states (Smithy: DeleteDBInstance errors DBInstanceNotFoundFault 404 / InvalidDBInstanceStateFault 400). The recording shows 200 for this request. Emulator: HTTP 400 — it keeps the instance in a non-deletable state and refuses. Why: every teardown/cleanup flow that deletes what it created fails, and a client cannot release the instance at all. Fix: the DeleteDBInstance handler's state guard and the instance state machine that holds the instance in 'creating'. Traces: rds-delete-groups-in-use-by-instance#teardown#12 (also rds-read-replica-and-instance-promotion#teardown#10).

Cause 2 of 2 — DescribeDBInstances polled after that delete. AWS: once the delete is accepted the instance is removed, so the describe answers 404 DBInstanceNotFound (Smithy: DescribeDBInstances error DBInstanceNotFoundFault 404). Emulator: answers 200 with DBInstanceStatus 'available' on every poll (7 attempts, poll capped at 60s) because the delete in cause 1 never took effect — the instance is never torn down. Why: delete-then-wait-for-gone is the standard RDS cleanup idiom; it can never terminate here, and the resource leaks into subsequent operations. Fix: once DeleteDBInstance is accepted, the instance must transition to 'deleting' and then be removed from state so reads 404. Traces: rds-delete-groups-in-use-by-instance#teardown#13.

## Evidence
Cause 1: rds-delete-groups-in-use-by-instance#teardown#12 — 'status: expected 200, got 400' (cluster also covers rds-read-replica-and-instance-promotion#teardown#10).
Cause 2: rds-delete-groups-in-use-by-instance#teardown#13 — 'status: expected 404, got 200'; poll: status 200 not in [404] after 7 attempt(s), poll capped at 60s; body still reports DBInstanceStatus 'available' for the identifier that was asked to be deleted.

## Steps to Reproduce
1) Replay the wire corpus (replay_wire_corpus).
2) Fixture rds-delete-groups-in-use-by-instance: run through task# steps so the instance exists, then teardown#12 DeleteDBInstance — expect 200, observe 400.
3) teardown#13: poll DescribeDBInstances for the same identifier — expect 404 DBInstanceNotFound, observe 200 with DBInstanceStatus 'available' until the poll cap.
