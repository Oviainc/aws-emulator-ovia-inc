Issue-Source: trace-sweep-divergence
Issue-Trace: rum-batch-delete-partial-metrics
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-batch-delete-partial-metrics rum-custom-metrics-create-and-get rum-metric-definitions-cloudwatch-lifecycle rum-update-metric-definition-roundtrip rum-metric-definitions-error-paths rum-metric-operations-not-found-errors rum-app-monitor-and-metrics-error-paths
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|BatchCreateRumMetricDefinitions|2xx->2xx|$.MetricDefinitions[*]
Issue-Key: rum|BatchGetRumMetricDefinitions|2xx->2xx|$.MetricDefinitions[*].Namespace
Issue-Key: rum|BatchGetRumMetricDefinitions|2xx->2xx|$.MetricDefinitions[*]
Issue-Key: rum|BatchDeleteRumMetricDefinitions|2xx->2xx|$.Errors[*].ErrorMessage
Issue-Key: rum|UpdateRumMetricDefinition|5xx->4xx|$.message
Issue-Key: rum|UpdateRumMetricDefinition|5xx->4xx|$headers.x-amzn-errortype
Issue-Key: rum|UpdateRumMetricDefinition|5xx->4xx|status
Issue-Key: rum|UpdateRumMetricDefinition|4xx->4xx|$.message
Issue-Key: rum|DeleteRumMetricsDestination|2xx->4xx|$
Issue-Key: rum|DeleteRumMetricsDestination|2xx->4xx|status
Issue-Commit: 1c82a6d99cb1f7308f6a13b71a678a6984dedc7f

# RUM metric-definition and metrics-destination responses diverge in body members, per-item message and error class

