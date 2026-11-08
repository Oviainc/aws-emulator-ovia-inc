Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.batch-delete-metric-definitions-missing-permission-denied
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum.batch-delete-metric-definitions-missing-permission-denied|run#19|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: a9b33a473a9762dcd2804e6444a9b0695dc27129

# BatchDeleteRumMetricDefinitions' AccessDeniedException says "because no policy allows" instead of AWS's "because no identity-based policy allows"

> _Note: this issue was identified against the code base at commit `a9b33a473a9762dcd2804e6444a9b0695dc27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDeleteRumMetricDefinitions

## Description
This issue carries ONE net-new root cause in the CloudWatch RUM metric-definition control plane.

Cause 1 of 1: the authorization refusal for BatchDeleteRumMetricDefinitions is worded with the generic "because no policy allows ..." clause instead of the IAM wording AWS emits when the denial comes from the absence of an identity-based policy.

AWS: Smithy (services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json) models AccessDeniedException on BatchDeleteRumMetricDefinitions (errorKind client, httpError 403; DELETE /rummetrics/{AppMonitorName}/metrics). The model does not state the message text, so recorded behavior only: a caller whose principal has NO identity-based policy granting the action is refused with AccessDeniedException whose message is the standard IAM implicit-deny sentence. Its FIXED text is "User: <principal-arn> is not authorized to perform: rum:BatchDeleteRumMetricDefinitions on resource: <resource-arn> because no identity-based policy allows the rum:BatchDeleteRumMetricDefinitions action" — i.e. the fixed parts are "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the " and " action"; the VARIABLE parts are the caller's principal ARN taken from the signing credentials (arn:aws:iam::<account>:user/<user-name>), the target app monitor's ARN built from the request (arn:aws:rum:<region>:<account>:appmonitor/<AppMonitorName>), and the action name rum:BatchDeleteRumMetricDefinitions, which appears twice. The reason clause is what identifies the kind of denial: an implicit deny with no identity-based policy in play says "no identity-based policy allows"; it is not the bare "no policy allows" phrasing. The call's effect must be absent (the definitions stay readable via BatchGetRumMetricDefinitions).

Emulator: the status (403) and the error code (AccessDeniedException) are now correct and the delete is correctly refused with no partial deletion, but the reason clause is shortened. Observed message: "User: arn:aws:iam::<account>:user/<user-name> is not authorized to perform: rum:BatchDeleteRumMetricDefinitions on resource: arn:aws:rum:<region>:<account>:appmonitor/<app-monitor-name> because no policy allows the rum:BatchDeleteRumMetricDefinitions action" — every variable part is filled correctly; the literal "identity-based " is missing from the fixed text.

Why: the IAM denial message is the only machine-readable discriminator a client has between an implicit deny from a missing identity-based policy and the other denial classes (explicit deny in an identity-based/resource-based/SCP policy, permissions boundary, session policy). Permission-diagnostics tooling, access-review automation and test suites that match the documented sentence classify the emulator's denial as an unknown reason or fail outright, and runbooks that tell an operator to attach an identity-based policy are not triggered.

Note: the open issue 20261029T192752Z-rum-metric-definition-and-metrics-destination-control-plane.md (its Cause 5) reported this operation performing NO authorization check at all and quoted the required wording; the authorization decision is now made, so that cause's replay key no longer matches and this message-wording divergence is not covered by it. It is also not a reopening of a refusal: the refusal itself is what AWS does.

Fix: the AccessDeniedException message construction used by the BatchDeleteRumMetricDefinitions handler's authorization path (the crate's shared IAM-denial wording helper in the service state's authorization module) must emit the "no identity-based policy allows" reason clause for an implicit deny with no identity-based policy, for every operation that renders this sentence.

Traces: rum.batch-delete-metric-definitions-missing-permission-denied.

## Evidence
Suite: 51/81 passed; all 30 failures are in service `rum` (bootstrapped as service-com-amazonaws-rum-rum). 29 of the 30 were marked OWNED by an already-filed issue; this is the only surviving failure.

[mismatch] rum.batch-delete-metric-definitions-missing-permission-denied
check_rum_batch_delete_metric_definitions_missing_permission_denied.py, line 73:
  assert message == expected_message
AssertionError:
  expected: 'User: arn:aws:iam::000000000000:user/sim2real-rum-deny-user-0d082037d117 is not authorized to perform: rum:BatchDeleteRumMetricDefinitions on resource: arn:aws:rum:us-west-2:000000000000:appmonitor/sim2real-rum-metrics-ac1378fd5369 because no identity-based policy allows the rum:BatchDeleteRumMetricDefinitions action'
  got:      'User: arn:aws:iam::000000000000:user/sim2real-rum-deny-user-0d082037d117 is not authorized to perform: rum:BatchDeleteRumMetricDefinitions on resource: arn:aws:rum:us-west-2:000000000000:appmonitor/sim2real-rum-metrics-ac1378fd5369 because no policy allows the rum:BatchDeleteRumMetricDefinitions action'

The error CODE assertion (AccessDeniedException) one line earlier passed, and the "effect absent" and positive-control steps were never reached. The only difference is the absence of the literal 'identity-based ' in the reason clause; the principal ARN, app-monitor ARN and account/region/user/app-monitor names shown above are per-run values of that run, not part of the requirement.

## Steps to Reproduce
1. CreateAppMonitor(Name=<m>, Domain="sim2real-behav.example.com").
2. PutRumMetricsDestination(AppMonitorName=<m>, Destination="CloudWatch").
3. BatchCreateRumMetricDefinitions(AppMonitorName=<m>, Destination="CloudWatch", MetricDefinitions=[{Name:"Sim2RealVisitCount", Namespace:"Sim2RealCustomMetrics", ValueKey:"event_details.visit_count", UnitLabel:"Count", EventPattern:'{"event_type": ["sim2real_custom_event"]}'}]); resolve the MetricDefinitionId via BatchGetRumMetricDefinitions.
4. iam.CreateUser(UserName=<u>) with NO policies attached; iam.CreateAccessKey(UserName=<u>).
5. Signed with that access key, call BatchDeleteRumMetricDefinitions(AppMonitorName=<m>, Destination="CloudWatch", MetricDefinitionIds=[<id>]).
6. Observed: ClientError AccessDeniedException (403) with Message '... because no policy allows the rum:BatchDeleteRumMetricDefinitions action'. Expected: the same status and code with Message '... because no identity-based policy allows the rum:BatchDeleteRumMetricDefinitions action', the principal ARN and app-monitor ARN filled from the signing credentials and the request.
7. Replay rum.batch-delete-metric-definitions-missing-permission-denied to confirm (it also re-asserts that the definition is still present after the denial and that the permissioned caller can delete it).
