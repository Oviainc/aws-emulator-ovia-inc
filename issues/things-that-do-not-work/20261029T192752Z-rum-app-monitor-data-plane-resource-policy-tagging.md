Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.deploy-app-monitor-cfn-behaves rum.deploy-app-monitor-cfn-stack-behaves rum.deploy-app-monitor-cfn-tags-propagate rum.deploy-app-monitor-via-cloudformation-then-behaves rum.put-rum-events-payload-timestamp-preserved rum.e2e-app-monitor-lifecycle-with-metrics-and-events rum.put-rum-metrics-destination-access-denied rum.put-rum-events-missing-permission-denied rum.e2e-resource-policy-access-lifecycle rum.get-resource-policy-unknown-app-monitor-denied rum.put-resource-policy-malformed-document-rejected rum.resource-policy-malformed-document-rejected rum.put-resource-policy-concurrent-revision-conflict rum.put-resource-policy-stale-revision-conflict rum.resource-policy-revision-mismatch-rejected rum.stale-policy-revision-delete-rejected rum.tag-resource-invalid-key-format-rejected rum.tag-resource-invalid-key-rejected
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|GetAppMonitor|ResourceNotFoundException
Issue-Key: rum|ListTagsForResource|NotFoundException
Issue-Key: rum.put-rum-events-payload-timestamp-preserved|run#15|AssertionError|raise AssertionError(
Issue-Key: rum.e2e-app-monitor-lifecycle-with-metrics-and-events|_poll_for_event#30|Timeout|time.sleep(interval_s)
Issue-Key: rum.put-rum-metrics-destination-access-denied|run#12|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: rum.put-rum-events-missing-permission-denied|run#16|AssertionError|assert message == (
Issue-Key: rum.e2e-resource-policy-access-lifecycle|run#20|AssertionError|assert denied_message == expected_denial, denied_message
Issue-Key: rum.get-resource-policy-unknown-app-monitor-denied|run#16|AssertionError|assert validation_code == "ValidationException", (
Issue-Key: rum.put-resource-policy-malformed-document-rejected|run#8|AssertionError|assert message is None, message
Issue-Key: rum.resource-policy-malformed-document-rejected|run#9|AssertionError|assert err.get("Message") is None, f"expected a null message, got {err.get('Message')!r}"
Issue-Key: rum.put-resource-policy-concurrent-revision-conflict|run#16|AssertionError|assert message == "If policyRevisionID is provided, then it must match the existing revision.", message
Issue-Key: rum.put-resource-policy-stale-revision-conflict|run#13|AssertionError|assert message == "If policyRevisionID is provided, then it must match the existing revision.", message
Issue-Key: rum.resource-policy-revision-mismatch-rejected|run#16|AssertionError|assert err.get("Message") == _STALE_REVISION, (
Issue-Key: rum.stale-policy-revision-delete-rejected|run#15|AssertionError|assert err.get("Message") == _STALE_REVISION, (
Issue-Key: rum.tag-resource-invalid-key-format-rejected|run#10|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: rum.tag-resource-invalid-key-rejected|run#6|AssertionError|assert err.get("Message") == expected_message, (
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# RUM app-monitor data plane, resource policy, tagging and CFN provisioning: 9 divergences (CFN AppMonitor never created, PutRumEvents never readable, AccessDenied wording/resource, policy-name validation, malformed-policy message, revision-mismatch messages, tag refusal messages)

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRumEvents

## Description
This issue bundles NINE distinct net-new root causes across the RUM data plane, resource-policy API, tagging API and the CloudFormation provisioning of AWS::RUM::AppMonitor. All nine must be closed before submitting; each is numbered and carries its own failing check ids.