> _Note: this issue was identified against the code base at commit `1c82a6d99cb1f7308f6a13b71a678a6984dedc7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateRumMetricDefinitions

## Description
This issue bundles SEVEN distinct net-new root causes in the RUM metric-definition and metrics-destination control plane, found by wire replay. All seven must be closed before submitting; each is numbered and carries its own failing fixture steps.

Cause 1 of 7: BatchCreateRumMetricDefinitions returns a MetricDefinitions member that AWS does not send.
AWS: Smithy: BatchCreateRumMetricDefinitionsResponse has `Errors` (@required) and `MetricDefinitions` (OPTIONAL). Recorded behavior: a successful create answers HTTP 200 with the Errors list only — the MetricDefinitions member is ABSENT from the body entirely, for single-item and multi-item batches alike.
Emulator: it adds a MetricDefinitions array echoing every created definition with a freshly minted MetricDefinitionId (observed 3-element, 2-element and 1-element arrays of {MetricDefinitionId, Name, EventPattern, Namespace}).
Why: the member is the emulator's invention on this operation, so a client that treats its presence as "here are the ids I just created" works against the emulator and gets nothing back from AWS — the correct way to learn the ids is BatchGetRumMetricDefinitions. Round-trip and golden-body comparisons also diverge on every create.
Fix: the BatchCreateRumMetricDefinitions handler/serializer must omit the optional MetricDefinitions member from the success response and return Errors only.
Traces: rum-batch-delete-partial-metrics#setup#3, rum-custom-metrics-create-and-get#task#3, rum-metric-definitions-cloudwatch-lifecycle#task#3 (the latter also covers rum-update-metric-definition-roundtrip#setup#3).

Cause 2 of 7: BatchGetRumMetricDefinitions omits the Namespace of an EXTENDED metric, which AWS reports as AWS/RUM.
AWS: Smithy: MetricDefinitionRequest.Namespace documents "If this structure is for a custom metric instead of an extended metrics, use this parameter to define the metric namespace ... Do not specify this parameter if this structure is for an extended metric" and "You cannot use any string that starts with AWS/ for your namespace"; MetricDefinition.Namespace (@length 1..237, @pattern "[a-zA-Z0-9-._/#:]+$") is the member read back. Recorded behavior: a definition created WITHOUT a Namespace (an extended metric, e.g. a page-view or JS-error metric) is read back with Namespace set to the vended namespace "AWS/RUM"; a custom metric keeps the namespace the request supplied.
Emulator: for the definition created without a Namespace, BatchGetRumMetricDefinitions omits the member altogether ($.MetricDefinitions[0].Namespace: expected 'AWS/RUM', got None), while the sibling custom definition in the same batch round-trips its own namespace correctly.
Why: the namespace is where the metric is actually published; a client that reads it back to build a CloudWatch GetMetricData/alarm request for an extended metric gets no namespace at all and cannot locate the metric. It also erases the only distinction between "extended metric in AWS/RUM" and "namespace unknown".
Fix: the metric-definition read path (BatchGetRumMetricDefinitions projection, or the state that stores a definition) must settle the omitted Namespace to AWS/RUM for extended metrics rather than storing/serving it as absent.
Traces: rum-custom-metrics-create-and-get#task#4 (also covers rum-metric-definitions-cloudwatch-lifecycle#task#4).

Cause 3 of 7: after a partially failing BatchDeleteRumMetricDefinitions the emulator keeps the definitions it reported as deleted.
AWS: Smithy: BatchDeleteRumMetricDefinitions is a batch operation whose response carries both `MetricDefinitionIds` (the ids it deleted) and the per-item `Errors`; its http binding is DELETE /rummetrics/{AppMonitorName}/metrics with the ids in the metricDefinitionIds query. Recorded behavior: when a batch mixes valid ids with an unknown id, the valid ids ARE deleted and the unknown one is reported per-item — a following BatchGetRumMetricDefinitions lists exactly the surviving definitions (in the recording, the definition that survives at position 0 is the third one created, 'MetricC').
Emulator: the same BatchGetRumMetricDefinitions after the partial delete still reports the definition at position 0 as 'MetricA' — the entry the partial batch was supposed to remove ($.MetricDefinitions[0]: expected the surviving definition, got {Name: 'MetricA', ...} under the same id slot). (Hypothesis: the handler abandons the whole batch when one item errors instead of applying the valid deletions, so the state is left as it was.)
Why: this is a state divergence, not a formatting one — the per-item Errors list says the batch partly succeeded while nothing was actually deleted, so a reconciler that trusts MetricDefinitionIds/Errors believes definitions are gone that still publish metrics, and a retry loop never converges.
Fix: the BatchDeleteRumMetricDefinitions handler and the delete path in the service state: the valid ids must be removed even when another item in the same batch fails.
Traces: rum-batch-delete-partial-metrics#task#6.

Cause 4 of 7: BatchDeleteRumMetricDefinitions' per-item error message is the emulator's own sentence instead of AWS's.
AWS: Smithy: BatchDeleteRumMetricDefinitionsError{MetricDefinitionId, ErrorCode, ErrorMessage} — all three @required. Recorded behavior: deleting an unknown or already-deleted MetricDefinitionId answers HTTP 200 with the id echoed in Errors[].MetricDefinitionId, ErrorCode "ResourceNotFoundException" and ErrorMessage exactly the fixed text "Resource not found" — no variable parts at all.
Emulator: ErrorMessage is "App monitor <app monitor name> has no metric definition <metric definition id>." — its fixed text and its two interpolated values (the app monitor name from the request path and the id from the query) are both absent from AWS's message. Status, per-item channel and ErrorCode are correct.
Why: the per-item error carries no discriminator beyond code and message, so a client or log pipeline matching the message misclassifies the item. The open BEHAVIORAL issue 20261029T192752Z-rum-metric-definition-and-metrics-destination-control-plane.md describes the same wording as its Cause 4, but it carries no wire replay key, so this fixture step is not covered by it and is filed here.
Fix: the per-item error construction in the BatchDeleteRumMetricDefinitions handler.
Traces: rum-metric-definitions-cloudwatch-lifecycle#task#6.

Cause 5 of 7: UpdateRumMetricDefinition for an unknown MetricDefinitionId answers a client 404 where AWS answers a server 500.
AWS: Smithy: UpdateRumMetricDefinition (PATCH /rummetrics/{AppMonitorName}/metrics, MetricDefinitionId @required) models both ResourceNotFoundException (client, httpError 404) and InternalServerException (server, httpError 500, retryable, message @required). Recorded behavior: when the destination exists but the MetricDefinitionId does not, the service answers HTTP 500 with x-amzn-errortype "InternalServerException" and a message whose fixed text is "Error adding metric definitions for app monitor " + " to destination." with the variable part being the app monitor name from the request path.
Emulator: HTTP 404 with x-amzn-errortype "ResourceNotFoundException" and message "App monitor <name> has no metric definition <id>." — wrong status, wrong error class and wrong message.
Why: the error class decides client behavior: InternalServerException is modeled retryable and the SDK's standard retry policy retries it, while ResourceNotFoundException is terminal. A client updating a definition against AWS retries and eventually surfaces a 5xx; against the emulator it aborts immediately with a not-found it never sees in production, and the reverse-direction test (a resilience test asserting the retry happens) silently passes.
Fix: the UpdateRumMetricDefinition handler's unknown-definition branch — it must construct InternalServerException (500) with the message above, not ResourceNotFoundException.
Traces: rum-metric-definitions-cloudwatch-lifecycle#task#5 (also covers rum-metric-definitions-error-paths#task#5).

Cause 6 of 7: UpdateRumMetricDefinition's "destination not configured" message is worded about the app monitor instead of the destination.
AWS: Smithy: UpdateRumMetricDefinitionRequest.Destination (@required, MetricDestination) names the destination the update applies to, and ResourceNotFoundException (client, 404) is modeled. Recorded behavior: when the app monitor exists but the named metrics destination was never put, the refusal is HTTP 404 ResourceNotFoundException whose message's fixed text is "Error adding metrics - Destination " + " not found." with the variable part being the Destination value from the request (recorded with the CloudWatch destination).
Emulator: the status and the error code are right, but the message is "App monitor <app monitor name> has no CloudWatch metrics destination." — different fixed text, and it interpolates the app monitor name (which AWS's message does not contain) while hard-coding the destination into the sentence.
Why: the message is the only part of this refusal that says WHICH of the two preconditions failed (missing monitor vs missing destination); a client branching on it to decide whether to call PutRumMetricsDestination or CreateAppMonitor takes the wrong branch, and message assertions fail.
Fix: the destination-lookup branch of the UpdateRumMetricDefinition handler must word the refusal as AWS does, with the request's Destination as the only variable part.
Traces: rum-metric-operations-not-found-errors#task#4.

Cause 7 of 7: DeleteRumMetricsDestination refuses a destination that was never configured instead of succeeding as a no-op.
AWS: Smithy: DeleteRumMetricsDestination is DELETE /rummetrics/{AppMonitorName}/metricsdestination, @idempotent, with an EMPTY output shape and http code 200; ResourceNotFoundException and ConflictException are modeled but recorded behavior is that deleting a destination that is NOT configured on an EXISTING app monitor SUCCEEDS — HTTP 200 with an empty body `{}` and no error (recorded behavior only; the idempotency is not stated in the model).
Emulator: HTTP 404 with body {"__type": "ResourceNotFoundException", "message": "App monitor <app monitor name> has no CloudWatch metrics destination."}.
Why: the operation is modeled @idempotent, so teardown and reconcile paths delete unconditionally; against the emulator every such path aborts with an exception, and a caller cannot distinguish "app monitor missing" (a real ResourceNotFoundException) from "destination already absent" (a success in AWS).
Fix: the DeleteRumMetricsDestination handler — the absent-destination branch must return the empty 200 success response; only a missing app monitor keeps ResourceNotFoundException. The open BEHAVIORAL issue 20261029T192752Z-rum-metric-definition-and-metrics-destination-control-plane.md states the same rule as its Cause 7 but carries no wire replay key, so this fixture step is filed here.
Traces: rum-app-monitor-and-metrics-error-paths#task#5.

## Evidence
Wire replay of trace set simparity-wire-aws-v1: 46 fixtures replayed, 239/279 steps passed, 40 mismatched, 0 errored, 0 blocked, score 85.7%. Every divergence is in service `rum` (bootstrapped as service-com-amazonaws-rum-rum). None of the ten clusters below reported a stub_path or an owned_by.

Cause 1 — BatchCreateRumMetricDefinitions, expected 200 / got 200:
  rum-batch-delete-partial-metrics#setup#3: $.MetricDefinitions[0..2]: extra (expected None, got three entries {MetricDefinitionId, Name: MetricA/MetricB/MetricC, EventPattern '{"event_type":["com.amazon.rum.custom_event"]}', Namespace BatchDeleteTest}).
  rum-custom-metrics-create-and-get#task#3: $.MetricDefinitions[0],[1]: extra (expected None, got PageViewCount and CustomClickCount entries).
  rum-metric-definitions-cloudwatch-lifecycle#task#3: $.MetricDefinitions[0]: extra (expected None, got the JsErrorCount entry); same cluster covers rum-update-metric-definition-roundtrip#setup#3.

Cause 2 — BatchGetRumMetricDefinitions, expected 200 / got 200:
  rum-custom-metrics-create-and-get#task#4: $.MetricDefinitions[0].Namespace: missing (expected 'AWS/RUM', got None). Also affects rum-metric-definitions-cloudwatch-lifecycle#task#4. The sibling custom metric's own namespace round-trips, so only the extended-metric default is missing.

Cause 3 — BatchGetRumMetricDefinitions after a partial BatchDelete, expected 200 / got 200:
  rum-batch-delete-partial-metrics#task#6: $.MetricDefinitions[0]: expected {Name 'MetricC', EventPattern '{"event_type":["com.amazon.rum.custom_event"]}', Namespace 'BatchDeleteTest', DimensionKeys/UnitLabel/ValueKey null}, got {Name 'MetricA', ...} — i.e. the entry the partial batch delete should have removed is still present.

Cause 4 — BatchDeleteRumMetricDefinitions, expected 200 / got 200:
  rum-metric-definitions-cloudwatch-lifecycle#task#6: $.Errors[0].ErrorMessage: expected 'Resource not found', got 'App monitor <app monitor name> has no metric definition <id>.' (recorded instance: app monitor sim2real-probe-rum-rum-metrics-app-a2d050a2, id 00000000-0000-0000-0000-000000000000).

Cause 5 — UpdateRumMetricDefinition:
  rum-metric-definitions-cloudwatch-lifecycle#task#5: status expected 500, got 404; $headers.x-amzn-errortype expected 'InternalServerException', got 'ResourceNotFoundException'; $.message expected 'Error adding metric definitions for app monitor <app monitor name> to destination.', got 'App monitor <app monitor name> has no metric definition <id>.' Also affects rum-metric-definitions-error-paths#task#5.

Cause 6 — UpdateRumMetricDefinition, expected 404 / got 404:
  rum-metric-operations-not-found-errors#task#4: $.message expected 'Error adding metrics - Destination CloudWatch not found.' (fixed text around the request's Destination value), got 'App monitor <app monitor name> has no CloudWatch metrics destination.'

Cause 7 — DeleteRumMetricsDestination:
  rum-app-monitor-and-metrics-error-paths#task#5: status expected 200, got 404; body expected {} (empty success), got {'__type': 'ResourceNotFoundException', 'message': 'App monitor <app monitor name> has no CloudWatch metrics destination.'}

## Steps to Reproduce
Replay fixture rum-batch-delete-partial-metrics step setup#3 (the representative) and the eight fixture#step bindings in trace_ids. Manual repro per cause (setup for all: CreateAppMonitor(Name=m, Domain=example.com); PutRumMetricsDestination(AppMonitorName=m, Destination=CloudWatch)):

Cause 1: BatchCreateRumMetricDefinitions(AppMonitorName=m, Destination=CloudWatch, MetricDefinitions=[{Name:'MetricA', Namespace:'BatchDeleteTest', EventPattern:'{"event_type":["com.amazon.rum.custom_event"]}'}, ...]) -> observed a MetricDefinitions array in the 200 body; expected a body carrying Errors only, with no MetricDefinitions member.

Cause 2: create one EXTENDED metric definition with NO Namespace (e.g. Name 'PageViewCount', EventPattern '{"event_type":["com.amazon.rum.page_view_event"]}') plus one custom definition with its own Namespace, then BatchGetRumMetricDefinitions(AppMonitorName=m, Destination=CloudWatch) -> observed the extended definition with no Namespace member; expected Namespace 'AWS/RUM' on it and the custom namespace preserved on the other.

Cause 3/4: create MetricA, MetricB, MetricC; BatchDeleteRumMetricDefinitions(AppMonitorName=m, Destination=CloudWatch, MetricDefinitionIds=[<MetricA id>, '00000000-0000-0000-0000-000000000000']) -> expected 200 with MetricA's id in MetricDefinitionIds and one Errors entry {MetricDefinitionId: the unknown id, ErrorCode: 'ResourceNotFoundException', ErrorMessage: 'Resource not found'}; then BatchGetRumMetricDefinitions must list only MetricB and MetricC. Observed: the per-item ErrorMessage is the 'App monitor ... has no metric definition ...' sentence AND MetricA is still listed.

Cause 5: UpdateRumMetricDefinition(AppMonitorName=m, Destination=CloudWatch, MetricDefinitionId='00000000-0000-0000-0000-000000000000', MetricDefinition={...valid...}) -> observed 404 ResourceNotFoundException; expected 500 with x-amzn-errortype InternalServerException and message 'Error adding metric definitions for app monitor m to destination.'

Cause 6: on a FRESH app monitor with NO metrics destination put, UpdateRumMetricDefinition(AppMonitorName=m2, Destination=CloudWatch, MetricDefinitionId=<any>, MetricDefinition={...}) -> expected 404 ResourceNotFoundException 'Error adding metrics - Destination CloudWatch not found.'; observed 'App monitor m2 has no CloudWatch metrics destination.'

Cause 7: on a fresh app monitor with no destination configured, DeleteRumMetricsDestination(AppMonitorName=m2, Destination=CloudWatch) -> expected HTTP 200 with an empty body; observed 404 ResourceNotFoundException. Repeat Put/Delete/Delete: the second Delete must also be a 200 no-op.
