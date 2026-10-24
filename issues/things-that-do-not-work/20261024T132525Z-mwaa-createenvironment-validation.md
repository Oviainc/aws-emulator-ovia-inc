Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Enforce the documented Schedulers / MinWebservers / MaxWebservers value ranges

_Filed: 2026-10-24T13:25:25Z_
_Operation: CreateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateEnvironment accepts unit counts AWS refuses, because the only cross-member rules the emulator enforces are the Min>Max pairings and the AirflowVersion/EnvironmentClass value sets — the per-member documented ranges are not checked at all.

Expected (AWS): `Schedulers` is documented as "v2 - For environments larger than mw1.micro, accepts values from 2 to 5. Defaults to 2 for all environment sizes except mw1.micro, which defaults to 1. v1 - Accepts 1." and `MinWebservers`/`MaxWebservers` as "For environments larger than mw1.micro, accepts values from 2 to 5." A create naming `Schedulers: 0` (or a negative value), or `MinWebservers`/`MaxWebservers` above 5, is refused with HTTP 400 / `ValidationException` and nothing is stored.

Actual (emulator): the Smithy model constrains `Schedulers` only by `@range(max: 5)` (no minimum) and `MinWebservers`/`MaxWebservers` only by `@range(min: 1)` (no maximum), so the boundary validator cannot refuse these values and the handler stores them verbatim. `CreateEnvironment` answers HTTP 200 with an Arn, and a following `GetEnvironment` reports `Schedulers: 0` / `MaxWebservers: 100` — a fleet AWS would never have provisioned. The same shared validator is used by `UpdateEnvironment`, so a patch can push a stored environment to the same impossible counts; one fix in `EnvironmentSpec::validate` closes both.

Note: this is NOT the MinWorkers<=MaxWorkers rule from the earlier trace issue — that pairing rule is now implemented (state.rs, the floor/ceiling loop); the missing part is the per-member documented range (and the mw1.micro special case of exactly 1).

## Evidence

Spec: smithy_query(query="shape", shape="Schedulers") -> {"range": {"max": 5}} (no min); MinWebservers/MaxWebservers -> {"range": {"min": 1}} (no max). The valid value sets are stated in the member docs of CreateEnvironmentInput/UpdateEnvironmentInput (Schedulers: "accepts values from 2 to 5 ... mw1.micro ... 1"; Min/MaxWebservers: "accepts values from 2 to 5. Defaults to 2 ... except mw1.micro, which defaults to 1"), reproduced verbatim in the handler's own doc header (services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/create_environment.rs lines 89-116, 138-141).
Handler: src/ops/create_environment.rs lines 333-341 take `input.schedulers`/`min_webservers`/`max_webservers` as given (defaulting only when absent) and src/state.rs `EnvironmentSpec::validate` (lines 354-397) checks ONLY the AirflowVersion/EnvironmentClass catalogs and the MinWorkers<=MaxWorkers / MinWebservers<=MaxWebservers pairings — there is no lower/upper bound check, so Schedulers=0, Schedulers=-3 and MaxWebservers=100 all pass.
Tests: tests/all/environment_business_rules.rs covers only the catalog values and the floor-above-ceiling pairings (lines 485-556); tests/all/environment_lifecycle.rs and environment_service_defaults.rs only assert defaults and round-trips. No test asserts a refusal for an out-of-range Schedulers or webserver count.

## Steps to Reproduce

1. Boot the emulator and point an mwaa client at it.
2. CreateEnvironment(Name='sched-zero', ExecutionRoleArn='arn:aws:iam::000000000000:role/mwaa-exec', SourceBucketArn='arn:aws:s3:::my-airflow-bucket', DagS3Path='dags', NetworkConfiguration={'SubnetIds': ['subnet-a','subnet-b'], 'SecurityGroupIds': ['sg-1']}, Schedulers=0).
3. Expect HTTP 400 ValidationException and no stored environment; observe HTTP 200 with an Arn.
4. GetEnvironment(Name='sched-zero') and observe Schedulers: 0 reported for a provisioned environment.
5. Repeat with MinWebservers=6, MaxWebservers=100 (and with Schedulers=-3) — each is accepted and read back.
6. Create a valid environment, wait for AVAILABLE, then UpdateEnvironment(Name=..., Schedulers=0): the same shared validator accepts it.
