Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.batch-create-metric-definition-duplicate-name-conflict rum.batch-create-metric-definition-duplicate-rejected rum.batch-create-metric-definitions-malformed-namespace-rejected rum.representation-metric-definition-numeric-precision-preserved rum.update-rum-metric-definition-concurrent-conflict rum.batch-delete-metric-definition-removes-it rum.batch-delete-metric-definitions-partial-errors rum.telemetry-batch-delete-definitions-removed rum.batch-delete-metric-definitions-missing-permission-denied rum.batch-get-rum-metric-definitions-invalid-destination-rejected rum.put-metrics-destination-invalid-arn-rejected rum.delete-rum-metrics-destination-unknown-conflict
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum.batch-create-metric-definition-duplicate-name-conflict|run#14|AssertionError|assert second.get("MetricDefinitions", []) == [], (
Issue-Key: rum.batch-create-metric-definition-duplicate-rejected|run#18|AssertionError|assert len(errors) == 1, f"expected exactly one per-item error, got {errors}"
Issue-Key: rum.batch-create-metric-definitions-malformed-namespace-rejected|run#13|AssertionError|raise AssertionError(f"expected a ValidationException, but the call succeeded: {result!r}")
Issue-Key: rum.representation-metric-definition-numeric-precision-preserved|run#12|AssertionError|assert create_resp["Errors"] == [], repr([e.get("ErrorCode") for e in create_resp["Errors"]])
Issue-Key: rum.update-rum-metric-definition-concurrent-conflict|run#8|AssertionError|assert not _unexpected, "codes=" + ",".join(str(e.get("ErrorCode")) for e in _unexpected)
Issue-Key: rum.batch-delete-metric-definition-removes-it|run#24|AssertionError|assert redelete_errs[0].get("ErrorMessage") == "Resource not found", (
Issue-Key: rum.batch-delete-metric-definitions-partial-errors|run#22|AssertionError|assert error["ErrorMessage"] == "Resource not found", (
Issue-Key: rum.telemetry-batch-delete-definitions-removed|run#22|AssertionError|assert redelete_errs[0].get("ErrorMessage") == "Resource not found", (
Issue-Key: rum.batch-delete-metric-definitions-missing-permission-denied|_error#16|AssertionError|raise AssertionError(f"expected the call to fail, but it succeeded and returned {result!r}")
Issue-Key: rum.batch-get-rum-metric-definitions-invalid-destination-rejected|run#9|AssertionError|assert malformed_message == expected_malformed, (
Issue-Key: rum.put-metrics-destination-invalid-arn-rejected|run#10|AssertionError|assert message == (
Issue-Key: rum|DeleteRumMetricsDestination|ResourceNotFoundException
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# RUM metric-definition and metrics-destination control plane: 7 divergences (no duplicate ConflictException, reserved-namespace accepted, valid nested EventPattern refused, wrong per-item not-found message, delete not authorized, wrong pattern text, delete-destination not idempotent)

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCreateRumMetricDefinitions

## Description
This issue bundles SEVEN distinct net-new root causes in the CloudWatch RUM metric-definition / metrics-destination control plane. All seven must be closed before submitting; each is numbered and carries its own failing check ids.

Cause 1 of 7: BatchCreateRumMetricDefinitions silently re-creates an already existing metric definition instead of reporting a per-item ConflictException.
AWS: Smithy models ConflictException on BatchCreateRumMetricDefinitions and a per-item BatchCreateRumMetricDefinitionsError{MetricDefinition, ErrorCode, ErrorMessage} (all three required) in BatchCreateRumMetricDefinitionsResponse.Errors. Recorded behavior: resubmitting a definition whose CONTENT is identical (same Name + Namespace + EventPattern) for the same AppMonitorName/Destination answers HTTP 200 with MetricDefinitions EMPTY and exactly one Errors entry: ErrorCode "ConflictException", ErrorMessage whose fixed text is "Metric definition already exists: MetricDefinitionRequest(name=, valueKey=, unitLabel=, dimensionKeys=, eventPattern=, namespace=)" with the variable parts filled from the request's MetricDefinition members and the literal `null` for each member the request omitted (e.g. valueKey=null, unitLabel=null, dimensionKeys=null). The pre-existing definition keeps its id and content. The conflict key is the definition's CONTENT, not its Name alone: the same Name with a DIFFERENT EventPattern is not a conflict and creates a second coexisting definition (recorded behavior only). Deleting the original frees the identity and makes the identical call succeed again.
Emulator: the second, identical create answers 200 with Errors=[] and a MetricDefinitions entry carrying a freshly minted MetricDefinitionId (observed: Errors [], MetricDefinitions [{MetricDefinitionId <new uuid>, Name DuplicateMetric, EventPattern ..., Namespace ...}]); in the second check the response had Errors=[] as well, so no per-item error is ever produced.
Why: a client that uses the batch API's per-item Errors list to detect "already defined" never sees the conflict, so idempotent provisioning loops (CDK/Terraform-style reconcilers) create duplicate definitions and double-publish the same CloudWatch metric, and the client's own dedupe-by-error logic is dead code.
Fix: the BatchCreateRumMetricDefinitions handler and the metric-definition insert path in the service state (the identity/uniqueness check that must be made before minting an id, plus the per-item error it must emit).
Traces: rum.batch-create-metric-definition-duplicate-name-conflict, rum.batch-create-metric-definition-duplicate-rejected.

