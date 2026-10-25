Issue-Source: trace-sweep-divergence
Issue-Trace: migrationhubstrategy-assessment-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: migrationhubstrategy-assessment-lifecycle
Issue-Shape-Id: com.amazonaws.migrationhubstrategy#AWSMigrationHubStrategyRecommendation
Issue-Key: migrationhub-strategy|GetLatestAssessmentId|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: migrationhub-strategy|GetLatestAssessmentId|4xx->4xx|status
Issue-Commit: 7b52a53863c32195473bcccd44fb6923fe37ec7d

# GetLatestAssessmentId answers 400 ValidationException instead of 404 ResourceNotFoundException when no assessment has been started

> _Note: this issue was identified against the code base at commit `7b52a53863c32195473bcccd44fb6923fe37ec7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLatestAssessmentId

## Description
Cause 1 of 1: the "no assessment has ever been started in this account/Region" condition on GetLatestAssessmentId is reported as a 400 ValidationException instead of the 404 ResourceNotFoundException AWS answers.

AWS: recorded behavior only for the error selection — Smithy GetLatestAssessmentId (GET /get-latest-assessment-id, success 200) takes an EMPTY input (input: {}) and declares errors AccessDeniedException (403), DependencyException (500), InternalServerException (500) and ValidationException (400); ResourceNotFoundException is NOT in that operation's error list, though the shape exists in the same model with errorKind=client, httpError=404 and the doc "The specified ID in the request is not found." The recorded prod response for the empty-store case is HTTP 404 with x-amzn-errortype: ResourceNotFoundException, so AWS reports the missing latest-assessment resource through the not-found shape even though the model omits it from this operation. Note the operation has no input members at all, so there is nothing a caller could have sent wrongly and a validation refusal is not an available reading of this condition.

Emulator: answers HTTP 400 with x-amzn-errortype: ValidationException. This is a deliberate choice, not an oversight: services/service-com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation/src/ops/get_latest_assessment_id.rs documents that "This operation declares no not-found, so the one client-side refusal a caller can see from it is that ValidationException" and maps the None branch of state.latest_assessment_id to ValidationException(crate::state::no_latest_assessment()). The comment reasons from the model's error list rather than from the wire, and the wire disagrees.

Why: 404/ResourceNotFoundException is the signal a client uses to decide "nothing assessed yet, start an assessment"; 400/ValidationException is the signal that the REQUEST is malformed. An SDK client or polling loop that branches on ResourceNotFoundException to bootstrap the assessment lifecycle never takes that branch and instead surfaces a non-retryable client-error for a request that was perfectly well formed — on a fresh account this is the FIRST call of the lifecycle, so the whole flow aborts before StartAssessment is ever issued. Status-class-based generic error handling likewise misclassifies a missing resource as caller error.

Fix: the GetLatestAssessmentId handler's not-found branch (ops/get_latest_assessment_id.rs). The empty-store branch must answer the 404 ResourceNotFoundException shape, widened into this operation's error channel via extra_errors the same way the crate already widens it for StopAssessment, rather than being folded into the modeled ValidationException. Pure validation refusals on this operation stay 400 — but with an empty input shape there are none.

Traces: migrationhubstrategy-assessment-lifecycle#task#1 (expected 404 ResourceNotFoundException, got 400 ValidationException).

Related but NOT covering this: open issue 20261024T153403Z-migrationhubstrategy-stopassessment-validation covers the OPPOSITE direction (StopAssessment and other id-bearing members answering 404 ResourceNotFoundException where a @pattern violation should give 400 ValidationException) on different operations, so it does not own this step.

## Evidence
Replay divergence (fixture migrationhubstrategy-assessment-lifecycle, step task#1, verdict=mismatch, affected_count=1):
  status: expected 404, got 400
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException', got 'ValidationException'
  divergence_key: service=migrationhub-strategy|GetLatestAssessmentId|status=404->400|keys=$headers.x-amzn-errortype,status
  stub_path: "" (not an unimplemented operation), owned_by: [] (no open issue carries this replay key)

Spec: smithy_query(query="operation", operation="GetLatestAssessmentId") -> input_shape GetLatestAssessmentIdRequest with NO members; http {method: GET, uri: /get-latest-assessment-id, code: 200}; errors = AccessDeniedException(403), DependencyException(500), InternalServerException(500), ValidationException(400). smithy_query(query="shape", shape="ResourceNotFoundException") -> errorKind client, httpError 404, doc "The specified ID in the request is not found."

Emulator source: services/service-com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation/src/ops/get_latest_assessment_id.rs:160-171 — `let latest = state.lock().latest_assessment_id.clone(); match latest { Some(id) => Ok(...), None => Err(GetLatestAssessmentIdErrorModeled::ValidationException(crate::state::no_latest_assessment())) }`, with the module doc at lines 60-63 and the comment at lines 80-85 stating the choice to route the no-assessment case through ValidationException because the operation "declares no not-found".

Requirement asserted: HTTP status 404 and error code ResourceNotFoundException. The recorded error message text is an example only.

## Steps to Reproduce
1. Boot the emulator with a fresh (empty) migrationhub-strategy store — no StartAssessment has been issued, so state.latest_assessment_id is None.
2. Issue GET /get-latest-assessment-id against the migrationhub-strategy endpoint (SDK: MigrationHubStrategyRecommendations.GetLatestAssessmentId with an empty request).
3. Observe HTTP 400 with header x-amzn-errortype: ValidationException.
4. Expected, per the recorded prod wire response in fixture migrationhubstrategy-assessment-lifecycle step task#1: HTTP 404 with header x-amzn-errortype: ResourceNotFoundException.
5. Re-replay the corpus (replay_wire_corpus) and confirm migrationhubstrategy-assessment-lifecycle#task#1 passes.