Cause 1 of 9: an AWS::RUM::AppMonitor deployed by CloudFormation reaches CREATE_COMPLETE but no app monitor exists in RUM.
AWS: creating a stack whose template contains Type: AWS::RUM::AppMonitor with Name/Domain/CwLogEnabled/AppMonitorConfiguration provisions a real app monitor: GetAppMonitor(Name=<the template's Name>) returns it (Id equal to the stack output !GetAtt AppMonitor.Id, Domain, AppMonitorConfiguration.AllowCookies, AppMonitorConfiguration.SessionSampleRate as configured), ListTagsForResource on its ARN returns the stack-propagated tags, and its data plane accepts PutRumEvents for that Id (recorded behavior only; RUM's Smithy model does not describe CloudFormation).
Emulator: the stack reaches CREATE_COMPLETE and even hands back an AppMonitorId output, but the monitor is absent from RUM: GetAppMonitor(Name=<template Name>) raises ResourceNotFoundException "Resource not found <name>" (three checks) and ListTagsForResource on the monitor ARN raises NotFoundException "Resource was not found" (one check). So the CFN resource provider mints an Id without registering the resource in the RUM service state.
Why: every infrastructure-as-code path is silently broken — the stack reports success, outputs an Id no API recognises, and every follow-up control-plane or data-plane call against the "deployed" monitor fails.
Fix: the CloudFormation resource provider / resource-type registration for AWS::RUM::AppMonitor must create the monitor through the same state path as CreateAppMonitor (name, domain, CwLogEnabled, AppMonitorConfiguration, stack tags) and delete it on stack delete.
Traces: rum.deploy-app-monitor-cfn-behaves, rum.deploy-app-monitor-cfn-stack-behaves, rum.deploy-app-monitor-cfn-tags-propagate, rum.deploy-app-monitor-via-cloudformation-then-behaves.

Cause 2 of 9: events accepted by PutRumEvents never become readable through GetAppMonitorData.
AWS: PutRumEvents accepts a batch for an app monitor Id and the events subsequently surface via GetAppMonitorData(Name=<monitor>, TimeRange={After, Before} in epoch MILLISECONDS) as JSON strings in Events[], each carrying event_id and event_timestamp (epoch milliseconds, sub-second precision preserved) (recorded behavior only).
Emulator: PutRumEvents returns success, but polling GetAppMonitorData over a window spanning the submission (now-1h .. now+1m, epoch ms, MaxResults 100, following NextToken to exhaustion) never yields the submitted event: "RUM event b31fa0ee-ef93-42a9-8d91-e43637090b24 never surfaced via GetAppMonitorData within 30s". The end-to-end lifecycle check is stuck in the same poll and is killed by the harness timeout. (Hypothesis: the ingested event is either not persisted into the data store GetAppMonitorData reads, or its stored timestamp/time-range comparison uses different units, so the window never matches.)
Why: the whole RUM data plane is write-only against the emulator — a client cannot verify ingestion, and any test or dashboard that reads back its own events sees an empty result rather than an error.
Fix: the PutRumEvents handler's event-store write and/or the GetAppMonitorData reader's time-range filter (the two must agree on where events are stored and on the timestamp unit).
Traces: rum.put-rum-events-payload-timestamp-preserved, rum.e2e-app-monitor-lifecycle-with-metrics-and-events.

Cause 3 of 9: the AccessDeniedException message uses a bespoke trailing clause instead of AWS's three distinct denial clauses.
AWS: AccessDeniedException is modeled on these operations; recorded behavior is the standard IAM denial sentence whose fixed text is "User:  is not authorized to perform:  on resource: " followed by ONE of these fixed clauses chosen by the kind of decision: (a) " because no identity-based policy allows the  action" when nothing grants it, (b) " with an explicit deny in an identity-based policy" when an identity policy denies it, (c) " with an explicit deny in a resource-based policy" when the app monitor's resource policy denies it. The variable parts are the caller's principal ARN, the action (e.g. rum:PutRumEvents, rum:PutRumMetricsDestination, rum:BatchDeleteRumMetricDefinitions) and the resource.
Emulator: it emits a single home-grown clause for every case — " with an explicit deny in a policy for the <action> action" (observed for both an identity-based deny on PutRumMetricsDestination and a resource-policy deny on PutRumEvents) and " because no policy allows the <action> action" instead of "because no identity-based policy allows the <action> action". The code (AccessDeniedException) and the leading text are correct; the trailing clause is wrong and does not distinguish identity-based from resource-based denials.
Why: the denial clause is the only machine-readable hint telling an operator WHICH policy refused the call; collapsing all three into one (and conflating identity with resource policies) sends debuggers to the wrong policy, and message assertions fail.
Fix: the authorization refusal renderer shared by the RUM handlers (src/state/authorization.rs refusal construction) must select among the three AWS clauses from the decision it already computes (not-stated vs identity deny vs resource-policy deny).
Traces: rum.put-rum-metrics-destination-access-denied, rum.put-rum-events-missing-permission-denied, rum.e2e-resource-policy-access-lifecycle.

