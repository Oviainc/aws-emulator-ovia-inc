Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-invoke-rest-api-nonexistent-environment-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-invoke-rest-api-nonexistent-environment-not-found
Issue-Shape-Id: com.amazonaws.mwaa#AmazonMWAA
Issue-Key: mwaa|InvokeRestApi|4xx->4xx|$
Issue-Commit: b887f4c166159c5dcfc6801bcb3cb6a6bedc24bf

# InvokeRestApi returns modeled ResourceNotFoundException body instead of the env endpoint's bare "Environment not found" payload

> _Note: this issue was identified against the code base at commit `b887f4c166159c5dcfc6801bcb3cb6a6bedc24bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeRestApi

## Description
The emulator serves the wrong 404 response BODY SHAPE for InvokeRestApi against an environment that does not exist. The status code is right; the payload is a different document type than the one AWS's env.* endpoint returns, so an SDK/client that inspects the body gets a different error classification.

Cause 1 of 1: InvokeRestApi on a nonexistent environment.

AWS: Smithy: MWAA InvokeRestApi is POST /restapi/{Name} with @endpoint hostPrefix "env." (i.e. it is served by the environment-scoped data endpoint, not the mwaa control-plane endpoint); it models ResourceNotFoundException @httpError 404 with a single `message` member, and output InvokeRestApiResponse{RestApiStatusCode, RestApiResponse:document}. For this request the RECORDED behavior (recorded behavior only - the model does not state which 404 shape the env.* host emits) is HTTP 404 whose body is a bare JSON string document, `"Environment not found"` - i.e. the response payload's root is a string, not an object, and it carries no `__type`/error-code field at all. Hypothesis: the env.* host rejects the request before it reaches the modeled MWAA control-plane error serializer, so the 404 is not a serialized ResourceNotFoundException.

Emulator: returns HTTP 404 (status matches) but with a JSON OBJECT body: `{"__type": "ResourceNotFoundException", "message": "Environment <name> not found."}`. Replay reports `$: type_mismatch` at the payload root - expected a string document, got a structure.

Why: a client relying on AWS's behavior sees the opposite error classification. Against AWS the bare-string 404 carries no resolvable error code, so an AWS SDK surfaces an unmodeled/unknown 404 service error and code paths written to catch that (generic 404 handling, retry/notify-on-unknown-error logic, raw-body parsing of the Airflow proxy response) run. Against the emulator the SDK resolves `__type` and raises a modeled ResourceNotFoundException instead, so those paths are never exercised and, conversely, code written against the emulator that catches ResourceNotFoundException will not catch anything in production. The body shape itself also differs (string vs object), so any direct body parse crashes in one of the two environments.

Fix: the InvokeRestApi handler's error path / response serializer for the env.* data endpoint in the MWAA service crate. The not-found condition for this operation must be emitted as the env endpoint's bare document payload (root-level JSON string, no `__type` key) with status 404, rather than being routed through the shared modeled-exception serializer that stamps `__type` and a `message` object. Note the recorded message text itself is only an example of the payload; the requirement here is the 404 status plus the root payload TYPE (string document, no error-code key).

Traces: fixture mwaa-invoke-rest-api-nonexistent-environment-not-found, step task#1 (divergence_key service=mwaa|InvokeRestApi|status=404->404|keys=$).

## Evidence
Fixture mwaa-invoke-rest-api-nonexistent-environment-not-found, step task#1, operation InvokeRestApi, verdict=mismatch.

Expected (recorded AWS): HTTP 404, body root = JSON string document `"Environment not found"` (no `__type`, no `message` member).
Actual (emulator): HTTP 404, body root = JSON object `{"__type": "ResourceNotFoundException", "message": "Environment sim2real-mwaa-missing-43317f9b-6751-4cdd-beac-bd0257e6e5ac not found."}` (the generated environment name is environment-specific and is not itself a requirement).

Replay detail: `$: type_mismatch (expected 'Environment not found', got {'__type': 'ResourceNotFoundException', 'message': '...'})`. Status codes agree (404 -> 404); only the payload type/shape diverges.

Model references: MWAA InvokeRestApi - http POST /restapi/{Name}, endpoint hostPrefix "env.", errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), RestApiClientException(400), RestApiServerException(400), ValidationException(400); ResourceNotFoundException members = {message: String}.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus), or replay fixture `mwaa-invoke-rest-api-nonexistent-environment-not-found` step `task#1` alone.
3. The step issues MWAA InvokeRestApi (POST /restapi/{Name}) against an environment name that was never created (e.g. sim2real-mwaa-missing-<uuid>).
4. Observe: response status is 404 in both AWS recording and emulator, but the emulator's body is a modeled-exception object with `__type: ResourceNotFoundException`, while the recording's body root is the bare JSON string `"Environment not found"`. Replay flags `$: type_mismatch`.
