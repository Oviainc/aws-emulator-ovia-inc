Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.email-identity-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.email-identity-lifecycle|run#23|AssertionError|assert err.get('Code') == 'BadRequestException', (
Issue-Commit: 4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666

# sesv2: PutEmailIdentityFeedbackAttributes refuses an identity that does not exist with NotFoundException/404 and the record-absent message instead of BadRequestException/400 "Identity ... is invalid. Must be a verified email address or domain."

> _Note: this issue was identified against the code base at commit `4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEmailIdentityFeedbackAttributes

## Description
One net-new root cause in the sesv2 identity-feedback surface.

Cause 1 of 1: PutEmailIdentityFeedbackAttributes applied to an email identity that is not a usable sending identity (here: one that was created and then deleted) is refused with the WRONG error class, status and message. The emulator answers the generic record-absent not-found refusal; SES answers the invalid-identity refusal. Failing check: sesv2.email-identity-lifecycle.

AWS: Smithy (com.amazonaws.sesv2): PutEmailIdentityFeedbackAttributes, PUT /v2/email/identities/{EmailIdentity}/feedback, input {EmailIdentity (httpLabel, required, @length min 1), EmailForwardingEnabled (Enabled, default false)}, output empty, success 200; modeled errors BadRequestException (HTTP 400, "The input you provided is invalid."), NotFoundException (HTTP 404), TooManyRequestsException (HTTP 429). For the not-a-valid-sending-identity class the model alone does not pick the variant, so: recorded behavior only — the call is refused with BadRequestException and HTTP 400, and the message is `Identity {EmailIdentity} is invalid. Must be a verified email address or domain.` Fixed text of that message: `Identity ` + ` is invalid. Must be a verified email address or domain.`; the ONLY variable part is the identity, taken verbatim from the request's EmailIdentity path label (no angle brackets). The same service words its other missing-identity messages differently and the emulator already matches those — DeleteEmailIdentity: `Email identity {identity} does not exist.`; GetEmailIdentity: `Email identity <{identity}> does not exist.` — both of those assertions passed earlier in this same check, so the divergence is specific to the feedback-attributes refusal class, not to not-found messages in general.

Emulator: the call is refused, but as `{'Code': 'NotFoundException', 'HTTPStatusCode': 404, 'Message': 'Email identity {identity} does not exist.'}`. All three parts diverge: code NotFoundException instead of BadRequestException, status 404 instead of 400, and the delete/get record-absent wording instead of the invalid-identity wording. Source of the behavior: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_email_identity_feedback_attributes.rs lines 162-174 — a miss in store.identities returns PutEmailIdentityFeedbackAttributesErrorModeled::NotFoundException with crate::state::email_identity_not_found_unquoted(...), the shared not-found text.

Why: SES deliberately distinguishes "this is not a valid/verified sending identity" (BadRequestException, 400) from "this identity record is absent" (NotFoundException, 404). A client that branches on the error code or the HTTP status — verification/retry flows that treat 404 as "recreate the identity" and 400 as "the input is wrong and retrying is pointless", SDK retry/exception-type handlers, and tooling that surfaces or asserts the documented message — mis-classifies this refusal. Anything matching the documented text also fails, since the message carries the record-absent wording.

Fix: the PutEmailIdentityFeedbackAttributes handler's identity validation / error construction in the sesv2 service — the missing-or-unverified-identity branch must raise BadRequestException (400) with `Identity {EmailIdentity} is invalid. Must be a verified email address or domain.` (the request's EmailIdentity interpolated) rather than delegating to the shared email-identity-not-found text and NotFoundException. Its sibling PutEmailIdentityMailFromAttributes is recorded identically and must word and classify it the same way.

Related: open issue 20261029T085623Z-sesv2-two-causes-the-dns-baseline-hosted-zone.md reports only the MESSAGE wording for this operation (its replay key is this check's message assertion) and explicitly states the code and status were accepted; it does not cover the error-class/status divergence that now refuses the call, which is what this check fails on.

Traces: sesv2.email-identity-lifecycle (check line 71: the assertion on err['Code'] after create -> get -> delete -> delete(NotFound) -> get(NotFound)).

## Evidence
Expected (AWS, recorded): sesv2.put_email_identity_feedback_attributes(EmailIdentity=<an identity that does not exist>, EmailForwardingEnabled=True) ->
  Error.Code      = "BadRequestException"
  HTTPStatusCode  = 400
  Error.Message   = "Identity <the EmailIdentity from the request> is invalid. Must be a verified email address or domain."
    fixed text: "Identity " ... " is invalid. Must be a verified email address or domain."
    variable part: the EmailIdentity path label, copied from the request.

Actual (emulator):
  Error.Code      = "NotFoundException"
  HTTPStatusCode  = 404
  Error.Message   = "Email identity <the EmailIdentity from the request> does not exist."

Replay line, [mismatch] sesv2.email-identity-lifecycle, check_sesv2_email_identity_lifecycle.py line 71:
  AssertionError: expected PutEmailIdentityFeedbackAttributes on the deleted identity to be refused as BadRequestException, got {'Message': 'Email identity sim2real-behav-identity-da4e3029d709@example.com does not exist.', 'Code': 'NotFoundException', 'HTTPStatusCode': 404}
(the local part sim2real-behav-identity-da4e3029d709 is a per-run generated name filling the template, not part of the requirement.)

The same check's earlier assertions PASSED, which bounds the cause: DeleteEmailIdentity on the already-deleted identity returned NotFoundException/4xx with "Email identity {id} does not exist." and GetEmailIdentity returned NotFoundException/4xx with "Email identity <{id}> does not exist.", both matching AWS. Only the feedback-attributes refusal class diverges.

Emulator source: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_email_identity_feedback_attributes.rs:162-174 returns NotFoundException with crate::state::email_identity_not_found_unquoted(&input.email_identity).

## Steps to Reproduce
1. Build and boot the emulator; point a boto3 sesv2 client at it.
2. sesv2.create_email_identity(EmailIdentity='<unique-local-part>@example.com').
3. sesv2.delete_email_identity(EmailIdentity=<same address>) so the identity no longer exists (equivalently, use any address that was never created).
4. sesv2.put_email_identity_feedback_attributes(EmailIdentity=<same address>, EmailForwardingEnabled=True).
5. Inspect the raised ClientError: observe Error.Code 'NotFoundException', ResponseMetadata.HTTPStatusCode 404 and Error.Message 'Email identity {address} does not exist.', where AWS answers Error.Code 'BadRequestException', HTTP 400 and 'Identity {address} is invalid. Must be a verified email address or domain.'
6. Re-replay sesv2.email-identity-lifecycle to confirm the fix (it also re-asserts the Delete/Get not-found messages, which already match and must keep matching).