Cause 4 of 9: a PutRumEvents denial names the app monitor's ARN where AWS names the app monitor Id.
AWS: for PutRumEvents (whose http uri is /appmonitors/{Id}/ and whose request member is Id) the denial message's "on resource: " slot is filled with the app monitor ID from the request — recorded in two independent checks, both expecting "... on resource: <app monitor Id> ..." (a UUID).
Emulator: it fills that slot with the derived app monitor ARN, "arn:aws:rum:<region>:<account>:appmonitor/<name>".
Why: the message names a resource identifier the caller never supplied for this operation; a caller correlating the denial back to the Id it sent (the only identifier PutRumEvents takes) cannot match it.
Fix: the PutRumEvents authorization refusal must pass the request's Id as the resource rendered in the message (the ARN stays the resource used for the policy evaluation itself).
Traces: rum.put-rum-events-missing-permission-denied, rum.e2e-resource-policy-access-lifecycle.

Cause 5 of 9: GetResourcePolicy does not apply the app-monitor Name pattern, so a malformed name is reported as not-found.
AWS: Smithy: the Name member of the policy operations targets AppMonitorName with @pattern "^(?!\\.)[\\.\\-_#A-Za-z0-9]+$" and @length 1..255; a value breaking that pattern is refused with the service-common ValidationException (HTTP 400) whose message's fixed text is "1 validation error detected: Value '' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: " with the variable parts being the rejected value and the pattern as AWS advertises it, "(?!\\.)[\\.\\-_#A-Za-z0-9]+" (unanchored). A well-formed but nonexistent name is a different outcome: ResourceNotFoundException with message "Resource not found " + the name. The two must be distinguishable.
Emulator: GetResourcePolicy(Name=".<something>") — a leading dot, which the pattern forbids — answers ResourceNotFoundException instead of ValidationException, so malformed input and missing resource are indistinguishable.
Why: a client cannot tell "you sent a bad name" (never retry, fix the input) from "that monitor does not exist" (create it / retry); the 4xx class is also wrong for an input error.
Fix: the GetResourcePolicy handler must run the Name constraint check (the crate's single constraint-refusal wording) before resolving the app monitor; the same ordering is needed on the sibling policy operations.
Traces: rum.get-resource-policy-unknown-app-monitor-denied.

Cause 6 of 9: MalformedPolicyDocumentException carries a message AWS does not send.
AWS: Smithy models MalformedPolicyDocumentException on PutResourcePolicy. Recorded behavior: a syntactically invalid PolicyDocument is refused with code MalformedPolicyDocumentException and NO message member at all (the error has no Message), and nothing is persisted (a following GetResourcePolicy answers PolicyNotFoundException "Resource policy does not exist").
Emulator: it attaches its JSON parser's diagnostic as the message — "The policy document is not valid JSON: expected ident at line 1 column 43." and "The policy document is not valid JSON: trailing comma at line 1 column 233." The code is right; the presence and the text of the message are not.
Why: the emulator invents an error field AWS leaves absent (and leaks parser internals and offsets that differ per document), so clients that render or assert on the message diverge.
Fix: the PutResourcePolicy handler's malformed-document branch must construct MalformedPolicyDocumentException with no message.
Traces: rum.put-resource-policy-malformed-document-rejected, rum.resource-policy-malformed-document-rejected.

Cause 7 of 9: PutResourcePolicy's stale-revision message is wrong.
AWS: Smithy models InvalidPolicyRevisionIdException on PutResourcePolicy. Recorded behavior: a PutResourcePolicy carrying a PolicyRevisionId that is not the currently stored revision is refused with InvalidPolicyRevisionIdException whose message is the fixed text "If policyRevisionID is provided, then it must match the existing revision." — no variable parts — and the stored policy/revision is left untouched; retrying with the current revision succeeds and mints a new revision id.
Emulator: the code is right but the message is "The policy revision ID 1 does not match the latest policy revision ID." — different fixed text, and it interpolates the submitted revision id, which AWS's message does not contain.
Why: callers implementing optimistic locking match this message (the code alone does not say which side was stale) and log it to operators; the emulator's wording also exposes the emulator's sequential revision numbering.
Fix: the revision-mismatch branch of the PutResourcePolicy handler.
Traces: rum.put-resource-policy-concurrent-revision-conflict, rum.put-resource-policy-stale-revision-conflict.

Cause 8 of 9: DeleteResourcePolicy's stale-revision message is wrong (and differs from PutResourcePolicy's in AWS).
AWS: Smithy models InvalidPolicyRevisionIdException on DeleteResourcePolicy. Recorded behavior: DeleteResourcePolicy with a stale PolicyRevisionId is refused with InvalidPolicyRevisionIdException whose message is the fixed text "PolicyRevisionID must match the existing revision" (no trailing period, no variable parts — deliberately NOT the same sentence PutResourcePolicy uses, see Cause 7), the stored policy and revision are unchanged, and deleting with the correct revision then succeeds (after which GetResourcePolicy answers PolicyNotFoundException "Resource policy does not exist").
Emulator: the same generic sentence as Cause 7 — "The policy revision ID 1 does not match the latest policy revision ID." — i.e. one shared message for both operations, matching neither.
Why: same as Cause 7; additionally a client that keys off the message to tell which operation's optimistic-lock check failed cannot.
Fix: the revision-mismatch branch of the DeleteResourcePolicy handler must use its own AWS wording rather than a shared helper with PutResourcePolicy.
Traces: rum.resource-policy-revision-mismatch-rejected, rum.stale-policy-revision-delete-rejected.

