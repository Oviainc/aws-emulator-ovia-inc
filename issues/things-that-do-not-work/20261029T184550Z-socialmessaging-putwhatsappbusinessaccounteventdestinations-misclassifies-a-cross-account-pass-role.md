Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: socialmessaging.event-destinations-exceeds-max-rejected
Issue-Shape-Id: com.amazonaws.socialmessaging#SocialMessaging
Issue-Key: socialmessaging.event-destinations-exceeds-max-rejected|run#12|AssertionError|assert code2 == "AccessDeniedException", f"expected AccessDeniedException, got {code2}"
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# socialmessaging: PutWhatsAppBusinessAccountEventDestinations misclassifies a cross-account pass-role refusal as 400 InvalidParametersException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutWhatsAppBusinessAccountEventDestinations

## Description
One net-new root cause in the socialmessaging surface, grounded in the behavioral suite simparity-behavioral-aws-v1. ONE cause must be closed.

Cause 1 of 1 — the pass-role denial is never evaluated, so the refusal carries the wrong error class, status and message.
AWS: Smithy: PutWhatsAppBusinessAccountEventDestinations, PUT /v1/whatsapp/waba/eventdestinations, @idempotent; input `id` (LinkedWhatsAppBusinessAccountId, len 1..115, pattern (^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$), @required) and `eventDestinations` (list<WhatsAppBusinessAccountEventDestination>, @length max 1, @required, each entry carrying eventDestinationArn + roleArn); operation errors InternalServiceException 500, InvalidParametersException 400, ThrottledRequestException 429, plus the service-common AccessDeniedException 403, ValidationException 400, InvalidParametersException 400, ThrottledRequestException 429. Recorded behavior: with exactly ONE syntactically valid destination whose roleArn names an account other than the caller's, AWS refuses the call 403 AccessDeniedException with the fixed message 'Cross-account pass role is not allowed.' — the text is wholly fixed, with neither the roleArn, the destination ARN nor the WABA id interpolated — and it does so even though the `id` in the same request names no WABA linked to the account, i.e. the pass-role check is evaluated ahead of (or independently of) the linked-account lookup. With TWO destinations the request is refused earlier, 400 ValidationException '1 validation error detected: Value \'[<the rendered destination structs>]\' at \'eventDestinations\' failed to satisfy constraint: Member must have length less than or equal to 1' (fixed text; the rendered value list and member path are the request-derived slots) — that half already passes.
Emulator: answers 400 InvalidParametersException with the emulator-authored message 'The id provided is not a linked WhatsApp Business Account.' The handler looks the WABA id up first and has no cross-account roleArn check at all, so the IAM pass-role refusal AWS states can never be produced; the handler's own comment records that it deliberately reports an unknown id as InvalidParametersException because the model lists no not-found for this operation.
Why: a client or least-privilege test that branches on AccessDeniedException / HTTP 403 for a pass-role failure instead sees a retryable-looking 400 about a different member, and the operator-facing text blames the WABA id for an IAM problem. Error-handling and remediation keyed on the access-denied class never fire, and the ordering contract (authorization of the passed role before resource resolution) is inverted.
Fix: the PutWhatsAppBusinessAccountEventDestinations handler's admission order and refusal classification — compare each destination's roleArn account id against the calling account and refuse with AccessDeniedException (403) carrying AWS's fixed text, evaluated ahead of the linked-account lookup and ahead of any state mutation. (The one open socialmessaging issue, 20261029T171221Z-social-messaging-whatsapp-wire-responses-diverge-from-recording.md, does not cover this operation.)
Traces: socialmessaging.event-destinations-exceeds-max-rejected

## Evidence
Behavioral suite simparity-behavioral-aws-v1: socialmessaging.event-destinations-exceeds-max-rejected [mismatch], not marked ALREADY FILED and not OWNED by any open issue.

Assertion from the replay:
  File "check_socialmessaging_event_destinations_exceeds_max_rejected.py", line 55, in run
    assert code2 == "AccessDeniedException", f"expected AccessDeniedException, got {code2}"
  AssertionError: expected AccessDeniedException, got InvalidParametersException

Request that diverges: PutWhatsAppBusinessAccountEventDestinations(id='waba-00000000000000000000000000000000', eventDestinations=[{eventDestinationArn: 'arn:aws:sns:us-east-1:111122223333:sim2real-topic-1', roleArn: 'arn:aws:iam::111122223333:role/<a role in another account>'}]).
Expected: HTTP 403, code AccessDeniedException, message 'Cross-account pass role is not allowed.' (fixed text, no request values).
Actual: HTTP 400, code InvalidParametersException, message 'The id provided is not a linked WhatsApp Business Account.' (emulator-authored).
The two-destination half of the same check passes, so the @length max 1 boundary and its ValidationException wording are already correct.

Source evidence: services/service-com-amazonaws-socialmessaging-socialmessaging/src/ops/put_whats_app_business_account_event_destinations.rs lines 145-177 — the handler resolves `store.linked_account_id(&input.id)` first and, on miss, returns InvalidParametersException with that constant message; no roleArn/account comparison exists anywhere in the handler. Model: .../smithy-model/com-amazonaws-socialmessaging-socialmessaging.json (service-common errors include AccessDeniedException; OPERATIONS.md lines 201-207 list the operation's own errors).

## Steps to Reproduce
Replay socialmessaging.event-designations… (exact id: socialmessaging.event-destinations-exceeds-max-rejected) from simparity-behavioral-aws-v1 against the emulator built from this checkout. By hand:

1. PUT /v1/whatsapp/waba/eventdestinations with id='waba-00000000000000000000000000000000' and ONE eventDestination whose roleArn belongs to an account other than the caller's (e.g. arn:aws:iam::111122223333:role/<role>) and whose eventDestinationArn is a valid SNS topic ARN. Expect HTTP 403, error code AccessDeniedException, message 'Cross-account pass role is not allowed.'; the emulator answers HTTP 400 InvalidParametersException 'The id provided is not a linked WhatsApp Business Account.'
2. Confirm the refused call mutates nothing (no event destination recorded on any account).
3. Confirm the pre-existing TWO-destination refusal still answers 400 ValidationException with the 'Member must have length less than or equal to 1' wording.

The check must pass before this issue is closed.
