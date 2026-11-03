Issue-Source: trace-sweep-divergence
Issue-Trace: email-receipt-ruleset-cloning-and-ip-filters
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-receipt-ruleset-cloning-and-ip-filters email-send-email-variants-round-trip email-configuration-set-full-lifecycle email-configuration-set-tracking-options-without-verified-domain email-custom-verification-email-template-workflow
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: email|CloneReceiptRuleSet|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|CreateReceiptFilter|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|SendEmail|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|SendRawEmail|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|SendTemplatedEmail|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|SendBulkTemplatedEmail|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|CreateConfigurationSetTrackingOptions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: email|CreateConfigurationSetTrackingOptions|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|UpdateConfigurationSetTrackingOptions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: email|UpdateConfigurationSetTrackingOptions|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|GetCustomVerificationEmailTemplate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: email|GetCustomVerificationEmailTemplate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|SendCustomVerificationEmail|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: email|SendCustomVerificationEmail|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# SES v1 refusal messages are emulator-authored and the send/clone refusal envelopes omit RequestId

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
10 net-new wire causes on the SES v1 (awsQuery, endpointPrefix `email`, apiVersion 2010-12-01) refusal path, in two families. Causes 1-6 are further instances of the error-envelope defect whose other ten instances are in the companion issue 'SES v1 Query error envelope omits the RequestId element on every refusal' (same single serializer; listed here so each cluster has a replay binding a fixer must re-run). Causes 7-10 are refusal-MESSAGE fidelity defects: status and Error.Code already match, only Error.Message text diverges, AND the same four responses also lack RequestId.

Family A — missing RequestId in the refusal envelope (causes 1-6).
AWS:      Smithy services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json — SimpleEmailService 2010-12-01, protocols [awsQuery], xmlNamespace http://ses.amazonaws.com/doc/2010-12-01/. The model does not spell out the envelope, so this is recorded behavior only: every recorded SES v1 refusal body is `<ErrorResponse><Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error><RequestId>…</RequestId></ErrorResponse>`. The RequestId VALUE is a per-request uuid (never a requirement); its PRESENCE, carrying the refused request's id, is.
Emulator: emits `<ErrorResponse><Error>…</Error></ErrorResponse>` with no RequestId sibling. Replay detail: `$.ErrorResponse.RequestId: missing (… got None)`.
Why:      an SDK exposes nothing for the error's request id, so refusal logs cannot be correlated and any assertion that the error envelope is well formed fails even though the refusal itself is correct.
Fix:      the SES crate's shared awsQuery error-envelope serializer (the same one named in the companion issue) — emit RequestId after Error from the per-request id source the success-path ResponseMetadata writer already uses.

Cause 1 of 10: CloneReceiptRuleSet refusal answers 400 with the right code and no RequestId. Traces: email-receipt-ruleset-cloning-and-ip-filters#task#5 (cluster also covers #task#6).
Cause 2 of 10: CreateReceiptFilter refusal answers 400 with the right code and no RequestId. Traces: email-receipt-ruleset-cloning-and-ip-filters#task#9.
Cause 3 of 10: SendEmail refusal answers 400 with the right code and no RequestId. Traces: email-send-email-variants-round-trip#task#3 (cluster also covers #task#8).
Cause 4 of 10: SendRawEmail refusal answers 400 with the right code and no RequestId. Traces: email-send-email-variants-round-trip#task#4.
Cause 5 of 10: SendTemplatedEmail refusal answers 400 with the right code and no RequestId. Traces: email-send-email-variants-round-trip#task#5 (cluster also covers #task#7).
Cause 6 of 10: SendBulkTemplatedEmail refusal answers 400 with the right code and no RequestId. Traces: email-send-email-variants-round-trip#task#6.

Family B — emulator-authored refusal messages (causes 7-10).

Cause 7 of 10: CreateConfigurationSetTrackingOptions refuses an unverified custom tracking (redirect) domain with its own sentence.
AWS:      Smithy: CreateConfigurationSetTrackingOptions errors = ConfigurationSetDoesNotExistException, InvalidTrackingOptionsException (awsQueryError code InvalidTrackingOptions), TrackingOptionsAlreadyExistsException — all errorKind client, httpError 400. The model states no message text, so the message is recorded behavior only: AWS answers `Domain <DOMAIN> is not verified under this account.` Fixed text is `Domain <` + `> is not verified under this account.` — the angle brackets are part of AWS's fixed text — and the single variable part is DOMAIN, the TrackingOptions.CustomRedirectDomain value taken from the request.
Emulator: same 400 and same Error.Code, but Error.Message `The tracking domain you specified is not verified in Amazon SES.` — different fixed text and no variable part at all, so the caller is never told WHICH domain was rejected. The response also omits RequestId (Family A).
Why:      a client that surfaces or branches on this sentence cannot match it and, worse, loses the domain name that tells the operator which DNS record to publish; message-equality assertions fail though the refusal class is right.
Fix:      the tracking-options validation path in the SES crate's tracking-options handlers/state helper that composes the unverified-domain message — one wording shared by the Create and Update paths (see cause 8).
Traces:   email-configuration-set-full-lifecycle#task#10 (cluster also covers email-configuration-set-tracking-options-without-verified-domain#task#2).

