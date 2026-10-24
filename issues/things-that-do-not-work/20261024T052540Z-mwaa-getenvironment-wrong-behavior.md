Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# An environment created with EndpointManagement=CUSTOMER never reports the documented PENDING status

_Filed: 2026-10-24T05:25:40Z_
_Operation: GetEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MWAA's own model documents a distinct create lifecycle for customer-managed VPC endpoints: with `EndpointManagement=CUSTOMER` the environment is created but PAUSED until the caller creates the required VPC endpoints, and that pause is observable as `Environment.Status = PENDING` (and after 72 hours without endpoints, `CREATE_FAILED`). With `EndpointManagement=SERVICE` (or omitted) MWAA creates the endpoints itself and the environment proceeds to `AVAILABLE`.

Expected: `CreateEnvironment(..., EndpointManagement='CUSTOMER')` followed by `GetEnvironment` reports `Status: 'PENDING'` and stays there (no transition to `AVAILABLE`) until the customer's VPC endpoints exist; `EndpointManagement: 'CUSTOMER'` is echoed back beside it.

Actual: the emulator ignores `EndpointManagement` for the status lifecycle. Every create is stored as `CREATING` with a single pending transition to `AVAILABLE` that the clock materialises `TRANSITION_SPAN` later, so a CUSTOMER-managed environment reports `CREATING` and then `AVAILABLE` — a status the real service only reaches after the endpoints are created. `PENDING` is never reachable through any operation of this emulator, so a client that polls for PENDING (the shared-VPC workflow) never sees it and a client that waits for AVAILABLE succeeds where the real service would still be blocked.

Note this is a DIFFERENT gap from the already-open CreateEnvironment/wrong-behavior issue 20261024T002430Z, which is about service-assigned defaults for members the create omitted; it says nothing about the status lifecycle. The divergence is observed on GetEnvironment's payload, but the code to change is the create's status/pending assignment.

## Evidence

Spec: `Environment.Status`'s own doc enumerates `PENDING` — "The request was successful, but the process to create the environment is paused until you create the required VPC endpoints in your VPC. After you create the VPC endpoints, the process resumes" — and `CreateEnvironmentInput.EndpointManagement`'s doc states the rule: "If you choose to create an environment in a shared VPC, you must set this value to CUSTOMER. In a shared VPC deployment, the environment will remain in PENDING status until you create the VPC endpoints. If you do not take action to create the endpoints within 72 hours, the status will change to CREATE_FAILED." (both quoted verbatim in src/ops/create_environment.rs's own header docs, lines 63-71).

Handler: src/ops/create_environment.rs:333 copies `endpoint_management` into the stored spec but nothing reads it; lines 386-392 always store `status: STATUS_CREATING` with `PendingTransition { to: STATUS_AVAILABLE, due_at: now + TRANSITION_SPAN }`, regardless of the endpoint-management mode. src/state.rs declares only `STATUS_CREATING`, `STATUS_AVAILABLE` and `STATUS_UPDATING` (lines 55-63) — there is no `PENDING` constant — and `EnvironmentRecord::settle` (src/state.rs:320-334) unconditionally lands the environment in `AVAILABLE`. src/ops/get_environment.rs:152-159 reports `stored.to_environment()`, so the status it shows is whatever create stored.

Tests: tests/all/environment_lifecycle.rs covers CREATING -> AVAILABLE and the per-case member round-trip (its case table includes EndpointManagement rows for the echo only); no test asserts a PENDING status or any endpoint-management-dependent lifecycle.

## Steps to Reproduce

1. Boot the emulator and point a botocore mwaa client at it.
2. mwaa.create_environment(Name='SharedVpcEnv', ExecutionRoleArn='arn:aws:iam::123456789012:role/mwaa-exec', SourceBucketArn='arn:aws:s3:::my-airflow-bucket-unique-name', DagS3Path='dags', NetworkConfiguration={'SubnetIds': ['subnet-00000000000000001','subnet-00000000000000002'], 'SecurityGroupIds': ['sg-00000000000000001']}, EndpointManagement='CUSTOMER') -> 200 with an Arn.
3. mwaa.get_environment(Name='SharedVpcEnv') immediately, then again after a second.
4. Expected: Status 'PENDING' on both reads (the create is paused until the customer's VPC endpoints exist), with EndpointManagement 'CUSTOMER'.
5. Observed: Status 'CREATING' on the first read and 'AVAILABLE' on the second — the same lifecycle a SERVICE-managed create gets; 'PENDING' is unreachable.
