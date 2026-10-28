Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# CreateEnvironment accepts WebserverAccessMode=PUBLIC_AND_PRIVATE for Apache Airflow versions the member's own doc excludes

_Filed: 2026-10-28T02:33:58Z_
_Operation: CreateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`WebserverAccessMode`'s documentation states a cross-member rule: `PUBLIC_AND_PRIVATE` "is only available for Apache Airflow version 3.2 and later". `AirflowVersion`'s documentation lists every version the service runs, and the newest is `3.0.6` — so no version the environment can be created on qualifies for that access mode.

The handler stores `WebserverAccessMode` verbatim and never compares it against the resolved `AirflowVersion`. `EnvironmentSpec::validate` — the one place this crate enforces "every rule the RESOLVED environment configuration must satisfy that the Smithy model cannot state … a value set the member's doc fixes … and a bound one member places on another" — checks the `AirflowVersion` and `EnvironmentClass` catalogs, the per-class unit-count ranges and the Min/Max pairings, but has no `webserver_access_mode` branch at all. The result is a 200 and a stored environment that reports a web-server access mode the model says that environment's Airflow version cannot have; `UpdateEnvironment` merges the same member through the same validator, so a patch can move a stored environment into the same impossible configuration.

## Evidence

Spec, smithy_query(query="doc", shape="CreateEnvironmentInput", member="WebserverAccessMode"): "Defines the access mode for the Apache Airflow web server. … If set to PUBLIC_AND_PRIVATE, creates both a public network load balancer (NLB) for browser access and a private VPC endpoint (VPCE) for worker-to-webserver communication. **This mode is only available for Apache Airflow version 3.2 and later.**" — reproduced verbatim in the handler's own doc header, services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/create_environment.rs:164-169.
Spec, smithy_query(query="doc", shape="CreateEnvironmentInput", member="AirflowVersion"): "Valid values: 2.7.2, 2.8.1, 2.9.2, 2.10.1, 2.10.3, 2.11.0, and 3.0.6." — the same list the crate stores as the enforced catalog, src/state.rs:218-220: `pub const SUPPORTED_AIRFLOW_VERSIONS: [&str; 7] = ["2.7.2", "2.8.1", "2.9.2", "2.10.1", "2.10.3", "2.11.0", "3.0.6"];`.
Handler: src/ops/create_environment.rs:343-346 stores the member with no cross-check —
```
webserver_access_mode: input
    .webserver_access_mode
    .clone()
    .unwrap_or_else(|| crate::state::DEFAULT_WEBSERVER_ACCESS_MODE.to_string()),
```
Validator: src/state.rs:529-612 — `EnvironmentSpec::validate`, documented at src/state.rs:530-534 as "Every rule the RESOLVED environment configuration must satisfy that the Smithy model cannot state — a value set the member's doc fixes, the per-class count range a sizing member's doc fixes, and a bound one member places on another" — contains only the `AirflowVersion`/`EnvironmentClass` catalog loop (lines 543-561), the `Schedulers`/`MinWebservers`/`MaxWebservers` range loop (lines 569-588) and the floor/ceiling loop (lines 591-610). `webserver_access_mode` appears nowhere in it, and no comment in state.rs justifies the omission (`DEFAULT_WEBSERVER_ACCESS_MODE`, src/state.rs:265-269, only names the three enum values).
UpdateEnvironment merges the same member through the same validator: src/ops/update_environment.rs:364-366 (`spec.webserver_access_mode = value;`) and 374 (`merged.validate()`).
Tests: tests/all/environment_business_rules.rs and tests/all/environment_unit_count_ranges.rs cover only the catalog values, the unit-count ranges and the Min>Max pairings; no test asserts anything about `WebserverAccessMode` versus `AirflowVersion`, so no existing test pins the accepting behaviour.

## Steps to Reproduce

1. Boot the emulator and point an mwaa client at it.
2. CreateEnvironment(Name='PubPriv', ExecutionRoleArn='arn:aws:iam::000000000000:role/mwaa-exec', SourceBucketArn='arn:aws:s3:::my-airflow-bucket', DagS3Path='dags', NetworkConfiguration={'SubnetIds':['subnet-a','subnet-b'],'SecurityGroupIds':['sg-1']}, AirflowVersion='2.10.3', WebserverAccessMode='PUBLIC_AND_PRIVATE').
3. Expect HTTP 400 / ValidationException (the mode is documented as available only for Apache Airflow 3.2 and later, and 2.10.3 is not) with nothing stored; observe HTTP 200 with an Arn.
4. GetEnvironment(Name='PubPriv') and observe AirflowVersion '2.10.3' reported beside WebserverAccessMode 'PUBLIC_AND_PRIVATE'.
5. Repeat with the newest supported version (AirflowVersion='3.0.6', still below 3.2) — also accepted.
6. Create a valid environment with WebserverAccessMode='PUBLIC_ONLY', wait for AVAILABLE, then UpdateEnvironment(Name=..., WebserverAccessMode='PUBLIC_AND_PRIVATE'): the shared validator accepts it too.
