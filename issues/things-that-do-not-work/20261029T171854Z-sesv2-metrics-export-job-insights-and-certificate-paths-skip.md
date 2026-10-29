Issue-Source: trace-sweep-divergence
Issue-Trace: sesv2-account-pricing-attributes-validation-and-success
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sesv2-account-pricing-attributes-validation-and-success sesv2-batch-get-account-level-metrics sesv2-batch-get-metrics-invalid-date-range sesv2-message-insights-unknown-message-id sesv2-email-address-insights-simulator sesv2-email-address-insights-validation sesv2-export-job-invalid-date-range sesv2-export-job-invalid-request-errors sesv2-associate-certificate-nonexistent-arn-error sesv2-identity-certificate-association-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|PutAccountPricingAttributes|4xx->4xx|$.message
Issue-Key: sesv2|BatchGetMetricData|4xx->4xx|$.message
Issue-Key: sesv2|BatchGetMetricData|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sesv2|BatchGetMetricData|4xx->4xx|status
Issue-Key: sesv2|GetMessageInsights|4xx->4xx|$.message
Issue-Key: sesv2|GetMessageInsights|4xx->4xx|$headers.content-type
Issue-Key: sesv2|GetMessageInsights|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: sesv2|GetMessageInsights|4xx->4xx|status
Issue-Key: sesv2|GetEmailAddressInsights|2xx->2xx|$.MailboxValidation.Evaluations.HasValidDnsRecords.ConfidenceVerdict
Issue-Key: sesv2|GetEmailAddressInsights|2xx->2xx|$.MailboxValidation.IsValid.ConfidenceVerdict
Issue-Key: sesv2|CreateExportJob|4xx->2xx|$.message
Issue-Key: sesv2|CreateExportJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: sesv2|CreateExportJob|4xx->2xx|status
Issue-Key: sesv2|CreateExportJob|4xx->4xx|$.message
Issue-Key: sesv2|ListEmailIdentityCertificates|2xx->2xx|$
Issue-Key: sesv2|ListEmailIdentityCertificates|2xx->2xx|$.Certificates[*].Status
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Metrics, export-job, insights and certificate paths skip validation and VDM gating

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportJob

## Description
Nine net-new causes across the SESv2 metrics, export-job, insights and S/MIME-certificate surfaces.

