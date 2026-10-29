Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.custom-verification-template-unverified-from-fails
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.custom-verification-template-unverified-from-fails|run#11|AssertionError|assert (get_code, get_message) == ("CustomVerificationEmailTemplateDoesNotExist", _ABSENT_MESSAGE), (
Issue-Commit: 31e185c10437ee55d835d207df74f07fcd501141

# SES CustomVerificationEmailTemplateDoesNotExist message interpolates the requested template name instead of AWS's fixed wording

> _Note: this issue was identified against the code base at commit `31e185c10437ee55d835d207df74f07fcd501141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCustomVerificationEmailTemplate

## Description
1 net-new root cause in the SES custom-verification-email-template plane.

Cause 1 of 1: the absence message for CustomVerificationEmailTemplateDoesNotExist is worded by the emulator, not by AWS.

AWS:      GetCustomVerificationEmailTemplate for a template name the account does not hold is refused with HTTP 400 and error code CustomVerificationEmailTemplateDoesNotExist. Smithy: GetCustomVerificationEmailTemplate models exactly one error, CustomVerificationEmailTemplateDoesNotExistException (errorKind client, httpError 400, awsQueryError code CustomVerificationEmailTemplateDoesNotExist), documented as "Indicates that a custom verification email template with the name you specified does not exist." The model does not state the message text, so the message is recorded behavior only: AWS answers the fixed sentence "Provided custom verification email template name does not exist." Every word of that message is FIXED TEXT — it has no variable part, and in particular it does not echo the TemplateName from the request. The status (400) and the code (CustomVerificationEmailTemplateDoesNotExist) are already correct in the emulator; only the message diverges.

Emulator: the same refusal carries the message "Custom verification email template <TEMPLATE_NAME> does not exist.", where TEMPLATE_NAME is the TemplateName member of the request interpolated between angle brackets. So the emulator invents both different fixed text and a variable part AWS's message does not have.

Why:      A client that asserts on, logs, or branches on the error message for a missing custom verification email template sees text that never appears in AWS. Message-equality assertions in customer test suites and in SES-facing tooling fail against the emulator even though the operation, status and error code are right; and the emulator reflects the caller's requested template name back in a message AWS deliberately keeps name-free, so anything scraping the message learns a field it would not learn from AWS.

Fix:      The shared message helper for this absence in the SES service's state module — it is one wording used by every operation that resolves a custom verification email template by name, so correcting the helper corrects all of them at once; the operation handler already selects the right error variant and status. Call sites to re-check once the wording is fixed are the operations that resolve a template out of custom_verification_email_templates (Get/Update/Delete/SendCustomVerificationEmail).

Traces:   failing check ses.custom-verification-template-unverified-from-fails. The check first confirms the positive refusal path (CreateCustomVerificationEmailTemplate with an unverified FromEmailAddress returns FromEmailAddressNotVerified with the expected message — that part passes), then asserts the rejected name was not left behind by reading it back; the read-back returns the right code with the wrong message. Emitting site: services/service-com-amazonaws-ses-simpleemailservice/src/state.rs:2880-2882 (missing_custom_verification_email_template_message), reached from services/service-com-amazonaws-ses-simpleemailservice/src/ops/get_custom_verification_email_template.rs:159-178.

## Evidence
Check: ses.custom-verification-template-unverified-from-fails (verdict: mismatch).

Expected (AWS), for GetCustomVerificationEmailTemplate on a name that does not exist:
  HTTP 400
  Error.Code    = "CustomVerificationEmailTemplateDoesNotExist"
  Error.Message = "Provided custom verification email template name does not exist."   <- entirely fixed text, no request-derived values

Actual (emulator):
  HTTP 400
  Error.Code    = "CustomVerificationEmailTemplateDoesNotExist"   (matches)
  Error.Message = "Custom verification email template <TEMPLATE_NAME> does not exist."
                  fixed text "Custom verification email template <" + "> does not exist." around a
                  variable part taken from the request's TemplateName member.

Replay assertion failure:
  AssertionError: expected the rejected custom verification template to not exist, got
  'CustomVerificationEmailTemplateDoesNotExist': 'Custom verification email template
  <sim2real-behav-cvtmpl-...> does not exist.'
  (the quoted template name is a per-run generated name, not part of the requirement)

Source of the diverging wording:
  services/service-com-amazonaws-ses-simpleemailservice/src/state.rs:2880-2882
    pub fn missing_custom_verification_email_template_message(template_name: &str) -> String {
        format!("Custom verification email template <{template_name}> does not exist.")
    }
  used by services/service-com-amazonaws-ses-simpleemailservice/src/ops/get_custom_verification_email_template.rs:169-173.

Smithy (services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json):
  GetCustomVerificationEmailTemplate errors = [CustomVerificationEmailTemplateDoesNotExistException,
  client, httpError 400, awsQueryError code CustomVerificationEmailTemplateDoesNotExist];
  doc = "Indicates that a custom verification email template with the name you specified does not exist."

## Steps to Reproduce
1. Against the emulator's SES (Query/v1) endpoint, call VerifyEmailIdentity for a fresh address so a sender exists but is unverified.
2. Call CreateCustomVerificationEmailTemplate with TemplateName=<a name that does not yet exist>, that unverified FromEmailAddress, a subject, HTML content and the two redirection URLs. It is refused with FromEmailAddressNotVerified (correct), so no template is created.
3. Call GetCustomVerificationEmailTemplate with the SAME TemplateName.
4. Observe HTTP 400 with Error.Code CustomVerificationEmailTemplateDoesNotExist and Error.Message "Custom verification email template <that name> does not exist."
5. AWS answers the same status and code with the fixed message "Provided custom verification email template name does not exist." — no template name is echoed.
6. Equivalent one-step reproduction: call GetCustomVerificationEmailTemplate with any TemplateName that was never created and compare Error.Message.
