Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connect-contact-lens.list-realtime-analysis-segments-unknown-contact-stays-not-found
Issue-Shape-Id: com.amazonaws.connectcontactlens#AmazonConnectContactLens
Issue-Key: connect-contact-lens.list-realtime-analysis-segments-unknown-contact-stays-not-found|run#12|AssertionError|assert error_response["Error"]["Message"] == "Real-time contact analysis not found.", (
Issue-Commit: 50b5738f20ef93c4d2a390fa7193c568f3ace32c

# ListRealtimeContactAnalysisSegments ResourceNotFoundException message is emulator-authored request echo, not the fixed text AWS returns

> _Note: this issue was identified against the code base at commit `50b5738f20ef93c4d2a390fa7193c568f3ace32c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRealtimeContactAnalysisSegments

## Description
Cause 1 of 1: the ResourceNotFoundException body for a contact that has no real-time analysis session carries an emulator-authored, request-derived message instead of AWS's fixed message text.

AWS: Smithy model services/service-com-amazonaws-connectcontactlens-amazonconnectcontactlens/smithy-model/com-amazonaws-connectcontactlens-amazonconnectcontactlens.json models ListRealtimeContactAnalysisSegments (POST /realtime-contact-analysis/analysis-segments) with error ResourceNotFoundException, errorKind client, httpError 404, a single member `Message` (target Message), doc "The specified resource was not found." The model does not state the message string, so this is recorded behavior only: for an InstanceId/ContactId pair with no real-time contact analysis session, Amazon Connect Contact Lens answers HTTP 404, Error.Code `ResourceNotFoundException`, and Error.Message exactly `Real-time contact analysis not found.` That message is ENTIRELY fixed text — it has no variable parts at all: no InstanceId, no ContactId, no quoted identifier, no count. Nothing from the request or from emulator state is interpolated into it. The body carries no `Segments` and no `NextToken` member (both are output-shape members and are absent from the error shape, whose only member is Message).

Emulator: the handler returns the right status (404) and the right code (ResourceNotFoundException), but composes its own message out of the request's identifiers, of the form `No real-time contact analysis session for ContactId '<ContactId>' in InstanceId '<InstanceId>'.` — observed in replay as `No real-time contact analysis session for ContactId 'b5f87632-c37e-4609-a9d6-6159703b1989' in InstanceId '4cd1aa0d-5ae8-497f-901e-e5a6892bcee8'.` (the two UUIDs there are per-run values from the check's own request, not part of any requirement; the DEFECT is the whole sentence template, which AWS never emits). The 404/ResourceNotFoundException classification, the absence of Segments/NextToken, and the stability of the refusal under repeated polling are all correct — only the message text diverges.

Why: a client that matches, asserts on, logs or surfaces the refusal message for an unknown contact gets prose AWS never produces. Code written and tested against the emulator — error-message assertions in integration tests, log scrapers, operator-facing messages, SDK-level message matching used to distinguish "analysis never existed" from other 404s — passes on the emulator and diverges against real Contact Lens. Worse, the emulator's variant echoes the caller's InstanceId and ContactId back in the refusal, so a client could come to rely on parsing identifiers out of a message that, on real AWS, contains none.

Fix: the ResourceNotFoundException construction in the ListRealtimeContactAnalysisSegments handler's session-lookup miss branch — the error should carry the fixed text `Real-time contact analysis not found.` with no interpolation of input.contact_id / input.instance_id. If any sibling path in this crate builds a not-found message the same request-echoing way, it belongs in one shared constructor rather than per-handler format! calls.

Traces: connect-contact-lens.list-realtime-analysis-segments-unknown-contact-stays-not-found (replayed check asserts the exact Error.Message for a freshly minted Connect instance plus a random, never-existed ContactId, polled repeatedly across a ~20s window; every poll returned the diverging message). Code citation: services/service-com-amazonaws-connectcontactlens-amazonconnectcontactlens/src/ops/list_realtime_contact_analysis_segments.rs lines 192-205.

## Evidence
Check: connect-contact-lens.list-realtime-analysis-segments-unknown-contact-stays-not-found (verdict: mismatch)

Request: ListRealtimeContactAnalysisSegments(InstanceId=<a CONNECT_MANAGED instance just created and waited to ACTIVE>, ContactId=<a fresh uuid4 that was never a contact>)

Expected (AWS):
  HTTP 404
  Error.Code    = "ResourceNotFoundException"
  Error.Message = "Real-time contact analysis not found."   <- 100% fixed text, no variable parts
  no "Segments" member, no "NextToken" member in the error response

Actual (emulator):
  HTTP 404                                   (correct)
  Error.Code    = "ResourceNotFoundException" (correct)
  Error.Message = "No real-time contact analysis session for ContactId 'b5f87632-c37e-4609-a9d6-6159703b1989' in InstanceId '4cd1aa0d-5ae8-497f-901e-e5a6892bcee8'."   <- WRONG
  no Segments / NextToken                     (correct)

Message-template breakdown of the emulator's string: the fixed part is "No real-time contact analysis session for ContactId '" + "' in InstanceId '" + "'."; the variable parts are the request's ContactId and InstanceId (the two UUIDs above are this run's values only). AWS's message has NO variable part, so no substitution makes the emulator's template equal AWS's.

Harness assertion that failed:
  assert error_response["Error"]["Message"] == "Real-time contact analysis not found."
  AssertionError: expected the exact ResourceNotFoundException message AWS returns, got "No real-time contact analysis session for ContactId '...' in InstanceId '...'."

Emulator source producing it — services/service-com-amazonaws-connectcontactlens-amazonconnectcontactlens/src/ops/list_realtime_contact_analysis_segments.rs:192-205:
  let Some(session) = state.session(&key) else {
      return Err(... ResourceNotFoundException { message: Some(format!(
          "No real-time contact analysis session for ContactId '{}' in InstanceId '{}'.",
          input.contact_id, input.instance_id)) } ) ...)

Smithy (same crate's model): ResourceNotFoundException -> errorKind client, httpError 404, members {Message}, doc "The specified resource was not found."; ListRealtimeContactAnalysisSegments errors = [AccessDeniedException 403, InternalServiceException 500, InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429].

The check also verified the refusal is STABLE (>=3 polls over ~20s, never flipping to a 200/in-progress session) and that no NextToken/Segments leaked — those held. The message is the sole divergence.

## Steps to Reproduce
1. Clone the repo and boot the emulator (the connect and connect-contact-lens services both answer).
2. connect.create_instance(IdentityManagementType="CONNECT_MANAGED", InstanceAlias=<unique>, InboundCallsEnabled=True, OutboundCallsEnabled=True) and poll connect.describe_instance until Instance.InstanceStatus == "ACTIVE"; keep the returned Id as <instance_id>.
3. Generate a random uuid4 as <fake_contact>; do not create any contact for it.
4. Call connect-contact-lens ListRealtimeContactAnalysisSegments(InstanceId=<instance_id>, ContactId=<fake_contact>) and catch the ClientError.
5. Inspect err.response: Error.Code is "ResourceNotFoundException" and the HTTP status is 404 (both correct), but Error.Message is "No real-time contact analysis session for ContactId '<fake_contact>' in InstanceId '<instance_id>'." instead of AWS's fixed "Real-time contact analysis not found."
6. Repeat step 4 a few times, ~2s apart, over a 20s window: the same wrong message is returned every time (the divergence is deterministic, not a race).
7. Equivalent: run_behavioral_suite and read check connect-contact-lens.list-realtime-analysis-segments-unknown-contact-stays-not-found.
