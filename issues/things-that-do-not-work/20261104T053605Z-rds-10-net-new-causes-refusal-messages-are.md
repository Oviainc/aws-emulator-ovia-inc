Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.create-cluster-backup-retention-out-of-range-rejected rds.cluster-snapshot-restore-share-over-quota-rejected rds.cluster-snapshot-shared-attribute-quota-exceeded-and-revocation-rejected rds.concurrent-create-db-proxy-duplicate-name-single-winner rds.concurrent-create-same-snapshot-name-conflict rds.delete-instance-final-snapshot-name-conflict-error rds.concurrent-failover-requests-single-execution rds.delete-cluster-with-members rds.delete-global-cluster-with-member rds.global-cluster-delete-with-member-rejected rds.tag-key-reserved-aws-prefix-rejected rds.describe-proxy-target-groups rds.delete-tenant-database-boundary
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.create-cluster-backup-retention-out-of-range-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: rds.cluster-snapshot-restore-share-over-quota-rejected|run#15|AssertionError|assert message == expected, f"unexpected message: {message!r} (expected {expected!r})"
Issue-Key: rds.cluster-snapshot-shared-attribute-quota-exceeded-and-revocation-rejected|run#16|AssertionError|assert message == expected, f"unexpected message: {message!r} (expected {expected!r})"
Issue-Key: rds.concurrent-create-db-proxy-duplicate-name-single-winner|run#37|AssertionError|assert message in expected, f"unexpected loser message: {message!r} (expected one of {expected!r})"
Issue-Key: rds.concurrent-create-same-snapshot-name-conflict|run#15|AssertionError|assert failures[0]["message"] == (
Issue-Key: rds.delete-instance-final-snapshot-name-conflict-error|run#14|AssertionError|assert msg == (
Issue-Key: rds.concurrent-failover-requests-single-execution|run#39|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: rds.delete-cluster-with-members|run#16|AssertionError|assert message == (
Issue-Key: rds.delete-global-cluster-with-member|run#18|AssertionError|assert message == f"Global Cluster arn:aws:rds::{account}:global-cluster:{global_id} is not empty", message
Issue-Key: rds.global-cluster-delete-with-member-rejected|run#17|AssertionError|assert err.get("Message") == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Key: rds.tag-key-reserved-aws-prefix-rejected|run#10|AssertionError|assert error["Message"] == 'Tag keys cannot start with the reserved prefix "aws:".', (
Issue-Key: rds.describe-proxy-target-groups|run#17|AssertionError|assert err2.get("Message") == f"Requested resource not found - TargetGroup: {bogus_group}", (
Issue-Key: rds.delete-tenant-database-boundary|run#14|AssertionError|assert wrong_arch_message == (
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds: 10 net-new causes — refusal messages are the emulator's own prose instead of RDS's fixed sentences

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Ten refusal messages whose fixed text does not match AWS. In each case the HTTP status and (unless noted) the error code are right; the message is re-authored, so a client that surfaces or matches on it diverges. For each, the fixed text is quoted and the variable parts are named; the ids/names/numbers below are template slots filled from the request or the emulator's state, never literals to copy.

Cause 1 of 10: BackupRetentionPeriod out of range. AWS words it 'Invalid backup retention period: <value>. Retention period must be between 1 and 35.' — fixed text plus the value from the request and the engine's fixed 1..35 bounds. Emulator: 'invalid value 40 for BackupRetentionPeriod: must be a value from 1 to 35'. Check: rds.create-cluster-backup-retention-out-of-range-rejected.

