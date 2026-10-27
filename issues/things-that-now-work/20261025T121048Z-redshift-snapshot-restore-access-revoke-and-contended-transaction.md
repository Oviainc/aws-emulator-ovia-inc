Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.snapshot-duplicate-and-ungranted-revoke-fail redshift.redshift-two-concurrent-transactions-on-the-same-row-serialize-to-one-winner
Issue-Key: redshift.snapshot-duplicate-and-ungranted-revoke-fail|run#19|AssertionError|assert revoke_code == "AccessToSnapshotDenied", (
Issue-Key: redshift.redshift-two-concurrent-transactions-on-the-same-row-serialize-to-one-winner|_wait_until#28|Timeout|time.sleep(interval_s)
Issue-Commit: 4bcd3873f5a87e761a7b51338711025fa6d95a34

# Redshift snapshot restore-access revoke and contended-transaction isolation are ungraded: RevokeSnapshotAccess answers NotImplemented instead of AccessToSnapshotDenied, and the Data API transaction path never reaches a terminal decision (2 root causes)

> _Note: this issue was identified against the code base at commit `4bcd3873f5a87e761a7b51338711025fa6d95a34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RevokeSnapshotAccess

## Description
Two net-new root causes in the `redshift` service, both found by behavioral replay (no wire fixture can grade either: one is an authorization-specific refusal that depends on prior grant state, the other only exists as the SQL-visible outcome of two interleaved transactions).

Related open issues that do NOT cover these: `20261024T115115Z-redshift-revokesnapshotaccess-stub.md` (filed as "implement the stub"; it names the operation but states no behavioral contract for the ungranted-account case, which is what Cause 1 specifies), `20261024T024248Z-redshift-cluster-sql-engine-rejects-core-redshift-statements.md` and `20261022T180846Z-redshift-data-statement-execution-engine-is-not-redshift.md` (both about statements being REJECTED as syntax errors, not about a transaction never terminating, which is Cause 2).

=== Cause 1 of 2: RevokeSnapshotAccess for an account that holds no grant answers NotImplemented instead of AccessToSnapshotDenied ===
Failing check: redshift.snapshot-duplicate-and-ungranted-revoke-fail

AWS: Smithy model `com.amazonaws.redshift#RevokeSnapshotAccess` declares `AccessToSnapshotDeniedFault` with `awsQueryError` code `AccessToSnapshotDenied` and HTTP 400 (the modeled error list is AccessToSnapshotDeniedFault 400, AuthorizationNotFoundFault 404, ClusterSnapshotNotFoundFault 404, UnsupportedOperationFault 400). Smithy doc for `AccessToSnapshotDeniedFault`: "The owner of the specified snapshot has not authorized your account to access the snapshot." So when `SnapshotIdentifier` names a snapshot that EXISTS but `AccountWithRestoreAccess` holds no grant on it, the refusal is the authorization-specific AccessToSnapshotDenied (HTTP 400) — explicitly NOT a not-found, because the snapshot resolves. The error message's fixed text is AWS's AccessToSnapshotDenied wording; its variable parts are the snapshot identifier and the account id taken from the request. AWS also requires the refusal be scoped: a denied revoke of an ungranted account must leave every other account's `AccountsWithRestoreAccess` grant on that snapshot untouched, and must not add the ungranted account.

Emulator: `RevokeSnapshotAccess` is an unimplemented scaffold stub that answers HTTP 501 `NotImplemented` for every input, so it never distinguishes the ungranted account from the granted one. Replay: the check's earlier legs pass (CreateClusterSnapshot correctly refuses a reused identifier with `ClusterSnapshotAlreadyExists` and does not replace the snapshot; AuthorizeSnapshotAccess grants access and the grant is visible on a subsequent DescribeClusterSnapshots as `AccountsWithRestoreAccess`), then the revoke of the never-granted account yields `AssertionError: expected AccessToSnapshotDenied for revoking an ungranted account, got 'NotImplemented'`. Because the same stub refuses the GRANTED account too, the whole revoke half of the snapshot-sharing lifecycle is unobservable: no caller can ever remove a restore grant.