Cause 2 of 7: a custom metric that reuses the reserved AWS/RUM namespace is reported per-item with a 200 instead of refusing the whole request.
AWS: MetricDefinitionRequest.Namespace documents "You cannot use any string that starts with AWS/ for your namespace" and the shape's doc states AWS/RUM is validated as the extended-metric namespace with a fixed set of recognized Names. Recorded behavior: BatchCreateRumMetricDefinitions with MetricDefinitions=[{Name=<custom name>, Namespace="AWS/RUM", EventPattern=...}] is refused for the WHOLE call with the modeled ValidationException (HTTP 400), message fixed text "Metric value  is not valid." with the variable part being the request's MetricDefinition.Name (recorded: "Metric value BadCustomMetric is not valid."), and nothing is persisted.
Emulator: HTTP 200 with MetricDefinitions=[] and a per-item Errors entry {ErrorCode "ValidationException", ErrorMessage "Namespace AWS/RUM can be used only for extended metrics, and BadCustomMetric is not an extended metric: a custom metric is published to a namespace of its own."} — wrong status class (2xx vs 4xx), wrong error channel (per-item vs operation error) and wrong message text.
Why: botocore raises nothing, so a caller's `except ClientError` path never runs; the request looks accepted and the invalid namespace surfaces only as an item buried in a 200 body.
Fix: the BatchCreateRumMetricDefinitions validation step — the reserved-namespace rule must be a whole-request ValidationException raised before any per-item processing, worded as above.
Traces: rum.batch-create-metric-definitions-malformed-namespace-rejected.

Cause 3 of 7: BatchCreateRumMetricDefinitions refuses a VALID custom metric definition whose EventPattern contains nested match objects.
AWS: MetricDefinitionRequest.EventPattern's own documentation gives exactly these shapes as valid examples, including '{ "event_type": ["com.amazon.rum.performance_navigation_event"], "metadata": { "browserName": [ "Chrome", "Firefox" ] }, "event_details": { "duration": [{ "numeric": [ ">=", 2000, "<", 8000 ] }] } }'. A custom definition using a non-AWS/ Namespace, such a nested EventPattern and (optionally) ValueKey/DimensionKeys is accepted: Errors is empty and the definition is created.
Emulator: the create returns a per-item error with ErrorCode "ValidationException" for such definitions. Two independent checks hit it: one with {"event_type":[...], "event_details":{"duration":[{"numeric":[">=",1234.567891,"<",987654.321098]}]}} plus ValueKey="event_details.duration", one with {"event_type":[...], "metadata":{"browserName":[...],"countryCode":[...]}}. A flat EventPattern ({"event_type":[...]}) with the same Namespace is accepted, so the rejection is tied to the nested keys. (Hypothesis: the EventPattern validator only accepts the top-level `event_type` key and treats `metadata`/`event_details` sub-objects — or the numeric clause inside them — as invalid.)
Why: every custom metric that filters on metadata or thresholds on event_details (the documented and overwhelmingly common case) cannot be created at all, and because the refusal arrives as a per-item ValidationException inside a 200 it is also not raised to the client as an error.
Fix: the metric-definition validation for EventPattern in the BatchCreateRumMetricDefinitions/UpdateRumMetricDefinition validation path.
Traces: rum.representation-metric-definition-numeric-precision-preserved, rum.update-rum-metric-definition-concurrent-conflict.

