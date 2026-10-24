Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# InvokeRestApi answers 501 for an environment that exists instead of 200 with RestApiStatusCode/RestApiResponse

_Filed: 2026-10-24T00:24:30Z_
_Operation: InvokeRestApi | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

InvokeRestApi is only half implemented: the handler resolves the environment from state and refuses an unknown name with the modeled 404, but a well-formed call against an environment that DOES exist returns the scaffold's HTTP 501 `NotImplemented` and never produces the operation's modeled output.

Expected (real MWAA): POST /restapi/{Name} proxies the request to the environment's Apache Airflow web server and answers HTTP 200 with `RestApiStatusCode` (the HTTP status the Airflow API returned, e.g. 200) and `RestApiResponse` (the Airflow JSON document, e.g. `{"dags": [...], "total_entries": N}` for `GET /dags`, or the created DAG run object for `POST /dags/{dag_id}/dagRuns`). Airflow-side failures surface as `RestApiClientException` / `RestApiServerException` (both HTTP 400), and an insufficient-permission call as `AccessDeniedException` (403).

Actual (emulator): HTTP 501 with error code `NotImplemented` and message "the Apache Airflow REST API of environment <name> is not served by this emulator, so <METHOD> <path> was not invoked". The whole Airflow data plane (listing DAGs, triggering DAG runs, reading task instances) is therefore unusable: `RestApiStatusCode`, `RestApiResponse`, `RestApiClientException` and `RestApiServerException` are all unreachable, and botocore sees an unmodeled 501 that it treats as a retryable server fault.

A minimal but stateful Airflow REST surface is implementable from the environment record (e.g. a per-environment DAG/DAG-run store so `POST /dags/{id}/dagRuns` is later visible to `GET /dags/{id}/dagRuns`), with `Method`/`Path`/`QueryParameters`/`Body` actually routed, an unknown path answered as the Airflow API would (RestApiStatusCode 404 or RestApiClientException), and no canned/echoed payloads.

## Evidence

Smithy model: InvokeRestApi -> POST /restapi/{Name}, output members `RestApiStatusCode` (Integer) and `RestApiResponse` (document), errors AccessDeniedException(403), RestApiClientException(400), RestApiServerException(400), ResourceNotFoundException(404), ValidationException(400), InternalServerException(500). Handler: services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/invoke_rest_api.rs:206-218 returns `Scaffolded::NotImplemented` whenever the named environment is present; the module doc (lines 13-19) states this is deliberate. `Method`, `Path`, `QueryParameters` and `Body` are only interpolated into the 501 message and never acted on. Test coverage PINS the divergence: tests/all/rejection_contract.rs:709-755 (`invoke_rest_api_does_not_synthesise_an_apache_airflow_answer`) asserts 501 / `NotImplemented` and asserts the body contains no `RestApiStatusCode`. The already-resolved issue 20261022T022901Z (cause 2) asked for "the environment lookup, the modeled 404, AND the REST-API invocation response" — only the first two were delivered; the open issue 20261023T180451Z concerns only the shape of the 404 body, not the success path.

## Steps to Reproduce

1. Boot the emulator and point a botocore mwaa client at it.
2. Create an environment: mwaa.create_environment(Name='RestApiEnv', ExecutionRoleArn='arn:aws:iam::123456789012:role/mwaa-exec', SourceBucketArn='arn:aws:s3:::my-airflow-bucket-unique-name', DagS3Path='dags', NetworkConfiguration={'SubnetIds': ['subnet-00000000000000001','subnet-00000000000000002'], 'SecurityGroupIds': ['sg-00000000000000001']}) -> 200.
3. Call mwaa.invoke_rest_api(Name='RestApiEnv', Path='/dags', Method='GET').
4. Expected: HTTP 200 with RestApiStatusCode and a RestApiResponse document computed from the emulator's Airflow state. Observed: HTTP 501, Error.Code 'NotImplemented', no RestApiStatusCode/RestApiResponse.
5. Also call mwaa.invoke_rest_api(Name='RestApiEnv', Path='/dags/x/dagRuns', Method='POST', Body={'conf': {}}) and observe the same 501.
