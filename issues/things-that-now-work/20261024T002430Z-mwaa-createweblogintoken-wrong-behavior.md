Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# CreateWebLoginToken reports the SigV4 access key id as IamIdentity/AirflowIdentity

_Filed: 2026-10-24T00:24:30Z_
_Operation: CreateWebLoginToken | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The output members `IamIdentity` ("The IAM identity creating the web login token. This might be an IAM user, or an assumed or federated identity") and `AirflowIdentity` ("The user name of the Apache Airflow identity creating the web login token") are filled with the access key id taken from the request's SigV4 credential, and AirflowIdentity is just that same string truncated to 64 chars.

Expected (real MWAA): `IamIdentity` is the caller's IAM identity — a user/role ARN or assumed-role session identity, e.g. `arn:aws:sts::123456789012:assumed-role/Admin/session-name` — and `AirflowIdentity` is the Apache Airflow user name that identity maps to (e.g. the role/user name, not an ARN and not a credential id). The two values differ in form.

Actual: both members report the access key id (e.g. `AKIA…`/`ASIA…`), which is a credential identifier and never an IAM identity. A client that parses IamIdentity as an ARN, or displays/maps AirflowIdentity as an Airflow user name, gets a value AWS never emits, and the two members are always equal.

## Evidence

Spec: CreateWebLoginToken output members IamIdentity and AirflowIdentity (AirflowIdentity @length(1,64)) with the docs quoted above. Handler: src/ops/create_web_login_token.rs:190-196 sets `iam_identity = crate::state::caller_iam_identity(ctx)` and `airflow_identity = crate::state::airflow_identity(&iam_identity)`; src/state.rs:595-610 documents that both are the SigV4 access-key id (`engine::extract_akid`) truncated for the Airflow one. No test asserts the shape of either member beyond presence.

## Steps to Reproduce

1. Boot the emulator; create environment 'TokenEnv' and let it reach AVAILABLE.
2. mwaa.create_web_login_token(Name='TokenEnv') with ordinary SigV4 credentials.
3. Expected: IamIdentity is an IAM user/assumed-role identity (ARN form) and AirflowIdentity is the mapped Apache Airflow user name; the two are not the same string.
4. Observed: both IamIdentity and AirflowIdentity are the request's access key id.