Cause 8 of 10: UpdateConfigurationSetTrackingOptions has the same re-worded unverified-domain refusal.
AWS:      same error family and same recorded template as cause 7 — `Domain <DOMAIN> is not verified under this account.`, DOMAIN taken from the request's TrackingOptions.CustomRedirectDomain; status 400, awsQueryError code InvalidTrackingOptions.
Emulator: `The tracking domain you specified is not verified in Amazon SES.` plus the missing RequestId.
Why:      as cause 7; the update path is what callers use to repoint an existing configuration set's click-tracking domain, so the uninformative sentence hides which domain failed verification.
Fix:      the shared unverified-tracking-domain message helper reached from the UpdateConfigurationSetTrackingOptions handler — correcting it with cause 7 fixes both.
Traces:   email-configuration-set-tracking-options-without-verified-domain#task#3.

Cause 9 of 10: GetCustomVerificationEmailTemplate's absence message is emulator-worded and echoes the requested name.
AWS:      Smithy: GetCustomVerificationEmailTemplate models exactly one error, CustomVerificationEmailTemplateDoesNotExistException (client, httpError 400, awsQueryError code CustomVerificationEmailTemplateDoesNotExist). Message text is recorded behavior only: AWS answers the fixed sentence `Provided custom verification email template name does not exist.` — ENTIRELY fixed text, with no variable part and in particular no echo of the request's TemplateName.
Emulator: 400 and the right code, but Error.Message `Custom verification email template <NAME> does not exist.` — different fixed text plus a request-derived variable part (the TemplateName between angle brackets). RequestId also missing.
Why:      clients that match on the absence sentence never see AWS's text, and the emulator reflects the caller's requested template name back in a message AWS keeps name-free.
Fix:      the shared absence-message helper in the SES crate's state module used by every operation that resolves a custom verification email template by name.
Related open issues: 20261029T165506Z-ses-customverificationemailtemplatedoesnotexist-message-interpolates-the-requested-template-name.md and 20261029T171402Z-ses-2-net-new-refusal-message-divergences-in.md describe the same helper from BEHAVIORAL checks. Neither carries this wire step's replay key, so this cluster is still un-owned; fixing the helper should close all of them, and this issue's fixture#step must be re-replayed to confirm.
Traces:   email-custom-verification-email-template-workflow#task#3.

Cause 10 of 10: SendCustomVerificationEmail's sandbox refusal message is emulator-invented.
AWS:      Smithy: SendCustomVerificationEmail errors include ProductionAccessNotGrantedException (client, httpError 400, awsQueryError code ProductionAccessNotGranted). Message is recorded behavior only: AWS answers the fixed sentence `Production access not granted. Please request production access prior to sending custom verification emails` — all fixed text, NO trailing period, no request- or state-derived value.
Emulator: 400 and the right code (the gate is also correctly placed before the template lookup), but Error.Message `The account has not been granted production access.` — the error shape's documentation re-used as prose. RequestId also missing.
Why:      the sentence is the only place AWS tells the caller to request production access; a client matching on it gets unmatchable text and loses the actionable instruction.
Fix:      the sandbox-gate message constant in the SendCustomVerificationEmail handler; the gate and its ordering are already right.
Related open issue: 20261029T171402Z-ses-2-net-new-refusal-message-divergences-in.md covers the same constant from a behavioral check and does NOT carry this wire step's replay key.
Traces:   email-custom-verification-email-template-workflow#task#5.

## Evidence
Replayed corpus simparity-wire-aws-v1 against the booted emulator (emulator_up=true). All ten clusters are verdict=mismatch with expected_status == actual_status == 400.

Family A (causes 1-6) — single diverging key `$.ErrorResponse.RequestId: missing (… got None)`:
  CloneReceiptRuleSet        email-receipt-ruleset-cloning-and-ip-filters#task#5   (affected also #task#6)
  CreateReceiptFilter        email-receipt-ruleset-cloning-and-ip-filters#task#9
  SendEmail                  email-send-email-variants-round-trip#task#3          (affected also #task#8)
  SendRawEmail               email-send-email-variants-round-trip#task#4
  SendTemplatedEmail         email-send-email-variants-round-trip#task#5          (affected also #task#7)
  SendBulkTemplatedEmail     email-send-email-variants-round-trip#task#6
