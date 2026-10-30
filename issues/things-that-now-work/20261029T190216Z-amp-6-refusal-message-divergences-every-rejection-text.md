Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amp.anomaly-detector-evaluation-interval-range-rejected amp.logging-configuration-lifecycle amp.resource-policy-lifecycle amp.resource-policy-stale-revision-conflict-rejected amp.workspace-delete-lifecycle amp.workspace-double-delete-conflict-rejected
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: amp.anomaly-detector-evaluation-interval-range-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: amp.logging-configuration-lifecycle|run#28|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: amp.resource-policy-lifecycle|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: amp.resource-policy-stale-revision-conflict-rejected|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: amp.workspace-delete-lifecycle|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: amp.workspace-double-delete-conflict-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 600bb9247975905af81e2aa2cda8a015f9396423

# amp: 6 refusal-message divergences — every rejection text is emulator-authored prose instead of the service's fixed message (CreateAnomalyDetector, logging configuration, resource policy, workspace not-found and double-delete)

> _Note: this issue was identified against the code base at commit `600bb9247975905af81e2aa2cda8a015f9396423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWorkspace (also DescribeWorkspace, CreateAnomalyDetector, DescribeLoggingConfiguration/UpdateLoggingConfiguration, DescribeResourcePolicy/DeleteResourcePolicy, PutResourcePolicy)

## Description
SIX net-new root causes in the amp (Amazon Managed Service for Prometheus) emulator, all found by the behavioral suite. amp IS served (its bootstrap issue is resolved and 15/22 amp checks pass), so these are fidelity defects, not bootstrap gaps. In every case the HTTP status and the error CODE match AWS; only the Message body text diverges, and in each case the emulator emits its own wording (often a client-side-style 'Member must ...' constraint sentence) where AWS emits a fixed, service-authored sentence. An error message is part of the contract: assertion/golden tests, log scrapers and user-facing surfaces all carry it. SIX causes must be closed before submitting; each cause names its own failing check.

Related open issue 20261022T110707Z-amp-3-fidelity-divergences-prometheusendpoint-announces-the-real.md covers earlier amp divergences on some of the same operations (CreateAnomalyDetector's missing server-side RCF rule, TagResource's not-found CODE); none of the six message texts below is described there, and that issue's RCF cause is about the rule EXISTING at all — the rule now fires, with the wrong text.

Cause 1 of 6: CreateAnomalyDetector's ValidationException for an RCF query whose root is not a supported aggregation operator uses the emulator's own constraint sentence.
  AWS: Smithy: CreateAnomalyDetector (POST /workspaces/{workspaceId}/anomalydetectors, success 202) takes required `configuration` (AnomalyDetectorConfiguration, member randomCutForest.query) and models ValidationException @httpError 400. Recorded behavior: a syntactically valid but unwrapped query (e.g. query "up") is refused 400 ValidationException with the message "Unsupported PromQL expression for RandomCutForest, expression's root must be a supported aggregation operator" — that whole sentence is fixed text with NO variable parts (it does not echo the rejected query, the member path, or the operator list). No detector is created, and the same alias with a wrapped query ("sum(up)") is accepted.
  Emulator: same code and status, but the message is 'Invalid configuration.randomCutForest.query: Member must be wrapped by one of the supported PromQL aggregation operators (avg, count, group, max, min, quantile, stddev, stdvar, sum)' — an emulator-authored 'Invalid <member path>: Member must ...' sentence in the shape of a client-side constraint violation, enumerating operators AWS never lists in this message.
  Why: a client asserting on, or parsing, AWS's documented rejection text for a bad anomaly-detector expression never matches; the wording also misleadingly presents a server-side semantic refusal as a client-side member-constraint failure, so callers mis-attribute it to their SDK's input validation.
  Fix: CreateAnomalyDetector's request validation / refusal construction in the amp service (services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/create_anomaly_detector.rs and whatever shared validation helper composes the 'Invalid <member>: Member must ...' text) must emit the service's fixed sentence for this class instead of the generic constraint template.
  Traces: amp.anomaly-detector-evaluation-interval-range-rejected

Cause 2 of 6: the logging-configuration ResourceNotFoundException message omits the resource family AWS names.
  AWS: Smithy: DescribeLoggingConfiguration (GET /workspaces/{workspaceId}/logging) and UpdateLoggingConfiguration (PUT, 202) both model ResourceNotFoundException @httpError 404. Recorded behavior: after DeleteLoggingConfiguration, both the read and the update on that workspace are refused 404 ResourceNotFoundException with the message "Workspace logging configuration not found." — fixed text, no variable parts (the workspaceId is not interpolated).
  Emulator: same code/status, message 'Logging configuration not found.' — AWS's sentence with the leading "Workspace " dropped.
  Why: a client distinguishing a missing WORKSHOP-level logging configuration from other not-found families by message text sees the wrong family; golden-output and log-assertion tests fail against the emulator while passing against AWS.
  Fix: the not-found refusal in the logging-configuration handlers of the amp service (describe_logging_configuration.rs / update_logging_configuration.rs, or the shared helper they call) must carry AWS's full sentence.
  Traces: amp.logging-configuration-lifecycle

Cause 3 of 6: the resource-policy ResourceNotFoundException message uses a different noun phrase than AWS.
  AWS: Smithy: DescribeResourcePolicy (GET /workspaces/{workspaceId}/policy) and DeleteResourcePolicy (DELETE, 202) both model ResourceNotFoundException @httpError 404. Recorded behavior: after DeleteResourcePolicy, both a second delete and the read are refused 404 ResourceNotFoundException with the message "Workspace resource policy not found." — fixed text, no variable parts.
  Emulator: same code/status, message 'Resource-based policy not found.' — a different noun phrase ("Resource-based policy" instead of "Workspace resource policy").
  Why: identical to cause 2 — any caller keyed on the message text to tell which resource family is absent gets a string AWS never emits.
  Fix: the not-found refusal in describe_resource_policy / delete_resource_policy of the amp service (or the shared policy-lookup helper) must emit AWS's sentence.
  Traces: amp.resource-policy-lifecycle

Cause 4 of 6: PutResourcePolicy's stale-revisionId ValidationException message is an emulator-authored constraint sentence.
  AWS: Smithy: PutResourcePolicy (PUT /workspaces/{workspaceId}/policy, 202) takes an optional `revisionId` and models ValidationException @httpError 400. Recorded behavior: a put whose revisionId is no longer the current revision is refused 400 ValidationException with the message "Can't conditionally update a workspace resource policy with a wrong revisionId." — fixed text, no variable parts (neither the supplied nor the current revisionId is interpolated), and the stored document/revision is left untouched.
  Emulator: same code/status, message 'Invalid revisionId: Member must name the current revision of the resource-based policy, or NO_POLICY while the workspace holds none' — an 'Invalid <member>: Member must ...' template plus an emulator-specific NO_POLICY sentinel that is not part of AWS's wording for this refusal.
  Why: clients that assert on the optimistic-lock rejection text (and humans reading it) get a message AWS never produces, and the NO_POLICY wording suggests an input vocabulary AWS does not document here.
  Related: fixed issue 20261023T031439Z-amp-2-error-code-divergences-on-rejection-paths.md corrected this refusal's error CODE to ValidationException (the code now matches); its message text was not part of that fix and still diverges, so this is the remaining half of the same refusal path, not a regression of it.
  Fix: PutResourcePolicy's revision-precondition refusal in the amp service (put_resource_policy.rs) must compose AWS's fixed sentence rather than routing through the generic member-constraint message builder.
  Traces: amp.resource-policy-stale-revision-conflict-rejected

Cause 5 of 6: DescribeWorkspace's not-found message carries a trailing period AWS does not use, so the read and write paths are not distinguished as AWS distinguishes them.
  AWS: Smithy: DescribeWorkspace (GET /workspaces/{workspaceId}) and DeleteWorkspace (DELETE, 202) both model ResourceNotFoundException @httpError 404. Recorded behavior: for a workspace that has finished deleting, DescribeWorkspace is refused 404 ResourceNotFoundException with the message "Workspace not found" (NO trailing period), while DeleteWorkspace on the same gone id is refused with "Workspace not found." (WITH the trailing period). Both are fixed texts; the workspaceId is not interpolated. The per-operation difference is what the recording shows.
  Emulator: emits 'Workspace not found.' (with the period) on BOTH paths, so the read path's text diverges while the write path's happens to match.
  Why: a client or test that matches AWS's read-path text exactly never matches the emulator; more generally the emulator single-homes one sentence where the service has two, so the distinction is unreproducible.
  Fix: the workspace not-found refusal used by DescribeWorkspace in the amp service (the per-handler inline refusal text — see the open maintainability issue 20261029T150024Z-amp-createanomalydetector-maintainability.md, which notes this refusal is composed inline in 28 handlers) must use the read-path spelling without the trailing period, keeping the delete path's spelling with it.
  Traces: amp.workspace-delete-lifecycle, amp.workspace-double-delete-conflict-rejected

Cause 6 of 6: DeleteWorkspace's wrong-status ConflictException message is rephrased.
  AWS: Smithy: DeleteWorkspace models ConflictException @httpError 409. Recorded behavior: a second DeleteWorkspace while the workspace is still DELETING is refused 409 ConflictException with the message "Can't delete workspace in non-ACTIVE state. Current status is DELETING" — fixed text "Can't delete workspace in non-ACTIVE state. Current status is " followed by one variable part, the workspace's current statusCode taken from service state (DELETING here); there is no trailing period.
  Emulator: same code/status, message 'The workspace is DELETING; deletions are allowed only when status is ACTIVE' — an entirely reworded sentence that puts the state first and drops AWS's fixed phrasing.
  Why: the state-conflict message is the one signal a caller racing deletes has; rewritten, it breaks message assertions and any operator tooling that greps for AWS's phrasing.
  Fix: DeleteWorkspace's status-precondition refusal in the amp service (delete_workspace.rs) must build AWS's template with the current status substituted.
  Traces: amp.workspace-double-delete-conflict-rejected

## Evidence
amp.anomaly-detector-evaluation-interval-range-rejected [mismatch]
  AssertionError: expected message "Unsupported PromQL expression for RandomCutForest, expression's root must be a supported aggregation operator", got 'Invalid configuration.randomCutForest.query: Member must be wrapped by one of the supported PromQL aggregation operators (avg, count, group, max, min, quantile, stddev, stdvar, sum)'
  (code ValidationException and status 400 matched; only the message diverged)

amp.logging-configuration-lifecycle [mismatch]
  AssertionError: expected message 'Workspace logging configuration not found.', got 'Logging configuration not found.'
  (ResourceNotFoundException/404 matched, on DescribeLoggingConfiguration after delete; the same text is asserted for UpdateLoggingConfiguration)

amp.resource-policy-lifecycle [mismatch]
  AssertionError: expected message 'Workspace resource policy not found.', got 'Resource-based policy not found.'
  (ResourceNotFoundException/404 matched, on DescribeResourcePolicy and on a repeat DeleteResourcePolicy after delete)

amp.resource-policy-stale-revision-conflict-rejected [mismatch]
  AssertionError: expected message "Can't conditionally update a workspace resource policy with a wrong revisionId.", got 'Invalid revisionId: Member must name the current revision of the resource-based policy, or NO_POLICY while the workspace holds none'
  (ValidationException/400 matched)

amp.workspace-delete-lifecycle [mismatch]
  AssertionError: expected message 'Workspace not found', got 'Workspace not found.'
  (DescribeWorkspace on a fully deleted workspace; ResourceNotFoundException/404 matched. The same check expects 'Workspace not found.' WITH the period from the subsequent DeleteWorkspace, which the emulator does produce.)

amp.workspace-double-delete-conflict-rejected [mismatch]
  AssertionError: expected message "Can't delete workspace in non-ACTIVE state. Current status is DELETING", got 'The workspace is DELETING; deletions are allowed only when status is ACTIVE'
  (ConflictException/409 matched; this check also asserts the DescribeWorkspace text 'Workspace not found' of cause 5 later in the run)

## Steps to Reproduce
Cause 1: amp.create_workspace(alias=...); poll describe_workspace until ACTIVE; amp.create_anomaly_detector(workspaceId=..., alias=..., configuration={'randomCutForest': {'query': 'up'}}, evaluationIntervalInSeconds=60) — the refusal is ValidationException/400 (correct) but its Message is the 'Invalid configuration.randomCutForest.query: Member must be wrapped by ...' sentence instead of "Unsupported PromQL expression for RandomCutForest, expression's root must be a supported aggregation operator".

Cause 2: create a workspace, wait ACTIVE; logs.create_log_group(...); amp.create_logging_configuration(workspaceId=..., logGroupArn=<arn>); wait ACTIVE; amp.delete_logging_configuration(workspaceId=...); then amp.describe_logging_configuration(workspaceId=...) and amp.update_logging_configuration(workspaceId=..., logGroupArn=<arn>) — read Error.Message: 'Logging configuration not found.' instead of 'Workspace logging configuration not found.'.

Cause 3: create a workspace, wait ACTIVE; amp.put_resource_policy(workspaceId=..., policyDocument=<doc>); wait policyStatus ACTIVE; amp.delete_resource_policy(workspaceId=..., revisionId=<rev>); then amp.describe_resource_policy(workspaceId=...) and a second amp.delete_resource_policy(...) — read Error.Message: 'Resource-based policy not found.' instead of 'Workspace resource policy not found.'.

Cause 4: create a workspace, wait ACTIVE; put a policy (revisionId r1), wait ACTIVE; put again with revisionId=r1 to get r2, wait ACTIVE; put a third document with the now-stale revisionId=r1 — read Error.Message: the 'Invalid revisionId: Member must name the current revision ...' sentence instead of "Can't conditionally update a workspace resource policy with a wrong revisionId.".

Cause 5: create a workspace, wait ACTIVE; amp.delete_workspace(workspaceId=...); poll amp.describe_workspace(workspaceId=...) until it refuses — Error.Message is 'Workspace not found.' where AWS answers 'Workspace not found' (no trailing period) for the describe path.

Cause 6: create a workspace, wait ACTIVE; amp.delete_workspace(workspaceId=...) once (workspace goes DELETING); immediately amp.delete_workspace(workspaceId=...) again — the refusal is ConflictException/409 (correct) with Message 'The workspace is DELETING; deletions are allowed only when status is ACTIVE' instead of "Can't delete workspace in non-ACTIVE state. Current status is DELETING".