Cause 4 of 7: BatchDeleteRumMetricDefinitions' per-item ResourceNotFoundException carries a bespoke message instead of AWS's.
AWS: BatchDeleteRumMetricDefinitionsError{MetricDefinitionId, ErrorCode, ErrorMessage} is the per-item error. Recorded behavior: deleting an unknown or already-deleted MetricDefinitionId yields HTTP 200, the id echoed in Errors[].MetricDefinitionId, ErrorCode "ResourceNotFoundException" and ErrorMessage exactly the fixed text "Resource not found" — no variable parts at all.
Emulator: ErrorMessage is "App monitor <app monitor name> has no metric definition <metric definition id>." (observed in three separate checks, e.g. "App monitor sim2real-rum-am-4071d208caa5 has no metric definition does-not-exist-0000000000000000."). The status, the per-item channel and the ErrorCode are right; only the message text diverges.
Why: clients and log pipelines that match on the error message string (RUM's per-item errors carry no other discriminator beyond the code) misclassify the item; the emulator also leaks a message shape AWS never emits.
Fix: the per-item error construction in the BatchDeleteRumMetricDefinitions handler.
Traces: rum.batch-delete-metric-definition-removes-it, rum.batch-delete-metric-definitions-partial-errors, rum.telemetry-batch-delete-definitions-removed.

Cause 5 of 7: BatchDeleteRumMetricDefinitions performs no authorization check, so an unprivileged principal deletes metric definitions.
AWS: AccessDeniedException is modeled on BatchDeleteRumMetricDefinitions. Recorded behavior: an IAM user with no identity-based policy at all is refused with AccessDeniedException (HTTP 403), message fixed text "User:  is not authorized to perform: rum:BatchDeleteRumMetricDefinitions on resource:  because no identity-based policy allows the rum:BatchDeleteRumMetricDefinitions action" with the variable parts being the caller's principal ARN (arn:aws:iam::<account>:user/<user>) and the app monitor ARN (arn:aws:rum:<region>:<account>:appmonitor/<name>); the definition is still present afterwards.
Emulator: the call signed with that user's freshly created access key answers HTTP 200 with {"Errors": [], "MetricDefinitionIds": ["<id>"]} — the definition is actually DELETED by an unauthorized caller. Note the open issue 20261028T102158Z-rum-getappmonitor-wrong-behavior.md asserts the opposite failure mode (an IAM user's attached policies being invisible so RUM over-refuses) and claims the four metric-definition calls all reach `authorize`; this replay shows BatchDeleteRumMetricDefinitions reaching no authorization decision at all, so that issue does not cover this.
Why: an over-permissive destructive operation — any caller with network reach deletes another principal's metric definitions, and a client's permission-boundary tests pass against the emulator while failing in AWS.
Fix: the BatchDeleteRumMetricDefinitions handler must route through the service's authorization decision (src/state/authorization.rs, as its sibling metric-definition handlers do) before mutating state.
Traces: rum.batch-delete-metric-definitions-missing-permission-denied.