Expected envelope: <ErrorResponse><Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error><RequestId>…</RequestId></ErrorResponse>
Actual envelope:   same without <RequestId>. (Recorded RequestId values are per-run uuids / '<shaped-not-asserted>'; presence is the requirement.)

Family B (causes 7-10) — message mismatch plus the same missing RequestId:

Cause 7, CreateConfigurationSetTrackingOptions, email-configuration-set-full-lifecycle#task#10 (affected also email-configuration-set-tracking-options-without-verified-domain#task#2):
  $.ErrorResponse.Error.Message: value_mismatch
    expected 'Domain <DOMAIN> is not verified under this account.'
             fixed text: 'Domain <' + '> is not verified under this account.'
             variable:   DOMAIN = request TrackingOptions.CustomRedirectDomain
    got      'The tracking domain you specified is not verified in Amazon SES.'
  $.ErrorResponse.RequestId: missing (got None)

Cause 8, UpdateConfigurationSetTrackingOptions, email-configuration-set-tracking-options-without-verified-domain#task#3:
  $.ErrorResponse.Error.Message: value_mismatch
    expected 'Domain <DOMAIN> is not verified under this account.'  (DOMAIN from the request)
    got      'The tracking domain you specified is not verified in Amazon SES.'
  $.ErrorResponse.RequestId: missing (got None)

Cause 9, GetCustomVerificationEmailTemplate, email-custom-verification-email-template-workflow#task#3:
  $.ErrorResponse.Error.Message: value_mismatch
    expected 'Provided custom verification email template name does not exist.'   <- all fixed text
    got      'Custom verification email template <NAME> does not exist.'          <- NAME = request TemplateName
  $.ErrorResponse.RequestId: missing (got None)
  Error.Code = CustomVerificationEmailTemplateDoesNotExist on both sides (matches).

Cause 10, SendCustomVerificationEmail, email-custom-verification-email-template-workflow#task#5:
  $.ErrorResponse.Error.Message: value_mismatch
    expected 'Production access not granted. Please request production access prior to sending custom verification emails'  <- all fixed text, no trailing period
    got      'The account has not been granted production access.'
  $.ErrorResponse.RequestId: missing (got None)
  Error.Code = ProductionAccessNotGranted on both sides (matches).

Smithy grounding: CreateConfigurationSetTrackingOptions -> {ConfigurationSetDoesNotExistException, InvalidTrackingOptionsException (code InvalidTrackingOptions), TrackingOptionsAlreadyExistsException}, all client/400. GetCustomVerificationEmailTemplate -> {CustomVerificationEmailTemplateDoesNotExistException} (client/400, code CustomVerificationEmailTemplateDoesNotExist). SendCustomVerificationEmail -> {ConfigurationSetDoesNotExistException, CustomVerificationEmailTemplateDoesNotExistException, FromEmailAddressNotVerifiedException, MessageRejected, ProductionAccessNotGrantedException}, all client/400. None of these models the message text, hence 'recorded behavior only' for the wording, taken from the recorded responses.

## Steps to Reproduce
Family A (causes 1-6):
1. Point an SES v1 (Query) client at the emulator's data listener.
2. Drive each refusal and capture the RAW body: CloneReceiptRuleSet onto an existing rule-set name; CreateReceiptFilter with a filter name/CIDR the service refuses; SendEmail / SendRawEmail / SendTemplatedEmail / SendBulkTemplatedEmail from a Source identity that is not verified.
3. Observe 400 with the right <Code> and no <RequestId> element in <ErrorResponse>.

Cause 7/8 (tracking-options wording):
1. Create a configuration set.
2. Call CreateConfigurationSetTrackingOptions with TrackingOptions.CustomRedirectDomain set to a subdomain of a domain NOT verified in the account.
3. Observe 400, code InvalidTrackingOptions, message 'The tracking domain you specified is not verified in Amazon SES.'
4. AWS answers the same status/code with 'Domain <that-domain> is not verified under this account.' — the domain from the request appears between the literal angle brackets.
5. Repeat with UpdateConfigurationSetTrackingOptions on a configuration set that already has tracking options, using another unverified domain.

Cause 9 (template absence wording):
1. Call GetCustomVerificationEmailTemplate with a TemplateName that was never created.
2. Observe 400, code CustomVerificationEmailTemplateDoesNotExist, message 'Custom verification email template <that name> does not exist.'
3. AWS answers the fixed 'Provided custom verification email template name does not exist.' with no name echoed.

Cause 10 (sandbox gate wording):
1. With the account still in the sandbox, call SendCustomVerificationEmail with any EmailAddress and any TemplateName.
2. Observe 400, code ProductionAccessNotGranted, message 'The account has not been granted production access.'
3. AWS answers 'Production access not granted. Please request production access prior to sending custom verification emails' (no trailing period).

Finally re-replay all ten fixture#step bindings named in this issue (issue_trace/issue_step plus trace_ids).