Why: a client that shares a snapshot cross-account cannot un-share it at all, and error handling inverts — code that catches AccessToSnapshotDenied (a 400 client refusal it is expected to surface as "that account was never granted access") instead sees a 501, which SDK retry policies and most clients treat as a server-side/unsupported condition rather than a caller error. Cleanup and least-privilege flows that revoke grants silently never take effect while reporting a non-client fault.

Fix: implement the operation's handler and its state transition — resolve the snapshot (ClusterSnapshotNotFound when it does not exist), look the requested account up in that snapshot's restore-access set, refuse with AccessToSnapshotDeniedFault when it is absent without mutating the set, and on success remove only that account and return the updated `Snapshot` in the output. Handler file cited under Traces.

=== Cause 2 of 2: a contended multi-statement Data API transaction never reaches a terminal decision, so serializable isolation is ungraded ===
Failing check: redshift.redshift-two-concurrent-transactions-on-the-same-row-serialize-to-one-winner

AWS: recorded behavior only — the Smithy model says nothing about isolation. Real Redshift runs each `BatchExecuteStatement` array serially inside ONE transaction, and its serializable isolation guarantees that two transactions which each read the same row and then write a value derived from that snapshot read cannot both commit a stale-derived value: every submitted transaction reaches a terminal status (`FINISHED`, or `FAILED`/`ABORTED` carrying Redshift error 1023, the serializable-isolation violation), the contended counter ends equal to the number that committed, the surviving value belongs to a committed writer, and the row count is unchanged. Serializing by queueing and serializing by aborting are both conforming; never terminating is not.

