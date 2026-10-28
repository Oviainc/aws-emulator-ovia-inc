Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.delivery-options-invalid-tls-policy-rejected ses.custom-verification-template-unverified-from-fails
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.delivery-options-invalid-tls-policy-rejected|run#9|AssertionError|assert err == (
Issue-Key: ses.custom-verification-template-unverified-from-fails|run#7|AssertionError|assert err == "FromEmailAddressNotVerified", (
Issue-Commit: 415bc37b6ebe55c6c7e638f2212d4379947a3e7c

# ses: 2 net-new behavioral root causes — the PutConfigurationSetDeliveryOptions enum-violation ValidationError message is rewritten (quotes the submitted value, wrong enum order), and CreateCustomVerificationEmailTemplate never refuses an unverified FromEmailAddress

> _Note: this issue was identified against the code base at commit `415bc37b6ebe55c6c7e638f2212d4379947a3e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConfigurationSetDeliveryOptions

## Description
Two independent net-new root causes in the SES (v1) control plane, found by replaying the behavioral suite. Both belong to the `ses` service. 2 causes in this issue; both must be closed before submitting.

=== Cause 1 of 2: PutConfigurationSetDeliveryOptions renders an enum-constraint ValidationError with its own wording ===

AWS: recorded behavior only (the Smithy model declares the constraint but not the message text). A PutConfigurationSetDeliveryOptions whose DeliveryOptions.TlsPolicy is not a modeled enum value is refused with HTTP 400 and Error.Code `ValidationError`, and the message is the standard AWS constraint-violation envelope:
  "<N> validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<enum values>]"
The fixed text is everything outside the angle brackets, including the absence of any quoted offending value: AWS does NOT echo the submitted value for this enum constraint. The variable parts are the count of violations, the member path taken from the request's wire member (here `deliveryOptions.tlsPolicy`), and the enum value set taken from the model and rendered in AWS's order. Smithy: PutConfigurationSetDeliveryOptions.DeliveryOptions -> com.amazonaws.ses#DeliveryOptions$TlsPolicy -> enum com.amazonaws.ses#TlsPolicy, whose declaration order is [Require, Optional]; AWS renders the set sorted — [Optional, Require]. The operation's modeled errors (ConfigurationSetDoesNotExistException, InvalidDeliveryOptionsException) are not used for an enum violation; the protocol-level ValidationError is.

Emulator: the same request is refused with the right status and the right code (`ValidationError`) but a different message: "1 validation error detected: Value 'NotAPolicy' at 'deliveryOptions.tlsPolicy' failed to satisfy constraint: Member must satisfy enum value set: [Require, Optional]". Two divergences in the fixed text: (a) it inserts "Value '<submitted value>' at" where AWS emits "Value at" — the quoted token is whatever string the request carried, so it is a value, not part of the template; (b) it lists the enum set in model-declaration order instead of AWS's sorted order.

Why: a client or test that asserts on the refusal text — the common way an enum rejection is distinguished from the other 400s this operation can return — sees a message AWS never emits, and the emulator additionally reflects the caller's rejected input back in the error, which AWS does not do for this constraint class. The positive-control half of the check (a valid TlsPolicy=Require lands and survives the rejected write) does pass, so only the message is wrong.

Fix: the SES v1 enum-constraint violation renderer used by PutConfigurationSetDeliveryOptions' validation stage (the shared pre-handler constraint validation for this crate, not the operation handler body) — emit the envelope without the submitted value and order the enum value set as AWS does.

Traces: ses.delivery-options-invalid-tls-policy-rejected.

=== Cause 2 of 2: CreateCustomVerificationEmailTemplate answers 501 NotImplemented, so an unverified FromEmailAddress is never refused with FromEmailAddressNotVerified ===

AWS: Smithy: CreateCustomVerificationEmailTemplate models FromEmailAddressNotVerifiedException (errorKind client, HTTP 400, awsQueryError code `FromEmailAddressNotVerified`), documented as "Indicates that the sender address specified for a custom verification email is not verified, and is therefore not eligible to send the custom verification email"; the exception additionally carries a FromEmailAddress member naming the offending address. The behavior required is a total one: the call with a FromEmailAddress whose verification status is not Success is refused with that code and MUST leave nothing behind (a following GetCustomVerificationEmailTemplate for the same TemplateName must fail with `CustomVerificationEmailTemplateDoesNotExist`), while the identical call differing only in the sender's verification state succeeds and GetCustomVerificationEmailTemplate echoes the recorded FromEmailAddress. That is what makes the rejection specific to the unverified sender rather than a blanket denial of the operation.

Emulator: every CreateCustomVerificationEmailTemplate is answered HTTP 501 with Error.Code `NotImplemented` — the handler is still the generated scaffold stub (services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_custom_verification_email_template.rs, the `Scaffolded::not_implemented` return at line 193, with the SCAFFOLDED marker at lines 4-6). The replay shows the check asking for `FromEmailAddressNotVerified` and getting `'NotImplemented'`, so neither the refusal nor the verified-sender success path exists.

Why: the whole custom-verification-template surface is unusable, and more subtly a caller cannot tell a correctly-refused unverified sender from an operation that is denied outright — the two differ by status (400 vs 501) and code, so error handling branches the wrong way and a client that treats 501 as retryable/transient retries forever.

Note: the suite also files a scaffold-stub issue naming this same operation and handler file (it was attributed to ses.custom-verification-template-not-found-semantics). This cause is filed because the check below was NOT marked as covered by it, and it records the behavioral contract the implementation must satisfy beyond merely existing: the FromEmailAddressNotVerified refusal, its no-side-effect guarantee, and the verified-sender positive control. Close them together.

Fix: implement the operation in its own handler file (services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_custom_verification_email_template.rs), resolving the sender's verification state from the shared SES identity state and returning FromEmailAddressNotVerifiedException before any template is persisted.

Traces: ses.custom-verification-template-unverified-from-fails.

## Evidence
Cause 1 — check ses.delivery-options-invalid-tls-policy-rejected (verdict: mismatch):
  AssertionError: expected ValidationError for an unknown TlsPolicy, got ('ValidationError', "1 validation error detected: Value 'NotAPolicy' at 'deliveryOptions.tlsPolicy' failed to satisfy constraint: Member must satisfy enum value set: [Require, Optional]")
  Expected (AWS): ("ValidationError", "1 validation error detected: Value at 'deliveryOptions.tlsPolicy' failed to satisfy constraint: Member must satisfy enum value set: [Optional, Require]")
  Actual (emulator): ("ValidationError", "1 validation error detected: Value 'NotAPolicy' at 'deliveryOptions.tlsPolicy' failed to satisfy constraint: Member must satisfy enum value set: [Require, Optional]")
  Fixed text vs values: fixed — "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]"; values from the request/model — the member path `deliveryOptions.tlsPolicy` and the enum set `Optional, Require`. The token 'NotAPolicy' is the request's submitted value and must not appear at all. Status and code (400 / ValidationError) match; only the message diverges.

Cause 2 — check ses.custom-verification-template-unverified-from-fails (verdict: mismatch):
  AssertionError: expected FromEmailAddressNotVerified for an unverified sender, got 'NotImplemented'
  Expected (AWS, Smithy-modeled): HTTP 400, Error.Code FromEmailAddressNotVerified; then GetCustomVerificationEmailTemplate for that name -> CustomVerificationEmailTemplateDoesNotExist; then the same create with a verified sender -> 200 and GetCustomVerificationEmailTemplate echoes FromEmailAddress.
  Actual (emulator): HTTP 501, Error.Code NotImplemented for every CreateCustomVerificationEmailTemplate call (scaffold stub).

## Steps to Reproduce
Cause 1 (ses.delivery-options-invalid-tls-policy-rejected):
1. ses.create_configuration_set(ConfigurationSet={"Name": <fresh name>}).
2. ses.put_configuration_set_delivery_options(ConfigurationSetName=<name>, DeliveryOptions={"TlsPolicy": "Require"}) and confirm describe_configuration_set(ConfigurationSetAttributeNames=["deliveryOptions"]) reports TlsPolicy=Require (positive control, currently passes).
3. ses.put_configuration_set_delivery_options(ConfigurationSetName=<name>, DeliveryOptions={"TlsPolicy": "NotAPolicy"}).
4. Observe the ClientError: code is ValidationError but the message quotes the submitted value and orders the enum set [Require, Optional]; AWS emits "1 validation error detected: Value at 'deliveryOptions.tlsPolicy' failed to satisfy constraint: Member must satisfy enum value set: [Optional, Require]".

Cause 2 (ses.custom-verification-template-unverified-from-fails):
1. ses.verify_email_identity(EmailAddress=<fresh address>) so the address exists but is not Success-verified.
2. ses.create_custom_verification_email_template(TemplateName=<fresh name>, FromEmailAddress=<that address>, TemplateSubject="Please verify", TemplateContent="<html><body>Please verify your address.</body></html>", SuccessRedirectionURL=..., FailureRedirectionURL=...).
3. Observe HTTP 501 NotImplemented instead of HTTP 400 FromEmailAddressNotVerified.
4. ses.get_custom_verification_email_template(TemplateName=<name>) must fail with CustomVerificationEmailTemplateDoesNotExist, and the identical create with a Success-verified sender must succeed and read back its FromEmailAddress.