Cause 9 of 9: TagResource's BadRequestException reports a framework constraint sentence instead of RUM's tag messages.
AWS: Smithy: TagKey has @length 1..128 and @pattern "^(?!aws:)[a-zA-Z+-=._:/]+$"; recorded behavior is that TagResource refuses a bad key with BadRequestException (the code the emulator already returns) and one of two human-worded messages, each entirely fixed text with no variable parts: for a key starting with the reserved prefix, "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)"; for a key containing a character outside the allowed charset, "Tags can only contain letters, numbers, spaces, and the following special characters: _ . : / = + - @". The rejected tag is not persisted (ListTagsForResource does not show it) and a well-formed key on the same resource still succeeds.
Emulator: both cases produce the generic framework sentence instead, e.g. "1 validation error detected: Value 'aws:reserved' at 'tags.aws:reserved.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!aws:)[a-zA-Z+-=._:/]+$" (and the same for 'bad!key'). Related but NOT the same as open issue 20261029T085313Z-rum-createappmonitor-error-response--r2.md, which is about the internal consistency of CreateAppMonitor's ValidationException tag-key clause (anchors and the extra `.member` path step); this cause is that TagResource must not use a constraint sentence at all — AWS answers BadRequestException with the two fixed messages above.
Why: clients that surface or branch on the tag-rejection reason get a message naming an internal member path and a regex instead of the documented reason, and the two distinct AWS reasons (reserved prefix vs charset) become indistinguishable.
Fix: the tagging frontend's tag-key refusal for TagResource — select between the two RUM messages (reserved aws: prefix, case-insensitive, vs charset violation) instead of emitting the framework constraint clause.
Traces: rum.tag-resource-invalid-key-format-rejected, rum.tag-resource-invalid-key-rejected.

## Evidence
Suite: 51/81 passed; all 30 failures are in service `rum` (bootstrapped as service-com-amazonaws-rum-rum). None of these 18 checks was marked ALREADY FILED or OWNED by.

Cause 1 — CFN deploys:
  rum.deploy-app-monitor-cfn-behaves: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetAppMonitor operation: Resource not found sim2real-rum-cfn-3fd0ad284a1b (the stack had reached CREATE_COMPLETE and the !GetAtt AppMonitor.Id output was read successfully).
  rum.deploy-app-monitor-cfn-stack-behaves: ... GetAppMonitor ...: Resource not found sim2real-behav-rum-cfnapp-5e32f61eb3d4
  rum.deploy-app-monitor-via-cloudformation-then-behaves: ... GetAppMonitor ...: Resource not found sim2real-rum-mon-1b3b2daffd6d
  rum.deploy-app-monitor-cfn-tags-propagate: botocore.exceptions.ClientError: An error occurred (NotFoundException) when calling the ListTagsForResource operation: Resource was not found