Emulator: the check never completes. Replay detail: `Error: Check timed out after 60.0s` — the emulator produced no terminal statement status for the two concurrent transactions within the harness budget, while sibling redshift control-plane checks against the same emulator (including Cause 1's, which provisions a cluster and waits for `available`) do complete inside it. Hypothesis: the Data API statement path does not terminate for a multi-statement `BatchExecuteStatement` under concurrent submission against the same table — either the two batches deadlock against each other or `DescribeStatement` never leaves a non-terminal status, so the poll loop cannot exit. The net effect for this behavior is the same either way: the lost-update invariant (both transactions report success while the counter reads 1) is not merely wrong, it is unobservable.

Why: a client that uses `BatchExecuteStatement` for a read-modify-write — the only Data API path that gives it a multi-statement transaction — hangs instead of getting a decision. Code that polls `DescribeStatement` to a terminal status spins until its own deadline and cannot tell a slow commit from a lost one, so retry-on-1023 logic (the documented way to handle a serialization conflict on Redshift) is never exercised and silently never triggers.

Fix: the Data API statement execution engine and its transaction/locking layer — concurrent batches on the same row must each be driven to a terminal statement status, with a refused one reported as the serializable-isolation failure rather than left pending. Note this failure is a wall-clock timeout with no captured emulator response, so the first step is to establish WHICH call stops answering before changing behavior.

Traces: redshift.snapshot-duplicate-and-ungranted-revoke-fail (stub at services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/revoke_snapshot_access.rs:170, which returns `Scaffolded::not_implemented("RevokeSnapshotAccess")`); redshift.redshift-two-concurrent-transactions-on-the-same-row-serialize-to-one-winner.

## Evidence
Cause 1 — redshift.snapshot-duplicate-and-ungranted-revoke-fail
Expected (AWS, per Smithy `com.amazonaws.redshift#RevokeSnapshotAccess`): RevokeSnapshotAccess on an existing snapshot for an account with no grant -> HTTP 400, error code `AccessToSnapshotDenied` (AccessToSnapshotDeniedFault), message AWS's "owner of the specified snapshot has not authorized your account to access the snapshot" wording with the snapshot id and account id filled in from the request; the snapshot's AccountsWithRestoreAccess set is unchanged (the ungranted account is not added, the separately granted account keeps its grant); a subsequent RevokeSnapshotAccess for the account that IS granted succeeds and removes exactly that account.
Actual (emulator): HTTP 501, error code `NotImplemented`, for every input. Replay assertion:
  File "check_redshift_snapshot_duplicate_and_ungranted_revoke_fail.py", line 293, in run
    assert revoke_code == "AccessToSnapshotDenied", (
  AssertionError: expected AccessToSnapshotDenied for revoking an ungranted account, got 'NotImplemented'
Source of the 501: services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/revoke_snapshot_access.rs:170 — `Err(crate::scaffolded::Scaffolded::not_implemented("RevokeSnapshotAccess"))`, with the file still carrying the `//! SCAFFOLDED, NOT IMPLEMENTED` marker. The modeled error enum in that same file already declares AccessToSnapshotDeniedFault, AuthorizationNotFoundFault, ClusterSnapshotNotFoundFault and UnsupportedOperationFault, so no model change is needed.
Positive controls in the same check that PASSED, scoping the defect to the revoke: CreateClusterSnapshot with a reused SnapshotIdentifier -> `ClusterSnapshotAlreadyExists` and the existing snapshot's SnapshotCreateTime unchanged; AuthorizeSnapshotAccess -> grant visible in AccountsWithRestoreAccess on an independent DescribeClusterSnapshots.

Cause 2 — redshift.redshift-two-concurrent-transactions-on-the-same-row-serialize-to-one-winner
Expected (recorded behavior only; isolation is unmodeled): each of the two concurrently submitted BatchExecuteStatement transactions reaches a terminal status; at least one is FINISHED; `SELECT n FROM <table> WHERE id = 1` equals the number of committed transactions (lower = lost update, higher = a rolled-back write survived); the surviving `v` belongs to a committed writer; `SELECT count(*)` is still 1; any FAILED/ABORTED transaction's Error carries Redshift error 1023 (serializable isolation violation).
Actual (emulator): no terminal decision was ever observed. Replay detail: `Error: Check timed out after 60.0s`. No HTTP status, error code or message was captured, because no response for the contended statements arrived. Sibling checks that provision a cluster and poll it to `available` against the same emulator complete inside the same budget, so the stall is in the Data API statement/transaction path rather than cluster provisioning.

## Steps to Reproduce
Cause 1 (RevokeSnapshotAccess):
1. redshift CreateCluster with a single-node cluster id and wait for ClusterStatus `available`.
2. redshift CreateClusterSnapshot with a fresh SnapshotIdentifier against that cluster; wait for Status `available`.
3. redshift CreateClusterSnapshot again with the SAME SnapshotIdentifier -> correctly refused with `ClusterSnapshotAlreadyExists` (control, passes today).
4. redshift AuthorizeSnapshotAccess on that snapshot for account A; DescribeClusterSnapshots shows A in AccountsWithRestoreAccess (control, passes today).
5. redshift RevokeSnapshotAccess on that snapshot for account B, which was never granted. OBSERVED: HTTP 501 `NotImplemented`. EXPECTED: HTTP 400 `AccessToSnapshotDenied`, with A's grant still present and B still absent.
6. redshift RevokeSnapshotAccess on that snapshot for account A. OBSERVED: HTTP 501 `NotImplemented`. EXPECTED: 200 with A removed from AccountsWithRestoreAccess.
Replay: run behavioral check `redshift.snapshot-duplicate-and-ungranted-revoke-fail`.

Cause 2 (contended transaction):
1. redshift CreateCluster single-node; wait for `available`.
2. Via redshift-data: CREATE TABLE t (id INT, n INT, v VARCHAR(16)); INSERT one row (1, 0, 'seed'); create a filler table with ~1000 rows so each transaction has real work to do.
3. Submit TWO BatchExecuteStatement calls concurrently against the same cluster/database, each with Sqls = [ SELECT n FROM t WHERE id = 1 ; a repeated wide aggregate over the filler table ; UPDATE t SET n = (SELECT max(n) FROM t) + 1, v = '<writer>' WHERE id = 1 ].
4. Poll DescribeStatement on both ids to a terminal status. OBSERVED: no terminal status arrives; the check is killed at the 60 s harness budget. EXPECTED: both terminate; at least one FINISHED; t.n equals the number of FINISHED transactions; any FAILED/ABORTED one reports Redshift error 1023.
Replay: run behavioral check `redshift.redshift-two-concurrent-transactions-on-the-same-row-serialize-to-one-winner`.
