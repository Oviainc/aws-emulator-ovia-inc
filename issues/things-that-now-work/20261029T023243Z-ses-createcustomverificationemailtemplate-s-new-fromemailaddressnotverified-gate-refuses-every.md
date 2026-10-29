Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.custom-verification-template-unverified-from-fails ses.custom-verification-template-not-found-semantics
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|CreateCustomVerificationEmailTemplate|FromEmailAddressNotVerified
Issue-Commit: 77670ec594ee56220d3fb04f56c8d32fdb5a5e9f

# ses: CreateCustomVerificationEmailTemplate's new FromEmailAddressNotVerified gate refuses every sender — no email identity is ever reachable in a verified state, so the operation's success path is unreachable

> _Note: this issue was identified against the code base at commit `77670ec594ee56220d3fb04f56c8d32fdb5a5e9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomVerificationEmailTemplate

## Description
REOPENS 20261028T091851Z-ses-2-net-new-behavioral-root-causes-v1.md: its fix left the call refused.

That issue's Cause 2 asked for the modelled FromEmailAddressNotVerified pre-flight on ses (v1) CreateCustomVerificationEmailTemplate, and it was marked fixed. The gate now fires, but it fires for EVERY from-address: the refusal branch was added without a reachable path by which an email identity requested through VerifyEmailIdentity ever becomes verified for sending, so the operation's 200 path can no longer be reached at all. Both checks below now fail on that refusal — the first one fails on its POSITIVE CONTROL (the create that must succeed for a verified sender), the second on the create of the template its negative assertions are controlled against. This is one cause.

Cause 1 of 1: CreateCustomVerificationEmailTemplate answers 400 FromEmailAddressNotVerified for a sender address of the account that the caller has requested verification for, and no sequence of API calls yields a from-address it accepts.

AWS: Smithy (com.amazonaws.ses#SimpleEmailService, awsQuery) CreateCustomVerificationEmailTemplate input {TemplateName, FromEmailAddress: FromAddress, TemplateSubject, TemplateContent, SuccessRedirectionURL, FailureRedirectionURL — all required}; output Unit; modelled errors CustomVerificationEmailInvalidContentException, CustomVerificationEmailTemplateAlreadyExistsException, FromEmailAddressNotVerifiedException (awsQueryError code "FromEmailAddressNotVerified", httpResponseCode 400) and LimitExceededException. The refusal class is conditional on the sender's state, not unconditional: when FromEmailAddress IS a Success-verified sending identity of the account, the call succeeds with an empty 200 body, the template is persisted, and GetCustomVerificationEmailTemplate (modelled error CustomVerificationEmailTemplateDoesNotExistException only) echoes back TemplateName, FromEmailAddress, TemplateSubject, TemplateContent and both redirection URLs. VerifyEmailIdentity {EmailAddress required} / output {} only REQUESTS verification; on real AWS the identity reaches VerificationStatus 'Success' once the confirmation link is clicked, and from then on the identical create is accepted — that transition is the state the suite's positive controls depend on, and both checks are written so that the refusal can be told apart from a blanket denial of the operation.

Emulator: for an address created with ses.verify_email_identity(EmailAddress=<addr>) and used immediately as FromEmailAddress, the create is refused with HTTP 400, Error.Code "FromEmailAddressNotVerified" and message "From email address <ADDR> is not verified." — fixed text "From email address <" + the request's FromEmailAddress + "> is not verified.", the only variable part being the address taken from the request. The negative half of ses.custom-verification-template-unverified-from-fails passes on that refusal; the check then asks for the SAME operation with a sender it has verified and gets the same 400, so the exception escapes the check uncaught (botocore.errorfactory.FromEmailAddressNotVerifiedException raised through the harness's context.py wrapper). ses.custom-verification-template-not-found-semantics never gets past its first create for the same reason. Because the v1 verify path now persists Pending (the behaviour 20261025T214303Z-ses-identity-verification-is-auto-completed-and-the.md asked for) and the emulator exposes no confirmation step, no out-of-band click, and no eventual Pending->Success transition, there is NO request sequence against the emulator that produces a from-address this handler accepts. Hypothesis: the gate reads the stored VerificationStatus correctly, and the missing half is the identity store's progression out of Pending for an emulated, inbox-less target.

Why: the operation is now 100% refusal. A client that bootstraps recipient verification cannot create a custom verification email template at all — every create, including the legitimate one, comes back 400 FromEmailAddressNotVerified — and every downstream read (GetCustomVerificationEmailTemplate, UpdateCustomVerificationEmailTemplate, ListCustomVerificationEmailTemplates, SendCustomVerificationEmail) is unreachable because no template can exist. The negative class also becomes untestable: a target that refuses unconditionally is indistinguishable from one that applies the sender gate, which is precisely what these two checks exist to separate.

Fix: the ses v1 identity state the gate reads, plus the verification lifecycle that writes it (services/service-com-amazonaws-ses-simpleemailservice/src/ops/verify_email_identity.rs and the account+region identity record in that crate's state), and the pre-flight in services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_custom_verification_email_template.rs: keep the FromEmailAddressNotVerified refusal driven by the stored status, but make a requested email identity able to reach VerificationStatus 'Success' (the emulator stands in for the inbox click that cannot happen here), so that the create's success path — persist the template, then serve it back through GetCustomVerificationEmailTemplate with FromEmailAddress echoed — is reachable. The refusal must stay atomic (nothing persisted, following GetCustomVerificationEmailTemplate refused CustomVerificationEmailTemplateDoesNotExist).

Traces: ses.custom-verification-template-unverified-from-fails, ses.custom-verification-template-not-found-semantics.

## Evidence
Expected (both checks): after the negative half is refused 400 FromEmailAddressNotVerified, the identical CreateCustomVerificationEmailTemplate with a Success-verified sender of the same account returns an empty 200 and GetCustomVerificationEmailTemplate on that TemplateName returns FromEmailAddress equal to that sender (plus TemplateSubject 'Please verify' round-tripped), and the template appears in ListCustomVerificationEmailTemplates.

Actual — replay of ses.custom-verification-template-unverified-from-fails:
  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.FromEmailAddressNotVerifiedException: An error occurred (FromEmailAddressNotVerified) when calling the CreateCustomVerificationEmailTemplate operation: From email address <sim2real-ses-sender-aa9d22f8b502@example.com> is not verified.
The raise is from the UNGUARDED positive-control create at check line 2546 (the guarded negative create earlier in the check already consumed its expected refusal), i.e. the verified-sender path is refused too.

Actual — replay of ses.custom-verification-template-not-found-semantics (same raise on its first, positive-control create):
  botocore.errorfactory.FromEmailAddressNotVerifiedException: An error occurred (FromEmailAddressNotVerified) when calling the CreateCustomVerificationEmailTemplate operation: From email address <sim2real-ses-sender-570de9b67dda@example.com> is not verified.

In both messages the address is a per-run generated value interpolated from the request's FromEmailAddress; the fixed text is "From email address <...> is not verified." The defect is the status code/condition (400 refusal where a 2xx is required for a verified sender), not the message wording.

## Steps to Reproduce
1. Boot the emulator and point boto3 at it; create a 'ses' (v1, awsQuery) client in any region.
2. ses.verify_email_identity(EmailAddress='sim2real-sender-<unique>@example.com').
3. Optional, to establish the gate is armed: ses.create_custom_verification_email_template(TemplateName='<unique-neg>', FromEmailAddress='<some-other-unverified>@example.com', TemplateSubject='Please verify', TemplateContent='<html><body>Please verify your address.</body></html>', SuccessRedirectionURL='https://example.com/success', FailureRedirectionURL='https://example.com/failure') — observe the expected 400 FromEmailAddressNotVerified, and that ses.get_custom_verification_email_template(TemplateName='<unique-neg>') is refused CustomVerificationEmailTemplateDoesNotExist.
4. Now drive the address from step 2 to a verified state by any API path the emulator offers, then issue the IDENTICAL create with FromEmailAddress='sim2real-sender-<unique>@example.com' — observe it is refused 400 FromEmailAddressNotVerified 'From email address <sim2real-sender-<unique>@example.com> is not verified.' as well, and that no call sequence (ses.verify_email_identity, repeated ses.get_identity_verification_attributes polling, ses.verify_email_address, sesv2.create_email_identity on the address) ever makes the create succeed.
5. Confirm the success path is unreachable: ses.list_custom_verification_email_templates() never contains a template, and ses.get_custom_verification_email_template on any name is CustomVerificationEmailTemplateDoesNotExist.
6. Re-replay the checks listed in trace_ids.