Cause 2 — data plane:
  rum.put-rum-events-payload-timestamp-preserved: AssertionError: RUM event b31fa0ee-ef93-42a9-8d91-e43637090b24 never surfaced via GetAppMonitorData within 30s (PutRumEvents had returned success; the poll used TimeRange in epoch ms spanning the submission and followed NextToken).
  rum.e2e-app-monitor-lifecycle-with-metrics-and-events: Check timed out after 60.0s — the check was inside the same GetAppMonitorData poll for the event it had just submitted (its control-plane assertions up to that point passed).

Cause 3 — denial clause:
  rum.put-rum-metrics-destination-access-denied:
    expected '... is not authorized to perform: rum:PutRumMetricsDestination on resource: arn:aws:rum:us-west-2:000000000000:appmonitor/sim2real-behav-appmon-f1cc578a12c5 with an explicit deny in an identity-based policy'
    actual   '... with an explicit deny in a policy for the rum:PutRumMetricsDestination action'
  rum.put-rum-events-missing-permission-denied (resource-policy Deny): actual '... with an explicit deny in a policy for the rum:PutRumEvents action'; expected '... with an explicit deny in a resource-based policy'.
  rum.e2e-resource-policy-access-lifecycle (no policy at all): actual '... because no policy allows the rum:PutRumEvents action'; expected '... because no identity-based policy allows the rum:PutRumEvents action'.

Cause 4 — resource slot on PutRumEvents:
  rum.put-rum-events-missing-permission-denied: actual 'on resource: arn:aws:rum:us-west-2:000000000000:appmonitor/sim2real-rum-events-a602c1ad29f8'; expected 'on resource: <app monitor Id>' (the Id the request carried).
  rum.e2e-resource-policy-access-lifecycle: same divergence (expected 'on resource: <app_monitor_id>').

Cause 5 — rum.get-resource-policy-unknown-app-monitor-denied: AssertionError: expected ValidationException, got ResourceNotFoundException for Name='.sim2real-behav-rum-missing-<suffix>' (leading dot). The well-formed-but-missing leg (ResourceNotFoundException, 'Resource not found <name>') passed, so the two outcomes are currently identical.

Cause 6 — malformed policy document:
  rum.put-resource-policy-malformed-document-rejected: AssertionError: expected a null message (assert message is None), got 'The policy document is not valid JSON: expected ident at line 1 column 43.'
  rum.resource-policy-malformed-document-rejected: expected a null message, got 'The policy document is not valid JSON: trailing comma at line 1 column 233.'

Cause 7 — PutResourcePolicy revision:
  rum.put-resource-policy-stale-revision-conflict and rum.put-resource-policy-concurrent-revision-conflict: assert message == "If policyRevisionID is provided, then it must match the existing revision." -> actual 'The policy revision ID 1 does not match the latest policy revision ID.' (code InvalidPolicyRevisionIdException matched).

Cause 8 — DeleteResourcePolicy revision:
  rum.resource-policy-revision-mismatch-rejected and rum.stale-policy-revision-delete-rejected: expected message 'PolicyRevisionID must match the existing revision', got 'The policy revision ID 1 does not match the latest policy revision ID.' (code InvalidPolicyRevisionIdException matched).

Cause 9 — TagResource:
  rum.tag-resource-invalid-key-rejected: expected 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)', got "1 validation error detected: Value 'aws:reserved' at 'tags.aws:reserved.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!aws:)[a-zA-Z+-=._:/]+$" (code BadRequestException matched).
  rum.tag-resource-invalid-key-format-rejected: expected 'Tags can only contain letters, numbers, spaces, and the following special characters: _ . : / = + - @', got the same framework sentence for Value 'bad!key' at 'tags.bad!key.member'.

## Steps to Reproduce
Replay the 18 checks listed in trace_ids. Minimal manual repro per cause:

