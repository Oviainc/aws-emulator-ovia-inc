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
Issue-Commit: dafc31b5abf596ae8a242777f1e33ae02a8e9a54

# observabilityadmin telemetry pipeline mutations skip the sigv4 authorization pre-check and are served instead of 401 Unauthorized

> _Note: this issue was identified against the code base at commit `dafc31b5abf596ae8a242777f1e33ae02a8e9a54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTelemetryPipeline

## Description
The emulator's observabilityadmin telemetry-pipeline mutation handlers (DeleteTelemetryPipeline, UpdateTelemetryPipeline) perform no request-authorization check at all. A request that AWS's service frontend rejects before dispatch is instead routed into the operation handler, which then answers from state — either a resource-level 404 or an outright 200 success. Both causes below are the SAME missing pre-dispatch auth gate on the restJson1 ObservabilityAdmin router; they are counted separately because each is a distinct operation and a distinct recorded step a fixer must re-replay.

2 causes.

Cause 1 of 2: DeleteTelemetryPipeline answers an unauthorized delete with 404 ResourceNotFoundException instead of 401 Unauthorized.
  Fixture: observabilityadmin-telemetry-pipeline-error-paths, step task#3.
  AWS: Smithy service ObservabilityAdmin (version 2018-05-10, protocol restJson1, authSchemes [sigv4], sigv4Name 'observabilityadmin'); DeleteTelemetryPipeline is POST /DeleteTelemetryPipeline with modeled errors AccessDeniedException(400), ConflictException(409), ResourceNotFoundException(404), TooManyRequestsException(429), ValidationException(400), InternalServerException(500). The model therefore does NOT model a 401, because sigv4 authorization is enforced by the service frontend ahead of operation dispatch; the recorded response is that frontend rejection. Recorded behavior: HTTP 401 with JSON body {"message": "Unauthorized"} and an x-amzn-errortype response header present. Message rule: the fixed text is the single-word value "Unauthorized" under the lowercase member name 'message' — this envelope is NOT the modeled-error envelope (modeled ObservabilityAdmin errors use the capitalized 'Message' member), it has no variable parts, and it carries no request- or state-derived values. Authorization is evaluated before the pipeline identifier is resolved, so whether the identifier exists is irrelevant to the outcome.
  Emulator: HTTP 400... no — HTTP 404 with the modeled ResourceNotFoundException envelope: x-amzn-errortype: ResourceNotFoundException and a capitalized 'Message' member; the expected lowercase '$.message' member is absent entirely (replay: "status: expected 401, got 404"; "$.message: missing (expected 'Unauthorized', got None)"; "$headers.x-amzn-errortype: shape_mismatch ... got 'ResourceNotFoundException'"). The emulator skipped authorization and went straight to the state lookup for the pipeline identifier.
  Why: a client that is unauthorized (unsigned request, missing/malformed Authorization header, expired or otherwise unusable credentials) is told the pipeline does not exist. SDK error handling branches on this: ResourceNotFoundException is a terminal, non-retryable 'already gone' signal, so delete-if-exists and reconciliation logic treats the unauthorized call as a completed no-op and stops, while against AWS the 401 would surface as an authorization failure that must be fixed or re-signed. Credential and signing misconfiguration becomes invisible and is misreported as missing resources.
  Fix: the ObservabilityAdmin request router / auth middleware, ahead of the DeleteTelemetryPipeline handler — reject requests that fail the sigv4 authorization pre-check with 401 and the non-modeled {"message": "Unauthorized"} envelope before any identifier resolution or state access, rather than inside the operation's handler after the lookup.
  Traces: observabilityadmin-telemetry-pipeline-error-paths#task#3.

Cause 2 of 2: UpdateTelemetryPipeline answers an unauthorized update with 200 and an empty body instead of 401 Unauthorized — an unauthorized mutation reported as success.
  Fixture: observabilityadmin-telemetry-pipeline-workflow, step task#8.
  AWS: Smithy UpdateTelemetryPipeline is POST /UpdateTelemetryPipeline, input PipelineIdentifier (required, @length 1..512) and Configuration (required), output shape UpdateTelemetryPipelineOutput (empty, http code 200 defaulted), modeled errors AccessDeniedException(400), ResourceNotFoundException(404), TooManyRequestsException(429), ValidationException(400), InternalServerException(500) — again no modeled 401, because the rejection happens in the sigv4-authorizing frontend before dispatch. Recorded behavior: HTTP 401 with body {"message": "Unauthorized"} and x-amzn-errortype present. Same message rule as Cause 1: fixed text "Unauthorized" under the lowercase 'message' member, no variable parts.
  Emulator: HTTP 200 with an empty JSON body {} and no x-amzn-errortype header (replay: "status: expected 401, got 200"; "$: empty_body (expected {'message': 'Unauthorized'}, got {})"; "$headers.x-amzn-errortype: missing"). Because UpdateTelemetryPipelineOutput is legitimately empty, the emulator's success response is indistinguishable from a real update, so the unauthorized write is acknowledged as applied.
  Why: strictly worse than Cause 1 — a caller with no valid authorization receives a success for a configuration mutation. Clients treat 200 on an idempotent-looking update as 'configuration is now what I sent' and proceed (flip traffic, mark a rollout complete, stop retrying), while against AWS nothing was changed and the caller is told so with a 401. It also hides broken credentials in automation and makes authorization behavior untestable against the emulator.
  Fix: same component as Cause 1 — the ObservabilityAdmin router / auth middleware ahead of the UpdateTelemetryPipeline handler; the gate must run before the handler mutates or returns the empty success shape.
  Traces: observabilityadmin-telemetry-pipeline-workflow#task#8.

Related but NOT covering these: the open issues 20261030T012240Z-observabilityadmin-deletes3tableintegration-answers-an-unknown-integration-arn-with-400.md and 20261031T203350Z-observabilityadmin-gettelemetrypipeline-data-source-name-and-validatetelemetrypipelineconfiguration-restriction-errors.md own the other observabilityadmin divergences in this corpus (S3-table-integration 404-vs-400 and the GetTelemetryPipeline / ValidateTelemetryPipelineConfiguration body mismatches). Neither concerns request authorization, and neither carries the two replay keys above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200, data listener accepting).

Cause 1 — DeleteTelemetryPipeline, fixture observabilityadmin-telemetry-pipeline-error-paths step task#3:
  expected_status 401, actual_status 404
  detail: status: expected 401, got 404
          $.message: missing (expected 'Unauthorized', got None)
          $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')
  Expected (AWS): 401, body {"message": "Unauthorized"}
  Actual (emulator): 404, modeled ResourceNotFoundException envelope with capitalized 'Message', x-amzn-errortype: ResourceNotFoundException

Cause 2 — UpdateTelemetryPipeline, fixture observabilityadmin-telemetry-pipeline-workflow step task#8:
  expected_status 401, actual_status 200
  detail: status: expected 401, got 200
          $: empty_body (expected {'message': 'Unauthorized'}, got {})
          $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  Expected (AWS): 401, body {"message": "Unauthorized"}
  Actual (emulator): 200, body {}, no x-amzn-errortype

Model evidence (services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json):
  service ObservabilityAdmin, version 2018-05-10, protocols [restJson1], authSchemes [sigv4], sigv4Name observabilityadmin
  DeleteTelemetryPipeline: POST /DeleteTelemetryPipeline; errors AccessDeniedException(400), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), TooManyRequestsException(429), ValidationException(400) — no 401 modeled
  UpdateTelemetryPipeline: POST /UpdateTelemetryPipeline; output empty, http code 200 defaulted; errors AccessDeniedException(400), InternalServerException(500), ResourceNotFoundException(404), TooManyRequestsException(429), ValidationException(400) — no 401 modeled

Neither cluster carries a stub_path or an owned_by, so neither is already filed; each cluster has affected_count 1.

## Steps to Reproduce
1. Boot the emulator from this checkout and confirm it serves observabilityadmin (/__admin/health answers 200).
2. Replay the wire corpus: replay_wire_corpus() over trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture observabilityadmin-telemetry-pipeline-error-paths and inspect step task#3 — the DeleteTelemetryPipeline call the recording issues without usable sigv4 authorization. Observe the emulator answers 404 with a ResourceNotFoundException envelope (x-amzn-errortype: ResourceNotFoundException, capitalized 'Message') where the recording has 401 with body {"message": "Unauthorized"}.
4. Cause 2: replay fixture observabilityadmin-telemetry-pipeline-workflow and inspect step task#8 — the UpdateTelemetryPipeline call the recording issues without usable sigv4 authorization. Observe the emulator answers 200 with body {} and no x-amzn-errortype where the recording has 401 with body {"message": "Unauthorized"}.
5. After fixing, re-replay both bindings — observabilityadmin-telemetry-pipeline-error-paths#task#3 and observabilityadmin-telemetry-pipeline-workflow#task#8 — and require each to answer 401 with the lowercase-'message' Unauthorized envelope before dispatching to the operation handler.
