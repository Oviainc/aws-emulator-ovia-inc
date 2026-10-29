Issue-Source: trace-sweep-divergence
Issue-Trace: socialmessaging-associate-whatsapp-business-account-invalid-token
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: socialmessaging-associate-whatsapp-business-account-invalid-token socialmessaging-dataset-conversion-and-send-message-not-found socialmessaging-waba-and-phone-details-not-found socialmessaging-list-linked-whatsapp-business-accounts-empty
Issue-Shape-Id: com.amazonaws.socialmessaging#SocialMessaging
Issue-Key: social-messaging|AssociateWhatsAppBusinessAccount|4xx->2xx|$.Message
Issue-Key: social-messaging|AssociateWhatsAppBusinessAccount|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: social-messaging|AssociateWhatsAppBusinessAccount|4xx->2xx|status
Issue-Key: social-messaging|CreateWhatsAppDataset|4xx->4xx|$.message
Issue-Key: social-messaging|DisassociateWhatsAppBusinessAccount|4xx->4xx|$.message
Issue-Key: social-messaging|ListLinkedWhatsAppBusinessAccounts|2xx->2xx|$.setupEnabled
Issue-Commit: ff63bbd76f191efb9689374a4f98215607f4bd70

# social-messaging WhatsApp wire responses diverge from recording in 4 places: console CSRF denial, generic not-found wording, missing setupEnabled

