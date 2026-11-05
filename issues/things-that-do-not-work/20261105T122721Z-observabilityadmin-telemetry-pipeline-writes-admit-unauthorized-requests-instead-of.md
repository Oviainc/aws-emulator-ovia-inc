Issue-Source: trace-sweep-divergence
Issue-Trace: observabilityadmin-telemetry-pipeline-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: observabilityadmin-telemetry-pipeline-error-paths observabilityadmin-telemetry-pipeline-workflow
Issue-Shape-Id: com.amazonaws.observabilityadmin#ObservabilityAdmin
Issue-Key: observabilityadmin|DeleteTelemetryPipeline|4xx->4xx|$.message
Issue-Key: observabilityadmin|DeleteTelemetryPipeline|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: observabilityadmin|DeleteTelemetryPipeline|4xx->4xx|status
Issue-Key: observabilityadmin|UpdateTelemetryPipeline|4xx->2xx|$
Issue-Key: observabilityadmin|UpdateTelemetryPipeline|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: observabilityadmin|UpdateTelemetryPipeline|4xx->2xx|status
Issue-Commit: 9de1430d35a30df63dfe38bccf7c2cf4feacacf3

# Telemetry pipeline writes admit unauthorized requests instead of answering 401 Unauthorized

> _Note: this issue was identified against the code base at commit `9de1430d35a30df63dfe38bccf7c2cf4feacacf3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTelemetryPipeline

## Description
The emulator never produces ObservabilityAdmin's front-door authentication refusal. Two recorded pipeline-write steps whose request credentials AWS does not accept are answered 401 by AWS and are instead admitted into the operation handler by the emulator, which then answers out of its own state. 2 causes, both on observabilityadmin.

Cause 1 of 2 — DeleteTelemetryPipeline answers an unauthorized delete with a resource verdict (404) instead of 401 Unauthorized. Fixture observabilityadmin-telemetry-pipeline-error-paths, step task#3.
AWS: recorded behavior only — the Smithy model binds ObservabilityAdmin to authSchemes [sigv4] (sigv4Name observabilityadmin, protocol restJson1) and DeleteTelemetryPipeline models only AccessDeniedException(400), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), TooManyRequestsException(429) and ValidationException(400); there is NO modeled 401, so the recorded answer is the service front door's own rejection, taken ahead of any operation dispatch. The recording shows HTTP 401 with a JSON body whose lowercase member `message` is the fixed text `Unauthorized` (no variable part: no name, id, limit or member name is interpolated into it), plus an `x-amzn-errortype` header the fixture asserts as present but does not assert a value for. Authentication is decided before the identifier is resolved, so whether the named pipeline exists is irrelevant to this answer.
Emulator: answers HTTP 404 with `x-amzn-errortype: ResourceNotFoundException` and no `message` member at all (the replay reports `$.message: missing (expected 'Unauthorized', got None)`), i.e. it ran DeleteTelemetryPipeline's handler, resolved the identifier against its store, found nothing and reported the resource verdict.
Why: a client that must distinguish "my credentials were not accepted" from "that pipeline is gone" gets the wrong answer. An SDK retry/credential-refresh path keyed on 401 never fires, and a caller deleting with bad credentials is told the delete already succeeded in effect (404 on a repeated delete is the emulator's documented success-idempotence signal), so a credential misconfiguration is silently read as a completed teardown.
Fix: the shared request-authentication stage of the centralized request walk (the server runtime's sigv4 verification that `emulator_server::extract_akid` reads headers for), not DeleteTelemetryPipeline's handler — the handler is correct for an authenticated caller and must simply not be reached. The operation's own module doc already claims "[t]he emulator verifies every request's signature centrally"; that stage currently admits a request whose credentials AWS refuses.
Traces: observabilityadmin-telemetry-pipeline-error-paths#task#3.

Cause 2 of 2 — UpdateTelemetryPipeline performs the update and answers 200 for an unauthorized request instead of 401 Unauthorized. Fixture observabilityadmin-telemetry-pipeline-workflow, step task#8.
AWS: recorded behavior only — same sigv4-only auth scheme; UpdateTelemetryPipeline models AccessDeniedException(400), InternalServerException(500), ResourceNotFoundException(404), TooManyRequestsException(429) and ValidationException(400) and no 401, so again the recorded answer is the front-door rejection. The recording shows HTTP 401 with body `{"message": "Unauthorized"}` — `message` carrying that one fixed word, with no values drawn from the request or from service state — and an `x-amzn-errortype` header present with its value unasserted. No state change occurs, because the request is rejected before dispatch.
Emulator: answers HTTP 200 with the modeled empty body (`$: empty_body (expected {'message': 'Unauthorized'}, got {})`) and emits no `x-amzn-errortype` header, which means it dispatched the handler and ran `store.replace_pipeline_configuration(...)`: the unauthorized request MUTATED the pipeline's stored configuration.
Why: this is worse than a wrong status — a caller whose credentials AWS would refuse can rewrite a pipeline's configuration in the emulator, so tests that assert an unauthenticated or wrongly-signed write is rejected pass against AWS and are inverted here, and any configuration written by such a call corrupts the state every later read of that pipeline observes.
Fix: same component — the centralized authentication stage must refuse the request before UpdateTelemetryPipeline's handler runs; no change belongs in the handler or its serializer.
Traces: observabilityadmin-telemetry-pipeline-workflow#task#8.

Neither cause is covered by the three open observabilityadmin issues (20261030T012240Z-observabilityadmin-deletes3tableintegration-answers-an-unknown-integration-arn-with-400.md, 20261031T203350Z-observabilityadmin-gettelemetrypipeline-data-source-name-and-validatetelemetrypipelineconfiguration-restriction-errors.md, 20261103T192150Z-observabilityadmin-validatetelemetrypipelineconfiguration-reports-an-unknown-processor-as-reason-unsupported.md): all three are about resource-verdict or validation-report content for authenticated calls, none about the authentication refusal itself.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 71.7%, 15 mismatched steps):

Cause 1 — observabilityadmin-telemetry-pipeline-error-paths#task#3, DeleteTelemetryPipeline:
  expected status 401, actual 400->no: actual 404
  $.message: missing (expected 'Unauthorized', got None)
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')
  divergence_key: service=observabilityadmin|DeleteTelemetryPipeline|status=401->404|keys=$.message,$headers.x-amzn-errortype,status

Cause 2 — observabilityadmin-telemetry-pipeline-workflow#task#8, UpdateTelemetryPipeline:
  expected status 401, actual 200
  $: empty_body (expected {'message': 'Unauthorized'}, got {})
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=observabilityadmin|UpdateTelemetryPipeline|status=401->200|keys=$,$headers.x-amzn-errortype,status

Model evidence (smithy_query on services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json):
  service_info: protocols [restJson1], authSchemes [sigv4], sigv4Name observabilityadmin
  DeleteTelemetryPipeline errors: AccessDeniedException 400, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, TooManyRequestsException 429, ValidationException 400 — no 401
  UpdateTelemetryPipeline errors: AccessDeniedException 400, InternalServerException 500, ResourceNotFoundException 404, TooManyRequestsException 429, ValidationException 400 — no 401

Code evidence: services/service-com-amazonaws-observabilityadmin-observabilityadmin/src/ops/delete_telemetry_pipeline.rs:184-209 resolves the identifier and returns ResourceNotFoundException unconditionally for an unknown one; src/ops/update_telemetry_pipeline.rs:243 writes the new configuration and returns the empty 200. Both handlers are reached because nothing upstream rejected the request's credentials.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay fixture observabilityadmin-telemetry-pipeline-error-paths and stop at step task#3 (DeleteTelemetryPipeline with credentials AWS does not accept): the emulator answers 404 ResourceNotFoundException where the recording has 401 {"message": "Unauthorized"}.
3. Replay fixture observabilityadmin-telemetry-pipeline-workflow through step task#8 (UpdateTelemetryPipeline with the same unaccepted credentials): the emulator answers an empty 200 and the pipeline's stored configuration is replaced, where the recording has 401 {"message": "Unauthorized"} and no state change.
4. In both cases compare status, the JSON body's lowercase `message` member and the presence of the x-amzn-errortype header against the recorded response.