Cause 2 of 10: sharing a manual snapshot past the account quota. AWS words it 'A snapshot cannot be shared with more than 20 customers.' — entirely fixed text (the 20 is AWS's documented limit). Emulator: 'a manual DB cluster snapshot cannot be shared with more than 20 accounts'. Checks: rds.cluster-snapshot-restore-share-over-quota-rejected, rds.cluster-snapshot-shared-attribute-quota-exceeded-and-revocation-rejected.

Cause 3 of 10: duplicate DB proxy name. AWS words it "The DBProxy '<name>' already exists" (quoted name from the request). Emulator: 'A DB proxy named <name> already exists'. Check: rds.concurrent-create-db-proxy-duplicate-name-single-winner.

Cause 4 of 10: duplicate DB snapshot identifier (code DBSnapshotAlreadyExists is correct). AWS words it 'Cannot create the snapshot because a snapshot with the identifier <id> already exists.' Emulator: 'DB snapshot already exists: <id>'. Checks: rds.concurrent-create-same-snapshot-name-conflict, rds.delete-instance-final-snapshot-name-conflict-error.

Cause 5 of 10: failover requested while a failover is in flight. AWS words it 'DB cluster:<id> cannot failover in state:<status>' — fixed text with the cluster id and the current status from state. Emulator: 'DB cluster <id> is not available for a failover (status failing-over)'. Check: rds.concurrent-failover-requests-single-execution.

Cause 6 of 10: deleting a cluster that still has members. AWS words it 'Cluster cannot be deleted, it still contains DB instances in non-deleting state.' Emulator: "DB cluster <id> can't be deleted because it still has 1 DB instance(s): <instance id>. Delete them first" — it also leaks the member list AWS does not include. Check: rds.delete-cluster-with-members.

Cause 7 of 10: deleting a global cluster that still has members. AWS words it 'Global Cluster <global cluster ARN> is not empty' — fixed text with the ARN from state. Emulator: "Global cluster <id> can't be deleted because it still has 1 member cluster(s): detach them first". Checks: rds.delete-global-cluster-with-member, rds.global-cluster-delete-with-member-rejected.

Cause 8 of 10: a tag key using the reserved 'aws:' prefix. AWS words this InvalidParameterValue as 'Tags cannot be null or have a key prefixed with aws:' (fixed text). Emulator: "Tags.Key '<key>' is prefixed with 'aws:', which Amazon RDS reserves". Check: rds.tag-key-reserved-aws-prefix-rejected.

Cause 9 of 10: DBProxyTargetGroupNotFoundFault. AWS words it 'Target group <name> not found for DB proxy <proxy>.' Emulator appends its own explanation: 'DBProxyTargetGroup <name> not found for DB proxy <proxy>: a proxy has one target group, named default'. Check: rds.describe-proxy-target-groups.

Cause 10 of 10: tenant-database boundary on a non-multi-tenant instance. AWS refuses with the modeled not-found/InvalidParameterValue sentence for the tenant database; the emulator substitutes its own explanation 'DB instance <id> runs engine postgres and does not use the multi-tenant configuration, so it holds no tenant databases'. Check: rds.delete-tenant-database-boundary.

Why: the message is part of the contract — clients log it, assert on it and parse limits out of it; re-worded text breaks message matching and misstates the rule (cause 6 even discloses member identifiers AWS withholds). Fix: the error-message construction in each operation's handler/validation in the rds crate (create_db_cluster.rs, modify_db_cluster_snapshot_attribute.rs, create_db_proxy.rs, create_db_snapshot.rs/delete_db_instance.rs, failover_db_cluster.rs, delete_db_cluster.rs, delete_global_cluster.rs, the shared tag validator, describe_db_proxy_target_groups.rs, delete_tenant_database.rs). Related open issues 20261103T043005Z-rds-7-net-new-root-causes-refusal-messages.md and 20261102T074545Z-rds-create-path-refusal-messages-use-emulator-wording.md cover different messages and do not carry these replay keys.

## Evidence
Verbatim replay output:
- "expected message 'Invalid backup retention period: 40. Retention period must be between 1 and 35.', got 'invalid value 40 for BackupRetentionPeriod: must be a value from 1 to 35'".
- "unexpected message: 'a manual DB cluster snapshot cannot be shared with more than 20 accounts' (expected 'A snapshot cannot be shared with more than 20 customers.')" (x2).
- "unexpected loser message: 'A DB proxy named <name> already exists' (expected one of (\"The DBProxy '<name>' already exists\", \"DBProxy '<name>' already exists\"))".
- "{'ok': False, 'code': 'DBSnapshotAlreadyExists', 'message': 'DB snapshot already exists: <id>'}" and 'AssertionError: DB snapshot already exists: <id>'.
- "expected message 'DB cluster:<id> cannot failover in state:failing-over', got 'DB cluster <id> is not available for a failover (status failing-over)'".
- "unexpected message: \"DB cluster <id> can't be deleted because it still has 1 DB instance(s): <instance>. Delete them first\"".
- "expected message 'Global Cluster arn:aws:rds::<account>:global-cluster:<id> is not empty', got \"Global cluster <id> can't be deleted because it still has 1 member cluster(s): detach them first\"".
- "unexpected message \"Tags.Key 'aws:<suffix>' is prefixed with 'aws:', which Amazon RDS reserves\"".
- "unexpected DBProxyTargetGroupNotFoundFault message: {'Code': 'DBProxyTargetGroupNotFoundFault', 'Message': 'DBProxyTargetGroup <name> not found for DB proxy <proxy>: a proxy has one target group, named default'}".
- 'DB instance <id> runs engine postgres and does not use the multi-tenant configuration, so it holds no tenant databases'.

## Steps to Reproduce
1. create_db_cluster(BackupRetentionPeriod=40).
2. modify_db_cluster_snapshot_attribute(AttributeName='restore', ValuesToAdd=[21 account ids]).
3. Two concurrent create_db_proxy calls with the same DBProxyName; read the loser's message.
4. Two concurrent create_db_snapshot calls with the same DBSnapshotIdentifier; and delete_db_instance(FinalDBSnapshotIdentifier=<existing snapshot id>).
5. failover_db_cluster twice in a row on the same cluster.
6. delete_db_cluster on a cluster that still has a member instance.
7. delete_global_cluster on a global cluster that still has a member.
8. add_tags_to_resource(Tags=[{'Key':'aws:<x>','Value':'v'}]).
9. describe_db_proxy_target_groups(DBProxyName=<proxy>, TargetGroupName=<name that does not exist>).
10. delete_tenant_database on a non-multi-tenant instance.
