Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.custom-verification-template-unverified-sender-and-absent-name-rejected ses.send-custom-verification-email-sandbox-gate
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.custom-verification-template-unverified-sender-and-absent-name-rejected|run#10|AssertionError|assert (code, message) == ("CustomVerificationEmailTemplateDoesNotExist", _ABSENT), (
Issue-Key: ses.send-custom-verification-email-sandbox-gate|run#4|AssertionError|assert err == (
Issue-Commit: ff63bbd76f191efb9689374a4f98215607f4bd70

# ses: 2 net-new refusal-message divergences in the custom-verification-email plane — the CustomVerificationEmailTemplateDoesNotExist absence message is emulator-worded (and echoes the requested TemplateName), and SendCustomVerificationEmail's ProductionAccessNotGranted sentence is invented

> _Note: this issue was identified against the code base at commit `ff63bbd76f191efb9689374a4f98215607f4bd70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCustomVerificationEmailTemplate

## Description
2 net-new root causes in the SES (v1 / Query) custom-verification-email surface. Both are refusal-MESSAGE fidelity defects: in each case the HTTP status (400) and the Error.Code are already right, and only the message text diverges. Neither cause changes which error is chosen or in what order the gates run — the emulator's gate ORDER for SendCustomVerificationEmail is correct (see Cause 2).

Cause 1 of 2: the CustomVerificationEmailTemplateDoesNotExist absence message is worded by the emulator and interpolates the request's TemplateName.

AWS:      Resolving a custom verification email template by a name the account does not hold is refused with HTTP 400 and error code CustomVerificationEmailTemplateDoesNotExist. Smithy (services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json): GetCustomVerificationEmailTemplate models exactly one error, CustomVerificationEmailTemplateDoesNotExistException (errorKind client, httpError 400, awsQueryError code CustomVerificationEmailTemplateDoesNotExist, doc "Indicates that a custom verification email template with the name you specified does not exist."); UpdateCustomVerificationEmailTemplate and SendCustomVerificationEmail bind the same error. The model does not state message text, so the message is recorded behavior only: AWS answers the fixed sentence "Provided custom verification email template name does not exist." EVERY word of that message is FIXED TEXT — it has no variable part, and in particular it does not echo the TemplateName from the request.

Emulator: the same refusal carries "Custom verification email template <TEMPLATE_NAME> does not exist.", where the fixed parts are "Custom verification email template <" and "> does not exist." and TEMPLATE_NAME is the request's TemplateName member interpolated between angle brackets. So the emulator both uses different fixed text and adds a request-derived variable part that AWS's message does not have.

Why:      A client that asserts on, logs or branches on the absence message for a custom verification email template sees a sentence that never appears in AWS; message-equality assertions in customer suites and SES tooling fail against the emulator even though operation, status and code are right. The emulator also reflects the caller's requested template name back in a message AWS deliberately keeps name-free, so anything scraping the message learns a value it would not learn from AWS.

Fix:      The shared absence-message helper in the SES service's state module (crate::state::missing_custom_verification_email_template_message) — one wording used by every operation that resolves a custom verification email template by name, so correcting the helper corrects Get/Update/Delete/SendCustomVerificationEmail at once. The operation handlers already select the right error variant and status; no handler logic changes.

Related open issue: 20261029T165506Z-ses-customverificationemailtemplatedoesnotexist-message-interpolates-the-requested-template-name.md describes the same helper for the check ses.custom-verification-template-unverified-from-fails. That issue does NOT carry this check's replay key, so this check is still failing and un-owned; fixing the helper should close both, and this issue's check id must be re-replayed to confirm.

Traces:   failing check ses.custom-verification-template-unverified-sender-and-absent-name-rejected. The check's first phase passes (CreateCustomVerificationEmailTemplate with an unverified FromEmailAddress is refused FromEmailAddressNotVerified with the expected message), then the read-back of that rejected name returns the right code with the wrong message and the check stops there. Emitting site: services/service-com-amazonaws-ses-simpleemailservice/src/state.rs (missing_custom_verification_email_template_message, ~line 2880), reached from services/service-com-amazonaws-ses-simpleemailservice/src/ops/get_custom_verification_email_template.rs and from services/service-com-amazonaws-ses-simpleemailservice/src/ops/send_custom_verification_email.rs:255-259.

Cause 2 of 2: SendCustomVerificationEmail's ProductionAccessNotGranted message is an emulator-invented sentence.

AWS:      SendCustomVerificationEmail is gated on account production access BEFORE anything the request names is resolved: a sandbox account is refused HTTP 400 with error code ProductionAccessNotGranted even when the TemplateName does not exist. Smithy: SendCustomVerificationEmail errors include ProductionAccessNotGrantedException (errorKind client, httpError 400, awsQueryError code ProductionAccessNotGranted, doc "Indicates that the account has not been granted production access."); the model does not state the message text, so the message is recorded behavior only: AWS answers the fixed sentence "Production access not granted. Please request production access prior to sending custom verification emails" (no trailing period). All of it is FIXED TEXT: it carries no value from the request (not the EmailAddress, not the TemplateName) and none from state.

Emulator: the gate itself is correct — the handler refuses on the account flag before resolving the configuration set, the template or the address — but it answers Error.Message "The account has not been granted production access.", a sentence of its own derived from the error shape's documentation. Code (ProductionAccessNotGranted) and status (400) match; only the message diverges.

Why:      A client that detects the sandbox condition by message text — the common case, because the sentence is the only place AWS tells the caller to request production access — gets text it cannot match, and loses the actionable "Please request production access prior to sending custom verification emails" instruction. Any message-equality assertion on the sandbox gate fails even though the emulator refuses correctly and in the right order.

Fix:      The constant in the operation's handler — services/service-com-amazonaws-ses-simpleemailservice/src/ops/send_custom_verification_email.rs, NO_PRODUCTION_ACCESS (line 132), used at lines 205-214. Only the string needs AWS's wording; the surrounding gate and its placement are already right.

Traces:   failing check ses.send-custom-verification-email-sandbox-gate.

## Evidence
Both causes are grounded in the behavioral replay against the emulator's SES (Query/v1) endpoint.

Cause 1 — check ses.custom-verification-template-unverified-sender-and-absent-name-rejected (verdict: mismatch)

Expected (AWS), for GetCustomVerificationEmailTemplate (and UpdateCustomVerificationEmailTemplate) on a name the account does not hold:
  HTTP 400
  Error.Code    = "CustomVerificationEmailTemplateDoesNotExist"
  Error.Message = "Provided custom verification email template name does not exist."
                  <- entirely fixed text; no request- or state-derived value

Actual (emulator):
  HTTP 400
  Error.Code    = "CustomVerificationEmailTemplateDoesNotExist"   (matches)
  Error.Message = "Custom verification email template <TEMPLATE_NAME> does not exist."
                  fixed text "Custom verification email template <" + "> does not exist."
                  around a variable part taken from the request's TemplateName member.

Replay assertion failure:
  File "check_ses_custom_verification_template_unverified_sender_and_absent_name_rejected.py", line 42
  assert (code, message) == ("CustomVerificationEmailTemplateDoesNotExist", _ABSENT)
  AssertionError: getting the refused template gave 'CustomVerificationEmailTemplateDoesNotExist':
  'Custom verification email template <sim2real-behav-cvt-08949977dc85> does not exist.'
  (the quoted name is a per-run generated value, not part of the requirement)

Source of the diverging wording:
  services/service-com-amazonaws-ses-simpleemailservice/src/state.rs (~2880)
    pub fn missing_custom_verification_email_template_message(template_name: &str) -> String {
        format!("Custom verification email template <{template_name}> does not exist.")
    }
  call sites include ops/get_custom_verification_email_template.rs and
  ops/send_custom_verification_email.rs:255-259.

Cause 2 — check ses.send-custom-verification-email-sandbox-gate (verdict: mismatch)

Expected (AWS), for SendCustomVerificationEmail from a sandbox account (TemplateName absent, EmailAddress fresh):
  HTTP 400
  Error.Code    = "ProductionAccessNotGranted"
  Error.Message = "Production access not granted. Please request production access prior to sending custom verification emails"
                  <- entirely fixed text, no trailing period, no request/state values

Actual (emulator):
  HTTP 400
  Error.Code    = "ProductionAccessNotGranted"   (matches — gate order is also correct:
                  the account flag is read before the template lookup)
  Error.Message = "The account has not been granted production access."

Replay assertion failure:
  File "check_ses_send_custom_verification_email_sandbox_gate.py", line 45
  AssertionError: expected ProductionAccessNotGranted (sandbox gate before template lookup),
  got ('ProductionAccessNotGranted', 'The account has not been granted production access.')

Source of the diverging wording:
  services/service-com-amazonaws-ses-simpleemailservice/src/ops/send_custom_verification_email.rs:132
    const NO_PRODUCTION_ACCESS: &str = "The account has not been granted production access.";
  used at lines 205-214.

Smithy grounding (services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json):
  SendCustomVerificationEmail errors = [ConfigurationSetDoesNotExistException,
    CustomVerificationEmailTemplateDoesNotExistException, FromEmailAddressNotVerifiedException,
    MessageRejected, ProductionAccessNotGrantedException] — all client, httpError 400.
  ProductionAccessNotGrantedException doc = "Indicates that the account has not been granted production access."
  GetCustomVerificationEmailTemplate errors = [CustomVerificationEmailTemplateDoesNotExistException]
    (client, 400, awsQueryError code CustomVerificationEmailTemplateDoesNotExist).

## Steps to Reproduce
Cause 1 (CustomVerificationEmailTemplateDoesNotExist message):
1. Against the emulator's SES (Query/v1) endpoint, pick a TemplateName that was never created.
2. Call CreateCustomVerificationEmailTemplate with that name and an UNVERIFIED FromEmailAddress, a subject, HTML content and the two redirection URLs. It is refused FromEmailAddressNotVerified with the expected message (correct), so nothing is stored.
3. Call GetCustomVerificationEmailTemplate with the SAME TemplateName.
4. Observe HTTP 400, Error.Code CustomVerificationEmailTemplateDoesNotExist, Error.Message "Custom verification email template <that name> does not exist."
5. AWS answers the same status and code with the fixed message "Provided custom verification email template name does not exist." — no template name is echoed.
6. Same divergence from UpdateCustomVerificationEmailTemplate on that name. One-step equivalent: GetCustomVerificationEmailTemplate on any never-created name and compare Error.Message.

Cause 2 (ProductionAccessNotGranted message):
1. Against the same endpoint, with the account still in the sandbox (production access NOT granted).
2. Call SendCustomVerificationEmail with EmailAddress=<a fresh address> and TemplateName=<a name that does not exist>.
3. Observe HTTP 400 with Error.Code ProductionAccessNotGranted (correct, and correctly chosen before the template lookup) and Error.Message "The account has not been granted production access."
4. AWS answers the same status and code with "Production access not granted. Please request production access prior to sending custom verification emails".
5. Confirm the rest of the check's expectation still holds: the target address must NOT appear in ListIdentities(IdentityType="EmailAddress") after the gated call.
