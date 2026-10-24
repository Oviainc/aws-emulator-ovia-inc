Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# UpdateEnvironment accepts an Apache Airflow version downgrade and unverified role/bucket/security-group replacements

_Filed: 2026-10-24T15:33:28Z_
_Operation: UpdateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two symptoms of the same class, both on the merge path in src/ops/update_environment.rs, and both distinct from the already-resolved UpdateEnvironment validation issue (which covered the S3 path/version pairing, the Min<=Max pairings, the AirflowVersion/EnvironmentClass value SETS and the not-AVAILABLE status gate — all now implemented):

1. Version DOWNGRADE accepted. Amazon MWAA supports moving an environment to a LATER Apache Airflow version only; downgrading an environment to an earlier version is not supported and the request is refused with HTTP 400 `ValidationException`. The emulator checks only SET MEMBERSHIP (`crate::state::SUPPORTED_AIRFLOW_VERSIONS` via `EnvironmentSpec::validate`), never the ordering, so `UpdateEnvironment(Name=..., AirflowVersion='2.7.2')` on an environment running '3.0.6' answers 200, the record is overwritten, and `GetEnvironment` reports the older version — a state AWS cannot produce. (The constant is already ordered oldest-first, so the comparison is available and simply not made.)

2. Replaced dependencies are not resolved. `ExecutionRoleArn`, `SourceBucketArn` and `NetworkConfiguration.SecurityGroupIds` are merged into the stored spec verbatim (lines 299-336) with no check that the role exists/is assumable, that the bucket exists in the environment's Region, or that the security groups exist in the environment's VPC. AWS refuses such a patch with `ValidationException`; the emulator answers 200 and begins an update that leaves the environment pointing at resources that do not exist.

## Evidence

Spec grounding: `UpdateEnvironment` declares `ValidationException` (@httpError 400); its `AirflowVersion` member doc enumerates the supported versions in ascending order ("2.7.2, 2.8.1, 2.9.2, 2.10.1, 2.10.3, 2.11.0, and 3.0.6") and MWAA's documented version-upgrade behavior is upgrade-only — an environment cannot be moved back to an earlier Apache Airflow version. `ExecutionRoleArn`/`SourceBucketArn`/`NetworkConfiguration` are documented as pointers to live IAM/S3/VPC resources (same grounding as the CreateEnvironment dependency gap filed alongside this one; see also the sibling precedent 20261024T002400Z-mwaa-serverless-updateworkflow-validation).
Handler: src/ops/update_environment.rs:297-371 merges member by member and then calls the shared `crate::state::EnvironmentSpec::validate`, which only (a) tests `SUPPORTED_AIRFLOW_VERSIONS.contains(&value)` / `ENVIRONMENT_CLASSES.contains(&value)` and (b) compares MinWorkers<=MaxWorkers and MinWebservers<=MaxWebservers (src/state.rs:354-397). There is no comparison against the STORED AirflowVersion and no dependency resolution; `stored.spec = merged; stored.begin_update(...)` follows unconditionally (lines 373-378).
Tests: tests/all/environment_business_rules.rs asserts only the catalog membership and the floor/ceiling pairings; tests/all/environment_lifecycle.rs round-trips internally consistent patches. No test asserts a refusal for a downgrade or for a patch naming a non-existent role/bucket/security group.

## Steps to Reproduce

1. Boot the emulator and create environment 'VerEnv' with AirflowVersion='3.0.6' (or no AirflowVersion, which defaults to the latest); wait until GetEnvironment reports Status='AVAILABLE'.
2. Call UpdateEnvironment (PATCH /environments/VerEnv) with AirflowVersion='2.7.2'.
3. Expect HTTP 400 ValidationException (MWAA does not support downgrading an environment's Apache Airflow version) and GetEnvironment still reporting '3.0.6'.
4. Observe HTTP 200 with the Arn; GetEnvironment reports AirflowVersion '2.7.2'.
5. Call UpdateEnvironment (PATCH /environments/VerEnv) with ExecutionRoleArn='arn:aws:iam::000000000000:role/never-created' and NetworkConfiguration={'SecurityGroupIds':['sg-does-not-exist']}.
6. Expect HTTP 400 ValidationException for the unusable role/security group; observe HTTP 200 and GetEnvironment reporting the dangling values.