Cause 6 of 7: the destinationArn ValidationException quotes the modeled @pattern without the leading ".*" AWS advertises.
AWS: Smithy: PutRumMetricsDestinationRequest.DestinationArn and BatchGetRumMetricDefinitionsRequest.DestinationArn carry @pattern "arn:[^:]*:[^:]*:[^:]*:[^:]*:.*" (unanchored) and @length 0..2048; refusal is the service-common ValidationException. Recorded behavior: a malformed DestinationArn is refused HTTP 400 ValidationException whose message is the standard framework sentence — fixed text "1 validation error detected: Value '' at '' failed to satisfy constraint: Member must satisfy regular expression pattern: " with the variable parts being the rejected value, the member path `destinationArn`, and the pattern as AWS advertises it: ".*arn:[^:]*:[^:]*:[^:]*:[^:]*:.*" (the modeled expression prefixed with ".*").
Emulator: identical message but the pattern is quoted as "arn:[^:]*:[^:]*:[^:]*:[^:]*:.*" (no leading ".*"), on both PutRumMetricsDestination and BatchGetRumMetricDefinitions.
Why: this message is the documented contract for the character grammar — a caller that lifts the advertised expression out of the message and re-applies it locally gets a different (anchored-at-start) grammar than AWS publishes, and message-equality assertions fail.
Fix: the pattern-rendering step of the destinationArn constraint refusal (the crate's single constraint-refusal wording in src/state/validation.rs, which currently reproduces the model pattern verbatim for this member).
Traces: rum.batch-get-rum-metric-definitions-invalid-destination-rejected, rum.put-metrics-destination-invalid-arn-rejected.

Cause 7 of 7: DeleteRumMetricsDestination refuses a destination that was never configured instead of being an idempotent no-op.
AWS: ResourceNotFoundException and ConflictException are modeled on DeleteRumMetricsDestination, but recorded behavior is that deleting a destination that is NOT configured on an existing app monitor SUCCEEDS as a no-op (HTTP 200, no error) and ListRumMetricsDestinations keeps reporting an empty Destinations list; deleting the same destination twice after a real Put is likewise a no-op the second time. (Recorded behavior only — the model does not state the idempotency; it is the Delete/Get resource-POLICY pair that rejects an absent policy, not this operation.)
Emulator: ResourceNotFoundException (HTTP 400/404) with message "App monitor <name> has no CloudWatch metrics destination." for the never-configured delete.
Why: teardown and reconcile paths that unconditionally delete the destination (including every cleanup helper in these checks) abort with an exception against the emulator where AWS accepts them, and a caller cannot distinguish "app monitor missing" from "destination absent".
Fix: the DeleteRumMetricsDestination handler — the absent-destination branch must return the empty success response rather than constructing ResourceNotFoundException (the app-monitor-missing branch keeps ResourceNotFoundException).
Traces: rum.delete-rum-metrics-destination-unknown-conflict.

## Evidence
Suite: 51/81 passed; all 30 failures are in service `rum` (bootstrapped as service-com-amazonaws-rum-rum). None of these 12 checks was marked ALREADY FILED or OWNED by.

Cause 1 — rum.batch-create-metric-definition-duplicate-name-conflict:
  expected: 200, MetricDefinitions=[], Errors=[{ErrorCode: ConflictException, ErrorMessage: "Metric definition already exists: MetricDefinitionRequest(name=DuplicateMetric, valueKey=null, unitLabel=null, dimensionKeys=null, eventPattern={\"event_type\": [\"com.amazon.rum.js_error_event\"]}, namespace=<namespace>)"}]
  actual:   200, Errors=[], MetricDefinitions=[{'MetricDefinitionId': '82200780-94a9-4468-a375-7e6616453a2b', 'Name': 'DuplicateMetric', ...}]
  rum.batch-create-metric-definition-duplicate-rejected: AssertionError: expected exactly one per-item error, got []

Cause 2 — rum.batch-create-metric-definitions-malformed-namespace-rejected:
  expected: ClientError ValidationException, Message "Metric value BadCustomMetric is not valid."
  actual:   HTTPStatusCode 200, Errors=[{'MetricDefinition': {'Name': 'BadCustomMetric', 'Namespace': 'AWS/RUM', ...}, 'ErrorCode': 'ValidationException', 'ErrorMessage': 'Namespace AWS/RUM can be used only for extended metrics, and BadCustomMetric is not an extended metric: a custom metric is published to a namespace of its own.'}], MetricDefinitions=[]

Cause 3 — rum.representation-metric-definition-numeric-precision-preserved: AssertionError: ['ValidationException'] (assert create_resp["Errors"] == [] for Namespace sim2real/PrecisionNamespace, ValueKey event_details.duration, EventPattern {"event_type": [...], "event_details": {"duration": [{"numeric": [">=", 1234.567891, "<", 987654.321098]}]}}).
  rum.update-rum-metric-definition-concurrent-conflict: AssertionError: codes=ValidationException (create of Namespace sim2real/CustomMetrics with EventPattern {"event_type": [...], "metadata": {"browserName": ["Chrome","Firefox","Safari"], "countryCode": ["US","JP","DE"]}}). The same Namespace with a flat EventPattern is accepted in rum.batch-delete-metric-definitions-partial-errors, which gets past create.

Cause 4 — rum.batch-delete-metric-definitions-partial-errors:
  expected ErrorMessage='Resource not found'; actual 'App monitor sim2real-rum-am-4071d208caa5 has no metric definition does-not-exist-0000000000000000.'
  rum.batch-delete-metric-definition-removes-it: actual {'MetricDefinitionId': '9d33ad25-...', 'ErrorCode': 'ResourceNotFoundException', 'ErrorMessage': 'App monitor sim2real-rum-app-0d5e839127a2 has no metric definition 9d33ad25-....'}
  rum.telemetry-batch-delete-definitions-removed: same shape ('App monitor sim2real-behav-appmon-28a52e789b98 has no metric definition 85f34a7c-....').

Cause 5 — rum.batch-delete-metric-definitions-missing-permission-denied:
  expected: AccessDeniedException, "User: arn:aws:iam::<account>:user/<user> is not authorized to perform: rum:BatchDeleteRumMetricDefinitions on resource: arn:aws:rum:<region>:<account>:appmonitor/<app> because no identity-based policy allows the rum:BatchDeleteRumMetricDefinitions action"
  actual:   AssertionError: expected the call to fail, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Errors': [], 'MetricDefinitionIds': ['db2e3db2-bbed-45fc-b520-fe9528d05476']}

Cause 6 — rum.batch-get-rum-metric-definitions-invalid-destination-rejected:
  expected message "1 validation error detected: Value 'not-a-valid-arn' at 'destinationArn' failed to satisfy constraint: Member must satisfy regular expression pattern: .*arn:[^:]*:[^:]*:[^:]*:[^:]*:.*"
  actual message "... regular expression pattern: arn:[^:]*:[^:]*:[^:]*:[^:]*:.*"
  rum.put-metrics-destination-invalid-arn-rejected: identical divergence on PutRumMetricsDestination (Destination=Evidently, DestinationArn='not-a-valid-arn', syntactically valid IamRoleArn).

Cause 7 — rum.delete-rum-metrics-destination-unknown-conflict:
  expected: DeleteRumMetricsDestination on a never-configured CloudWatch destination succeeds (200) and ListRumMetricsDestinations stays empty.
  actual:   botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteRumMetricsDestination operation: App monitor sim2real-rum-am-5465c688e422 has no CloudWatch metrics destination.

## Steps to Reproduce
Replay the 12 checks listed in trace_ids against the emulator. Minimal manual repro per cause:

Cause 1: CreateAppMonitor(Name=m, Domain=example.com); PutRumMetricsDestination(AppMonitorName=m, Destination=CloudWatch); BatchCreateRumMetricDefinitions(AppMonitorName=m, Destination=CloudWatch, MetricDefinitions=[{Name:"DuplicateMetric", Namespace:"custom_ns", EventPattern:'{"event_type": ["com.amazon.rum.js_error_event"]}'}]) -> created; issue the IDENTICAL call again -> observed 200 with a new MetricDefinitionId and Errors=[]; expected 200 with MetricDefinitions=[] and one ConflictException item worded as in the description. Then BatchGetRumMetricDefinitions must show exactly one definition with that Name, with the original id and EventPattern.

Cause 2: same setup, then BatchCreateRumMetricDefinitions(MetricDefinitions=[{Name:"BadCustomMetric", Namespace:"AWS/RUM", EventPattern:'{"event_type": ["com.amazon.rum.js_error_event"]}'}]) -> observed 200 + per-item error; expected 400 ValidationException "Metric value BadCustomMetric is not valid." and nothing persisted.

Cause 3: same setup, then BatchCreateRumMetricDefinitions(MetricDefinitions=[{Name:"PrecisionCustomMetric", Namespace:"sim2real/PrecisionNamespace", ValueKey:"event_details.duration", EventPattern:'{"event_type": ["com.amazon.rum.performance_navigation_event"], "event_details": {"duration": [{"numeric": [">=", 1234.567891, "<", 987654.321098]}]}}'}]) -> observed per-item ValidationException; expected Errors=[] and the definition created, round-tripping the EventPattern unchanged. Repeat with a "metadata" sub-object pattern.

Cause 4: same setup with one real definition, then BatchDeleteRumMetricDefinitions(MetricDefinitionIds=[<real id>, "does-not-exist-0000000000000000"]) -> observed per-item ErrorMessage "App monitor <name> has no metric definition <id>."; expected exactly "Resource not found". Also re-delete an id already deleted.

Cause 5: create an IAM user with NO policies and an access key; signed with it call BatchDeleteRumMetricDefinitions(AppMonitorName=m, Destination=CloudWatch, MetricDefinitionIds=[<id>]) -> observed 200 and the definition gone; expected 403 AccessDeniedException and the definition still present.

Cause 6: PutRumMetricsDestination(AppMonitorName=m, Destination="Evidently", DestinationArn="not-a-valid-arn", IamRoleArn="arn:aws:iam::<account>:role/whatever") and BatchGetRumMetricDefinitions(AppMonitorName=m, Destination="Evidently", DestinationArn="not-a-valid-arn") -> compare the quoted pattern in the ValidationException message against ".*arn:[^:]*:[^:]*:[^:]*:[^:]*:.*".

Cause 7: CreateAppMonitor(Name=m2, Domain=example.com); ListRumMetricsDestinations(m2) -> []; DeleteRumMetricsDestination(AppMonitorName=m2, Destination=CloudWatch) -> observed ResourceNotFoundException; expected 200 no-op with the list still empty. Then Put, Delete, Delete again -> the second Delete must also be a 200 no-op.
