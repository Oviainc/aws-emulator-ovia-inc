Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0

# An Apache Airflow version change never reports the documented CREATING_SNAPSHOT status

_Filed: 2026-10-24T16:17:20Z_
_Operation: UpdateEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ec42ee63c3cbc91c5bb52d083d61328ac6a6aee0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MWAA's own model documents a two-phase status lifecycle for an environment update: before the environment enters `UPDATING`, Amazon MWAA snapshots the Amazon RDS cluster behind the environment and reports `Environment.Status = CREATING_SNAPSHOT` ("The request to update environment details, or upgrade the environment version, is in progress and Amazon MWAA is creating a storage volume snapshot of the Amazon RDS database cluster associated with the environment"). The snapshot exists so MWAA can restore the environment if the update fails, which is why the two neighbouring statuses `ROLLING_BACK` and `UPDATE_FAILED` are modeled beside it.

Expected: `UpdateEnvironment` on an AVAILABLE environment — at minimum one that changes `AirflowVersion`, i.e. the version upgrade/downgrade path the status doc names explicitly — answers 200, and a `GetEnvironment` poll observes `CREATING_SNAPSHOT` first, then `UPDATING`, then `AVAILABLE` with `LastUpdate.Status = SUCCESS`.

Actual: the handler goes straight from AVAILABLE to `UPDATING` and owes only `AVAILABLE`. `CREATING_SNAPSHOT` is unreachable through every operation of this emulator, so a client that polls the update lifecycle (the documented way to tell "MWAA is still snapshotting, the web server is still up" from "the environment is being replaced") never sees it, and a client that treats `CREATING_SNAPSHOT` as a distinct phase of its own wait loop never exercises that branch. This is a different gap from the two UpdateEnvironment validation issues already filed (which are about which patches should be REFUSED, not about the status sequence of an accepted one) and from the open GetEnvironment PENDING issue (which is about the CREATE lifecycle with EndpointManagement=CUSTOMER).

## Evidence

Spec: `Environment.Status`'s own doc (smithy_query query="doc" shape="Environment" member="Status") enumerates `CREATING_SNAPSHOT` — "The request to update environment details, or upgrade the environment version, is in progress and Amazon MWAA is creating a storage volume snapshot of the Amazon RDS database cluster associated with the environment. A database snapshot is a backup created at a specific point in time. Amazon MWAA uses snapshots to recover environment metadata if the process to update or upgrade an environment fails." — and `UpdateEnvironmentInput.AirflowVersion`'s doc describes the upgrade/downgrade path that triggers it.
Handler: services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/update_environment.rs:373-378 does `stored.spec = merged; stored.begin_update(now, UPDATE_SOURCE_API, ...)`; `EnvironmentRecord::begin_update` (src/state.rs:614-632) sets `status = STATUS_UPDATING` and a single `PendingTransition { to: STATUS_AVAILABLE, last_update_status: SUCCESS }`. src/state.rs:53-69 declares only CREATING / AVAILABLE / UPDATING — there is no CREATING_SNAPSHOT constant anywhere in the crate, and nothing branches on whether the patch changes `AirflowVersion`.
Tests: tests/all/environment_business_rules.rs:563-648 (`an_update_is_refused_while_the_environment_has_not_settled_into_available`) asserts exactly the UPDATING -> AVAILABLE pair after an accepted update, so the current two-status sequence is the only thing pinned; no test asserts a snapshot phase for a version change.

## Steps to Reproduce

1. Boot the emulator and point a botocore mwaa client at it.
2. mwaa.create_environment(Name='VerEnv', ExecutionRoleArn='arn:aws:iam::123456789012:role/mwaa-exec', SourceBucketArn='arn:aws:s3:::my-airflow-bucket-unique-name', DagS3Path='dags', NetworkConfiguration={'SubnetIds':['subnet-00000000000000001','subnet-00000000000000002'],'SecurityGroupIds':['sg-00000000000000001']}, AirflowVersion='2.10.1') and poll mwaa.get_environment until Status == 'AVAILABLE'.
3. mwaa.update_environment(Name='VerEnv', AirflowVersion='2.11.0') -> 200 with the Arn.
4. Poll mwaa.get_environment(Name='VerEnv') repeatedly and collect the sequence of Status values.
5. Expected: the first status observed after the version change is 'CREATING_SNAPSHOT' (MWAA snapshots the RDS cluster before replacing the environment), followed by 'UPDATING' and then 'AVAILABLE' with LastUpdate.Status 'SUCCESS'.
6. Observed: 'UPDATING' immediately, then 'AVAILABLE'; 'CREATING_SNAPSHOT' never appears for any update.