Cause 1: CreateStack with a template containing `AppMonitor: {Type: AWS::RUM::AppMonitor, Properties: {Name: m, Domain: localhost, CwLogEnabled: false, AppMonitorConfiguration: {AllowCookies: true, SessionSampleRate: 1}}}` and Output !GetAtt AppMonitor.Id; wait for CREATE_COMPLETE; then GetAppMonitor(Name=m) -> observed ResourceNotFoundException 'Resource not found m'; expected the monitor with Id equal to the stack output, Domain localhost, AllowCookies true, SessionSampleRate 1.0. Also ListTagsForResource(arn:aws:rum:<region>:<account>:appmonitor/m) -> observed NotFoundException; expected the stack's propagated tags.

Cause 2: CreateAppMonitor(Name=m, DomainList=[example.com]); PutRumEvents(Id=<monitor Id>, BatchId=<uuid>, AppMonitorDetails={...}, UserDetails={sessionId,userId}, RumEvents=[{id=<uuid>, timestamp=<now with microsecond=123456>, type=com.amazon.rum.session_start_event, metadata=..., details=...}]); then GetAppMonitorData(Name=m, TimeRange={After: now_ms-3600000, Before: now_ms+60000}, MaxResults=100), following NextToken -> observed no event ever; expected the event JSON with event_id equal to the submitted id and event_timestamp in epoch ms matching the sent timestamp to sub-second precision.

Cause 3/4: (a) assume a role whose identity policy explicitly denies rum:PutRumMetricsDestination and call PutRumMetricsDestination -> compare the trailing clause with ' with an explicit deny in an identity-based policy'. (b) PutResourcePolicy on the monitor with an explicit Deny of rum:PutRumEvents for Principal '*', then PutRumEvents as the normal caller -> expected '... on resource: <app monitor Id> with an explicit deny in a resource-based policy'; observed the ARN plus ' with an explicit deny in a policy for the rum:PutRumEvents action'. (c) assume a role with NO identity policy and call PutRumEvents with no resource policy attached -> expected '... on resource: <app monitor Id> because no identity-based policy allows the rum:PutRumEvents action'.

Cause 5: GetResourcePolicy(Name='.not-a-real-monitor') -> observed ResourceNotFoundException; expected 400 ValidationException "1 validation error detected: Value '.not-a-real-monitor' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!\\.)[\\.\\-_#A-Za-z0-9]+". Contrast with GetResourcePolicy(Name='not-a-real-monitor') which must stay ResourceNotFoundException 'Resource not found not-a-real-monitor'.

Cause 6: PutResourcePolicy(Name=m, PolicyDocument='{"Version": "2012-10-17", "Statement": [ this is not valid json') -> observed MalformedPolicyDocumentException WITH a parser message; expected MalformedPolicyDocumentException with no Message member. Then GetResourcePolicy(Name=m) must be PolicyNotFoundException 'Resource policy does not exist'.

Cause 7: PutResourcePolicy(Name=m, PolicyDocument=doc1) -> rev1; PutResourcePolicy(Name=m, PolicyDocument=doc2, PolicyRevisionId=rev1) -> rev2; PutResourcePolicy(Name=m, PolicyDocument=doc3, PolicyRevisionId=rev1) -> expected InvalidPolicyRevisionIdException 'If policyRevisionID is provided, then it must match the existing revision.'; observed 'The policy revision ID 1 does not match the latest policy revision ID.'

Cause 8: with rev1/rev2 as above, DeleteResourcePolicy(Name=m, PolicyRevisionId=rev1) -> expected InvalidPolicyRevisionIdException 'PolicyRevisionID must match the existing revision'; observed the same generic sentence as Cause 7.

Cause 9: TagResource(ResourceArn=<monitor arn>, Tags={'aws:reserved': 'value'}) -> expected BadRequestException 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'. TagResource(ResourceArn=<monitor arn>, Tags={'bad!key': 'value'}) -> expected BadRequestException 'Tags can only contain letters, numbers, spaces, and the following special characters: _ . : / = + - @'. Both observed as the framework's '1 validation error detected: ... tags.<key>.member ...' sentence. A valid key must still be accepted and visible in ListTagsForResource.