> _Note: this issue was identified against the code base at commit `ff63bbd76f191efb9689374a4f98215607f4bd70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWhatsAppBusinessAccount

## Description
Four net-new root causes in the social-messaging (SocialMessaging / WhatsApp) surface, all found by replaying simparity-wire-aws-v1. They share one fault family: the response this emulator composes for an operation does not carry the status, error wording or body members the real service composes for the same request. FOUR separate causes must be closed before this issue is done.

Cause 1 of 4 — AssociateWhatsAppBusinessAccount accepts a signup request the service denies (403 -> 200).
AWS: the operation is documented "This is only used through the Amazon Web Services console during sign-up to associate your WhatsApp Business Account to your Amazon Web Services account" (Smithy: AssociateWhatsAppBusinessAccount, POST /v1/whatsapp/signup; modeled errors DependencyException 502, InvalidParametersException 400, LimitExceededException 400, ThrottledRequestException 429, plus service-common AccessDeniedException 403). A signup callback presented outside that console flow — i.e. without the console's CSRF header — is refused, and the recorded refusal is HTTP 403 carrying the Coral access-denied envelope: response header `x-amzn-errortype: AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/` and body member `Message` (capital M, the Coral framing, NOT the model's lowercase `message` on AccessDeniedException) with the fixed text `CSRF header does not exist`. That message has no variable parts: no name, id or value from the request appears in it, so it is reproduced verbatim for this error class, and the errortype header value is likewise fixed text.
Emulator: answers HTTP 200 with a success body (a minted signupCallbackResult.associateInProgressToken and statusCode), no `Message` member and no `x-amzn-errortype` header. The handler's gate chain in services/service-com-amazonaws-socialmessaging-socialmessaging/src/ops/associate_whats_app_business_account.rs checks only member presence, token validity and the store's capacity limits; nothing in it refuses a request that did not arrive through the console.
Why: a client (or a test of least-privilege behavior) that expects this console-only door to be shut sees the signup accepted. Worse, the accepted call MUTATES state the real service never would: a pending signup is recorded and an associateInProgressToken minted, so a subsequent setupFinalization succeeds against the emulator and 403s against AWS. SDK error handling keyed on AccessDeniedException/x-amzn-errortype never fires.
Fix: AssociateWhatsAppBusinessAccount's request-admission gate (the console/CSRF precondition, ahead of every store mutation so a refused call leaves the store untouched), plus the error serializer path that must emit this refusal with the Coral-shaped `Message` body member and the `x-amzn-errortype` header rather than the model's lowercase shape.

Cause 2 of 4 — CreateWhatsAppDataset's not-found refusal uses a generic message instead of the service's wording.
AWS: CreateWhatsAppDataset (POST /v1/whatsapp/waba/dataset, required `id`: LinkedWhatsAppBusinessAccountId, len 1..115, pattern (^waba-.*$)|(^arn:.*:waba/[0-9a-zA-Z]+$)) refuses an `id` that names no WABA linked to the calling account with ResourceNotFoundException (Smithy: @error client, httpError 404, member `message`). Recorded behavior for the message: the fixed text `Id not found for account.` — it is wholly fixed, naming neither the requested id nor the account.
Emulator: correct status (404) and error shape, but `message` is `Requested resource not found.`
Why: the status matches, so a client that branches on the exception type is fine, but anything that asserts on or logs the message text — error-message assertions in tests, support tooling, operator triage — sees wording AWS never emits, and the two distinct AWS wordings (see Cause 3) collapse into one, so a caller cannot tell which resource lookup failed.
Fix: CreateWhatsAppDataset's not-found refusal must be worded for this operation instead of reusing crate::state::RESOURCE_NOT_FOUND_MESSAGE, the single generic constant every ResourceNotFoundException in this crate currently interpolates. The per-operation wordings belong beside that constant in state.rs (so the crate keeps single-homing its client-error wording) with the handlers selecting the right one.

Cause 3 of 4 — DisassociateWhatsAppBusinessAccount's not-found refusal uses the same generic message.
AWS: DisassociateWhatsAppBusinessAccount (DELETE /v1/whatsapp/waba/disassociate, required httpQuery `id`) refuses an id that names no linked WABA with ResourceNotFoundException (404). Recorded behavior for the message: the fixed text `Resource requested for disassociation does not exist.` — again wholly fixed text, with no id or account interpolated. It is DIFFERENT text from the CreateWhatsAppDataset refusal in Cause 2, which is the point: AWS words its 404s per operation.
Emulator: 404 with `message` = `Requested resource not found.`
Why: same as Cause 2 — the emulator's one-size message is not the text AWS emits, and it erases the distinction AWS draws between "the id is not one of this account's" and "the resource you asked to disassociate does not exist". Note this is a SEPARATE cause from Cause 2 even though both read the same constant: closing one wording does not close the other, and both recorded steps must be re-replayed.
Fix: same component as Cause 2 — the disassociation not-found wording, selected per operation instead of the shared crate::state::RESOURCE_NOT_FOUND_MESSAGE.

Cause 4 of 4 — ListLinkedWhatsAppBusinessAccounts omits the top-level setupEnabled flag.
AWS: recorded behavior only — the Smithy model's ListLinkedWhatsAppBusinessAccountsOutput declares just `linkedAccounts` and `nextToken` (GET /v1/whatsapp/waba/list), but the recorded 200 response carries an ADDITIONAL top-level boolean member `setupEnabled`, true in the recording, reported even when the account has no linked WABAs at all (the recorded listing is empty). It reports whether WhatsApp setup is enabled for the calling account, so it is a property of the account's state rather than a per-run value.
Emulator: 200 with the right linkedAccounts/nextToken, but no `setupEnabled` member at all — the generated output type has no such field, so the serializer cannot emit one.
Why: a client that reads response.setupEnabled to decide whether to drive the sign-up flow gets null/absent from the emulator where AWS gives a boolean, so the branch keyed on it is never exercised; an SDK/consumer that treats the member as always present mis-reads a successful list call.
Fix: the ListLinkedWhatsAppBusinessAccounts output projection — the flag has to be carried in the service state (an account-level "setup enabled" fact) and emitted by this listing's serializer as a top-level member alongside linkedAccounts, since the modeled output shape does not declare it.

No existing open issue covers any of these: a search of the open issues for "social-messaging", "socialmessaging" and "whatsapp" returns nothing. The 26 steps this corpus could not exercise because the operation is an unimplemented scaffold are NOT part of this issue; they are blocked-by-stub and already owned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (14 fixtures, 44 steps, 13 passed, score 29.5%, emulator up and serving: /__admin/health 200). Four divergence clusters, all service=social-messaging, none blocked by a stub and none owned by an open issue.

Cause 1 — fixture socialmessaging-associate-whatsapp-business-account-invalid-token, step task#1, AssociateWhatsAppBusinessAccount, verdict mismatch:
  status: expected 403, got 200
  $.Message: missing (expected 'CSRF header does not exist', got None)
  $headers.x-amzn-errortype: missing (expected 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', got None)
  Expected: 403 + Coral AccessDeniedException envelope. Actual: 200 + success body (signup recorded, token minted).

Cause 2 — fixture socialmessaging-dataset-conversion-and-send-message-not-found, step task#1, CreateWhatsAppDataset, verdict mismatch:
  $.message: value_mismatch (expected 'Id not found for account.', got 'Requested resource not found.')
  Expected 404 / got 404 — status and error class agree, only the message text diverges.

Cause 3 — fixture socialmessaging-waba-and-phone-details-not-found, step task#3, DisassociateWhatsAppBusinessAccount, verdict mismatch:
  $.message: value_mismatch (expected 'Resource requested for disassociation does not exist.', got 'Requested resource not found.')
  Expected 404 / got 404.

Cause 4 — fixture socialmessaging-list-linked-whatsapp-business-accounts-empty, steps task#1 and task#2 (2 affected steps), ListLinkedWhatsAppBusinessAccounts, verdict mismatch:
  $.setupEnabled: missing (expected True, got None)
  Expected 200 / got 200 — body member missing.

Source evidence in the checkout: services/service-com-amazonaws-socialmessaging-socialmessaging/src/ops/associate_whats_app_business_account.rs (handle, lines ~186-390: no console/CSRF admission gate; store mutated and token minted on the path the recording refuses); .../src/ops/create_whats_app_dataset.rs lines 159-166 and .../src/ops/disassociate_whats_app_business_account.rs lines 153-176 (both interpolate crate::state::RESOURCE_NOT_FOUND_MESSAGE); .../src/ops/list_linked_whats_app_business_accounts.rs lines 191-194 (output built from linked_accounts + next_token only). Smithy model: services/service-com-amazonaws-socialmessaging-socialmessaging/smithy-model/com-amazonaws-socialmessaging-socialmessaging.json — AccessDeniedException httpError 403, ResourceNotFoundException httpError 404, ListLinkedWhatsAppBusinessAccountsOutput declares no setupEnabled.

## Steps to Reproduce
Replay the social-messaging fixtures of trace set simparity-wire-aws-v1 against the emulator built from this checkout (replay_wire_corpus), then re-replay the four recorded units named in Issue-Trace/Issue-Step and trace_ids:

1. Cause 1: socialmessaging-associate-whatsapp-business-account-invalid-token#task#1 — POST /v1/whatsapp/signup with a signupCallback presented outside the console sign-up flow (no console CSRF header). Expect 403, header x-amzn-errortype: AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/, body {"Message":"CSRF header does not exist"}; the emulator answers 200 with a minted associateInProgressToken. Also confirm the refused call records NO pending signup.
2. Cause 2: socialmessaging-dataset-conversion-and-send-message-not-found#task#1 — POST /v1/whatsapp/waba/dataset with an `id` naming no linked WABA. Expect 404 ResourceNotFoundException with message 'Id not found for account.'; the emulator says 'Requested resource not found.'
3. Cause 3: socialmessaging-waba-and-phone-details-not-found#task#3 — DELETE /v1/whatsapp/waba/disassociate?id=<unlinked waba id>. Expect 404 ResourceNotFoundException with message 'Resource requested for disassociation does not exist.'; the emulator says 'Requested resource not found.'
4. Cause 4: socialmessaging-list-linked-whatsapp-business-accounts-empty#task#1 (and task#2) — GET /v1/whatsapp/waba/list on an account with no linked WABAs. Expect 200 whose body carries top-level setupEnabled: true alongside linkedAccounts; the emulator omits setupEnabled entirely.

All four steps must pass before this issue is closed.