Cause 1 of 9: PutAccountPricingAttributes enum validation message lists the enum in the wrong order. AWS: 400 BadRequestException with the standard validation sentence "1 validation error detected: Value at 'plan' failed to satisfy constraint: Member must satisfy enum value set: [ENTERPRISE, ESSENTIALS, PRO, NONE]" — the fixed text is the validation template, the variable parts are the member path ('plan'), the offending value and the enum value set, which AWS renders in the order shown. Emulator: the same sentence but with the set rendered '[NONE, ESSENTIALS, PRO, ENTERPRISE]' (the model's declaration order). Why: a client asserting on, or parsing the allowed-value list out of, AWS's message sees a different set. Fix: the PutAccountPricingAttributes input validation's enum-set rendering. The open issue 20261026T013649Z-sesv2-putaccountpricingattributes-is-an-unimplemented-scaffold-stub-every.md describes the operation refusing 501; it now validates, so that issue does not cover this. Traces: sesv2-account-pricing-attributes-validation-and-success#task#1.

Cause 2 of 9: BatchGetMetricData does not gate on Virtual Deliverability Manager. AWS: when VDM is not enabled for the account the call is refused 404 NotFoundException (x-amzn-errortype: NotFoundException) with the fixed sentence 'To use this feature you must enable Virtual Deliverability Manager' (no variable part). Smithy: BatchGetMetricData errors include NotFoundException(404) and BadRequestException(400). Emulator: 400 BadRequestException with its own sentence 'VDM is not enabled for your account.'. Why: a client branching on status/error type treats a missing-feature condition as a malformed request and never follows the documented 'enable VDM' path. Fix: the BatchGetMetricData handler's VDM pre-check (error class and message). Traces: sesv2-batch-get-account-level-metrics#task#2.

Cause 3 of 9: BatchGetMetricData invalid date-range message. AWS: 400 BadRequestException with the fixed sentence 'Expected start date to be before the end date.' — no variable part. Emulator: 'Start date <start> must be before end date <end>', interpolating the request's timestamps. Why: a client matching AWS's text does not recognise the error. Fix: the BatchGetMetricData date-range validation message. Traces: sesv2-batch-get-metrics-invalid-date-range#task#2.

Cause 4 of 9: GetMessageInsights does not gate on VDM either. AWS: with VDM disabled, 404 NotFoundException with 'To use this feature you must enable Virtual Deliverability Manager', regardless of whether the message id is known. Emulator: 400 BadRequestException 'Invalid MessageId', and the response also carries content-type application/json instead of application/x-amz-json-1.1 (recorded behavior only). Why: as Cause 2 — the client is told its input is bad rather than that the feature is off. Fix: the GetMessageInsights handler's VDM pre-check, ordered ahead of message-id lookup. Traces: sesv2-message-insights-unknown-message-id#task#1.

Cause 5 of 9: GetEmailAddressInsights reports the wrong confidence verdict. AWS: for the simulator/validation address used here the mailbox validation returns ConfidenceVerdict HIGH both for MailboxValidation.IsValid and for Evaluations.HasValidDnsRecords. Emulator: returns MEDIUM for both. Why: a client that only accepts HIGH-confidence validation rejects an address AWS validates confidently. Fix: the GetEmailAddressInsights evaluation/verdict computation. Traces: sesv2-email-address-insights-simulator#task#1 (same cluster also at sesv2-email-address-insights-validation#task#1).

Cause 6 of 9: CreateExportJob accepts an inverted date range. AWS: 400 BadRequestException (x-amzn-errortype: BadRequestException) with the fixed sentence 'Expected start date to be before the end date.'. Smithy: CreateExportJob errors BadRequestException(400)/LimitExceededException(400)/NotFoundException(404). Emulator: 200 with a JobId, no message and no error header — the job is created. Why: a client gets a job id for an impossible range and discovers the problem only later (or never), where AWS refuses up front. Fix: the CreateExportJob handler's ExportDataSource date-range validation. Traces: sesv2-export-job-invalid-date-range#task#1 (same cluster also at sesv2-export-job-invalid-request-errors#task#3).

Cause 7 of 9: CreateExportJob custom-S3-URL refusal message. AWS: 400 BadRequestException with the fixed sentence 'Providing a custom S3 URL is not supported' — no variable part. Emulator: 'ExportDestination.S3Url is set by SES and must not be included in a CreateExportJob request.', an emulator-authored sentence naming the member. Why: a client matching AWS's text does not recognise the refusal. Fix: the CreateExportJob validation message for ExportDestination.S3Url. Traces: sesv2-export-job-invalid-request-errors#task#1.

Cause 8 of 9: an S/MIME certificate association to a nonexistent ACM certificate never reaches FAILED. AWS: associating a certificate ARN that does not exist leaves the identity certificate in Status FAILED — ListEmailIdentityCertificates reports Certificates[i].Status FAILED (and CertificateExpiryTime unset). Emulator: reports Status ACTIVE and never transitions, so the fixture's poll for FAILED exhausts its 60s budget. Why: a client that waits for the terminal state of a certificate association hangs forever, and one that trusts ACTIVE believes an unusable certificate is in force. Fix: the certificate-association state machine behind AssociateCertificate/ListEmailIdentityCertificates — validate the referenced ACM certificate and settle the record into FAILED. Traces: sesv2-associate-certificate-nonexistent-arn-error#task#3.

Cause 9 of 9: the same certificate-status defect on the association-lifecycle path — ListEmailIdentityCertificates reports ACTIVE where AWS reports FAILED. Fix: as Cause 8. Traces: sesv2-identity-certificate-association-lifecycle#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1.
Cause 1 (sesv2-account-pricing-attributes-validation-and-success#task#1, 400/400): $.message expected "1 validation error detected: Value at 'plan' failed to satisfy constraint: Member must satisfy enum value set: [ENTERPRISE, ESSENTIALS, PRO, NONE]", got "... [NONE, ESSENTIALS, PRO, ENTERPRISE]".
Cause 2 (sesv2-batch-get-account-level-metrics#task#2): status expected 404, got 400; $headers.x-amzn-errortype expected 'NotFoundException', got 'BadRequestException'; $.message expected 'To use this feature you must enable Virtual Deliverability Manager', got 'VDM is not enabled for your account.'
Cause 3 (sesv2-batch-get-metrics-invalid-date-range#task#2, 400/400): $.message expected 'Expected start date to be before the end date.', got 'Start date 2026-09-05T18:44 must be before end date 2026-09-04T18:44'.
Cause 4 (sesv2-message-insights-unknown-message-id#task#1): status expected 404, got 400; x-amzn-errortype expected 'NotFoundException', got 'BadRequestException'; $.message expected 'To use this feature you must enable Virtual Deliverability Manager', got 'Invalid MessageId'; content-type application/json vs application/x-amz-json-1.1.
Cause 5 (sesv2-email-address-insights-simulator#task#1, 200/200): $.MailboxValidation.IsValid.ConfidenceVerdict and $.MailboxValidation.Evaluations.HasValidDnsRecords.ConfidenceVerdict expected 'HIGH', got 'MEDIUM'.
Cause 6 (sesv2-export-job-invalid-date-range#task#1): status expected 400, got 200; $.message expected 'Expected start date to be before the end date.', got absent; x-amzn-errortype expected 'BadRequestException', got absent.
Cause 7 (sesv2-export-job-invalid-request-errors#task#1, 400/400): $.message expected 'Providing a custom S3 URL is not supported', got 'ExportDestination.S3Url is set by SES and must not be included in a CreateExportJob request.'
Cause 8 (sesv2-associate-certificate-nonexistent-arn-error#task#3, 200/200): $.Certificates[0].Status expected 'FAILED', got 'ACTIVE'; poll not satisfied after 61 attempts / capped at 60s.
Cause 9 (sesv2-identity-certificate-association-lifecycle#task#5, 200/200): $.Certificates[0].Status expected 'FAILED', got 'ACTIVE'.

## Steps to Reproduce
1. Replay fixtures sesv2-account-pricing-attributes-validation-and-success (task#1), sesv2-batch-get-account-level-metrics (task#2), sesv2-batch-get-metrics-invalid-date-range (task#2), sesv2-message-insights-unknown-message-id (task#1), sesv2-email-address-insights-simulator (task#1), sesv2-export-job-invalid-date-range (task#1), sesv2-export-job-invalid-request-errors (task#1), sesv2-associate-certificate-nonexistent-arn-error (task#3) and sesv2-identity-certificate-association-lifecycle (task#5).
2. PutAccountPricingAttributes with an invalid plan value and compare the validation message's enum-set ordering.
3. With VDM not enabled, call BatchGetMetricData and GetMessageInsights and compare status, x-amzn-errortype and message.
4. Call BatchGetMetricData and CreateExportJob with StartDate after EndDate and compare status and message.
5. Call CreateExportJob with ExportDestination.S3Url set and compare the 400 message.
6. GetEmailAddressInsights for the simulator address and compare the ConfidenceVerdict values.
7. Associate an S/MIME certificate ARN that does not exist, then poll ListEmailIdentityCertificates for the terminal status.
