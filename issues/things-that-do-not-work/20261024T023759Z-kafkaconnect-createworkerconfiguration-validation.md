Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047af81d647cd24bcec0ef3f34fd68e55311879a

# Reject a propertiesFileContent that is not valid base64

_Filed: 2026-10-24T02:37:59Z_
_Operation: CreateWorkerConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `047af81d647cd24bcec0ef3f34fd68e55311879a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`propertiesFileContent` is documented as "Base64 encoded contents of connect-distributed.properties file" — the service decodes it to read the worker properties, so a value that is not valid base64 cannot be stored and is refused with HTTP 400 BadRequestException.

Expected: POST /v1/worker-configurations with `propertiesFileContent: "not base64!!"` is a 400 BadRequestException naming the parameter (the same `Invalid parameter <member>: <reason>` form the rest of this emulator uses).

Actual: the handler stores whatever string arrives verbatim (`properties_file_content: input.properties_file_content`) and answers 200 ACTIVE; DescribeWorkerConfiguration then hands the non-base64 blob back in `latestRevision.propertiesFileContent`, so a client decoding it fails against the emulator where it would have been told at create time against the real service.

## Evidence

Spec: Smithy `CreateWorkerConfigurationRequest$propertiesFileContent` doc (via smithy_query doc): "Base64 encoded contents of connect-distributed.properties file." — the member is typed `__sensitiveString`, so the model itself cannot express the encoding; CreateWorkerConfiguration declares BadRequestException (400).
Handler: src/ops/create_worker_configuration.rs lines 181-234 — the only refusals are duplicate name and quota; no encoding check before the record is built. src/state.rs `WorkerConfigurationRecord::properties_file_content` is documented "stored verbatim" and is echoed by `revision_description()`.
Tests: tests/all/worker_configuration_create_and_describe.rs `the_modelled_constraints_on_a_create_worker_configuration_request_are_refused` covers only the model's own length/required constraints; no test sends a non-base64 body.

## Steps to Reproduce

1. POST /v1/worker-configurations with `{"name": "wc-bad-b64", "propertiesFileContent": "not base64!!"}`.
2. Observe 200 with workerConfigurationState ACTIVE; the real service answers 400 BadRequestException for a propertiesFileContent it cannot base64-decode.
3. GET /v1/worker-configurations/{workerConfigurationArn} — `latestRevision.propertiesFileContent` hands the undecodable string back.
