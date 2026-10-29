Issue-Source: trace-sweep-divergence
Issue-Trace: acm-certificate-error-paths
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acm-certificate-error-paths acm-import-certificate-get-and-reimport-cycle acm-import-certificate-invalid-material-then-recover acm-certificate-export-renew-eligibility-edge-cases acm-request-certificate-email-validation-resend acm-request-public-certificate-dns-validation-lifecycle acm-renew-certificate-not-eligible-error acm-request-certificate-dns-lifecycle acm-update-certificate-options-validation-method
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|ImportCertificate|2xx->4xx|status
Issue-Key: acm|ImportCertificate|4xx->4xx|$.__type
Issue-Key: acm|ImportCertificate|4xx->4xx|$.message
Issue-Key: acm|GetCertificate|4xx->4xx|$.message
Issue-Key: acm|ResendValidationEmail|4xx->4xx|$.message
Issue-Key: acm|RenewCertificate|4xx->4xx|$.__type
Issue-Key: acm|RenewCertificate|4xx->4xx|$.message
Issue-Key: acm|UpdateCertificateOptions|4xx->4xx|$.__type
Issue-Key: acm|UpdateCertificateOptions|4xx->4xx|$.message
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# ACM certificate control-plane refusals: a valid import is rejected and five refusals carry the wrong code or message (6 causes)

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportCertificate

## Description
Six distinct root causes in how the ACM emulator refuses certificate control-plane calls: one call that AWS accepts is refused outright (blocking the rest of its fixture), and five refusals arrive with the wrong modeled error code and/or a hand-written message. Related open filing: 20261029T190818Z-acm-certificate-control-plane-importcertificate-and-updatecertificateoptions-refuse.md records the same classification/wording rules from the BEHAVIORAL suite — its Issue-Trace-Ids are check ids (acm.op-contract-*) and it carries NO fixture/step replay key, so none of the six wire steps below is covered or re-replayed by it; causes 2-6 here are the wire bindings for that family, and cause 1 appears in neither.

Cause 1 of 6: ImportCertificate refuses certificate material that AWS accepts, with HTTP 400 instead of 200.
AWS: Smithy ImportCertificate(Certificate required blob @length 1..32768, PrivateKey required sensitive blob @length 1..5120, CertificateChain blob @length 1..2097152, Tags list @length 1..50) -> ImportCertificateResponse{CertificateArn}. Recorded behavior: the fixture's setup imports one PEM-encoded end-entity certificate with its matching private key and AWS answers HTTP 200 with the new CertificateArn, leaving the certificate ISSUED with Type IMPORTED.
Emulator: the same request is refused with HTTP 400, so no certificate exists for the fixture's subsequent error-path probes; the replay reports 1 blocked step behind this refusal.
Why: this is the entry point for every imported-certificate flow — a client that imports material real ACM accepts cannot create the certificate at all, and everything keyed on it (DescribeCertificate, GetCertificate, tagging, options, revoke, the negative probes this fixture exists to exercise) is unreachable. An over-strict import check also hides itself: the caller sees a client-error status and concludes its own material is bad.
Fix: the request-validation path of ImportCertificate in the ACM service crate (the PEM/key decode and acceptance checks) — accept the modeled material AWS accepts rather than refusing it.
Traces: acm-certificate-error-paths#setup#1.

Cause 2 of 6: an ImportCertificate Certificate field holding more than one certificate is classified InvalidParameterException instead of ValidationException, with different text.
AWS: Smithy ImportCertificate models InvalidParameterException and ValidationException as separate client errors, both HTTP 400. Recorded behavior: a Certificate blob containing two or more concatenated PEM certificates (the chain pasted into the leaf field) is refused 400 ValidationException with 'The certificate field contains more than one certificate. You can specify only one certificate in this field.' — entirely fixed text, nothing from the request interpolated, terminating period included.
Emulator: 400 with code InvalidParameterException and message 'the Certificate must be one PEM-encoded X.509 certificate' — a different code and invented text (lower-case opener, no period).
Why: the two codes are modeled separately and clients branch on them; a caller catching ValidationException for malformed submitted material sees an unhandled InvalidParameterException and aborts. Since ACM distinguishes the several material defects by MESSAGE under one code, the text is the only signal that tells the caller to split the chain out of the leaf field, and the emulator's wording matches no ACM message.
Fix: the refusal classification and message construction in the ImportCertificate material-decode path.
Traces: acm-import-certificate-get-and-reimport-cycle#task#4 (cluster of 2, with acm-import-certificate-invalid-material-then-recover#task#2).

Cause 3 of 6: GetCertificate on a not-yet-issued certificate carries a generic sentence instead of ACM's ARN- and account-naming message.
AWS: Smithy GetCertificate(CertificateArn required) models RequestInProgressException (client, 400). Recorded behavior: a certificate still PENDING_VALIDATION is refused 400 RequestInProgressException with 'Certificate <CertificateArn> in account <account-id> not yet issued' — fixed text 'Certificate ' / ' in account ' / ' not yet issued' around two variable parts, the ARN taken from the request and the account id taken from that ARN's 5th field, with no terminating period.
Emulator: status 400 and code RequestInProgressException are correct; the message is 'The certificate request is in process and the certificate in your account has not yet been issued.' — different fixed text naming neither the certificate nor the account.
Why: the message is produced by a known rule from the request, so the rule is the requirement. A client polling many certificates attributes the refusal using the ARN in this message; the emulator's text is unattributable, so a batch poller cannot tell which certificate is still pending.
Fix: the not-yet-issued refusal construction in the GetCertificate handler.
Traces: acm-certificate-export-renew-eligibility-edge-cases#task#7.

Cause 4 of 6: ResendValidationEmail before the initial validation email has been dispatched carries an invented, ARN-interpolating message.
AWS: Smithy ResendValidationEmail(CertificateArn required, Domain required, ValidationDomain required) models InvalidStateException (client, 400). Recorded behavior: called for the certificate's own validation domain immediately after RequestCertificate(ValidationMethod='EMAIL'), it is refused 400 with exactly 'The certificate request is still in progress. Try again later' — all fixed text, no identifier interpolated and no terminating period.
Emulator: the message is 'The request for certificate <CertificateArn> is still in progress: its initial domain validation email has not been sent yet. Try again later.' — different fixed text with the request's ARN spliced in and a trailing period added.
Why: the text is generated by a fixed rule, so the rule is the requirement. ResendValidationEmail returns this class for more than one reason, and the message is what separates the short-lived pre-dispatch gate (retry) from the certificate-is-not-EMAIL-validated-for-that-domain case (do not retry); a client matching ACM's wording matches neither of the emulator's.
Fix: the pre-dispatch gate's error construction in the ResendValidationEmail handler.
Traces: acm-certificate-export-renew-eligibility-edge-cases#task#5 (cluster of 3, with acm-request-certificate-email-validation-resend#task#3 and acm-request-public-certificate-dns-validation-lifecycle#task#2).

Cause 5 of 6: RenewCertificate on a certificate that is not issued answers RequestInProgressException where AWS answers ResourceNotFoundException, with different text.
AWS: Smithy RenewCertificate(CertificateArn required) -> Unit models InvalidArnException, RequestInProgressException, ResourceNotFoundException and ValidationException, all client errors at 400; RenewalEligibility is documented as applying only to exported private certificates. Recorded behavior: renewing a certificate that has not been issued is refused 400 ResourceNotFoundException with 'Could not find an issued certificate <CertificateArn>' — fixed text 'Could not find an issued certificate ' plus the ARN from the request, no terminating period. ACM treats the renewal target set as 'issued certificates', so a non-issued ARN is a lookup miss, not an in-progress condition.
Emulator: 400 RequestInProgressException with 'The certificate request is in process and the certificate in your account has not yet been issued.' — the wrong modeled code and the same generic sentence it uses for GetCertificate (cause 3).
Why: the code is the branch a client takes — ResourceNotFoundException means 'this ARN is not a renewable certificate, stop', while RequestInProgressException means 'retry later'. The emulator sends a renewal client into an unbounded retry loop on a certificate that will never become renewable, and the message names no ARN so the loop cannot even report which certificate it is stuck on.
Fix: the eligibility/lookup check in the RenewCertificate handler — raise ResourceNotFoundException with the issued-certificate text instead of reusing the in-progress error.
Traces: acm-certificate-export-renew-eligibility-edge-cases#task#9 (cluster of 2, with acm-renew-certificate-not-eligible-error#task#3).

Cause 6 of 6: UpdateCertificateOptions on a certificate that is not ISSUED is refused InvalidStateException instead of ValidationException, with different text.
AWS: Smithy UpdateCertificateOptions(CertificateArn required, Options required) -> Unit models ConflictException, InvalidArnException, InvalidStateException, LimitExceededException, ResourceNotFoundException and ValidationException, all client errors at 400. Recorded behavior: changing Options (here the renewal ValidationMethod) on a certificate still PENDING_VALIDATION is refused 400 ValidationException with the fixed message 'Certificate is not issued' — no identifier interpolated, no terminating period; the certificate's Status and Options are left unchanged.
Emulator: 400 InvalidStateException with 'The selected certificate is not Issued and/or Valid.' — the state class instead of the request-validation class ACM uses for this precondition, and invented text.
Why: ACM deliberately answers the request-validation class here; a client catching ValidationException (what the contract directs for this precondition) sees an unhandled InvalidStateException and aborts, while a client catching InvalidStateException retries a call that cannot succeed for this reason until validation completes.
Fix: the precondition check in the UpdateCertificateOptions handler — raise ValidationException with the fixed text above.
Traces: acm-request-certificate-dns-lifecycle#task#4 (cluster of 2, with acm-update-certificate-options-validation-method#task#3).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (78 fixtures, 388 steps, 344 passed, 1 blocked). Every cause below is verdict=mismatch.

Cause 1 — acm-certificate-error-paths#setup#1, ImportCertificate: 'status: expected 200, got 400' (divergence_key service=acm|ImportCertificate|status=200->400|keys=status). The fixture's setup is the import that every later step in it depends on; the run reports steps_blocked=1 behind it.
Cause 2 — acm-import-certificate-get-and-reimport-cycle#task#4, ImportCertificate (cluster of 2, +acm-import-certificate-invalid-material-then-recover#task#2), both at HTTP 400: $.__type expected 'ValidationException', got 'InvalidParameterException'; $.message expected 'The certificate field contains more than one certificate. You can specify only one certificate in this field.', got 'the Certificate must be one PEM-encoded X.509 certificate'. Both strings are wholly fixed text — no request value appears in either.
Cause 3 — acm-certificate-export-renew-eligibility-edge-cases#task#7, GetCertificate at HTTP 400: $.message expected 'Certificate arn:aws:acm:us-west-2:000000000000:certificate/295480d4-3378-4541-9f85-ff27e67e5b33 in account 000000000000 not yet issued', got 'The certificate request is in process and the certificate in your account has not yet been issued.' Code RequestInProgressException matched. The ARN and account id in the expectation are per-run values filling the template (ARN from the request, account id from its 5th ARN field); the fixed text is 'Certificate ' / ' in account ' / ' not yet issued'.
Cause 4 — acm-certificate-export-renew-eligibility-edge-cases#task#5, ResendValidationEmail at HTTP 400 (cluster of 3, +acm-request-certificate-email-validation-resend#task#3, acm-request-public-certificate-dns-validation-lifecycle#task#2): $.message expected 'The certificate request is still in progress. Try again later', got 'The request for certificate arn:aws:acm:us-west-2:000000000000:certificate/295480d4-3378-4541-9f85-ff27e67e5b33 is still in progress: its initial domain validation email has not been sent yet. Try again later.' Code matched; the ARN is a per-run value in the emulator's template and appears nowhere in AWS's message.
Cause 5 — acm-certificate-export-renew-eligibility-edge-cases#task#9, RenewCertificate at HTTP 400 (cluster of 2, +acm-renew-certificate-not-eligible-error#task#3): $.__type expected 'ResourceNotFoundException', got 'RequestInProgressException'; $.message expected 'Could not find an issued certificate arn:aws:acm:us-west-2:000000000000:certificate/295480d4-3378-4541-9f85-ff27e67e5b33', got 'The certificate request is in process and the certificate in your account has not yet been issued.' Fixed text is 'Could not find an issued certificate ' plus the request's ARN.
Cause 6 — acm-request-certificate-dns-lifecycle#task#4, UpdateCertificateOptions at HTTP 400 (cluster of 2, +acm-update-certificate-options-validation-method#task#3): $.__type expected 'ValidationException', got 'InvalidStateException'; $.message expected 'Certificate is not issued', got 'The selected certificate is not Issued and/or Valid.' Both are wholly fixed text.
Smithy citations (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json): ImportCertificate(Certificate required blob @length 1..32768, PrivateKey required sensitive blob @length 1..5120, CertificateChain blob, Tags @length 1..50) errors ConflictException/InvalidArnException/InvalidParameterException/InvalidTagException/LimitExceededException/ResourceNotFoundException/TagPolicyException/TooManyTagsException/ValidationException, all 400; GetCertificate(CertificateArn required) errors InvalidArnException/RequestInProgressException/ResourceNotFoundException/ValidationException (400); RenewCertificate(CertificateArn required)->Unit errors InvalidArnException/RequestInProgressException/ResourceNotFoundException/ValidationException (400); ResendValidationEmail(CertificateArn, Domain, ValidationDomain all required)->Unit errors InvalidArnException/InvalidDomainValidationOptionsException/InvalidStateException/ResourceNotFoundException/ValidationException (400); UpdateCertificateOptions(CertificateArn required, Options required)->Unit errors ConflictException/InvalidArnException/InvalidStateException/LimitExceededException/ResourceNotFoundException/ValidationException (400); CertificateDetail.RenewalEligibility documented as applying to exported private certificates.

## Steps to Reproduce
1. Boot the emulator and replay trace set simparity-wire-aws-v1.
2. Cause 1: acm.import_certificate(Certificate=<single PEM end-entity certificate>, PrivateKey=<its matching key>) — the emulator answers HTTP 400 where AWS answers 200 with a CertificateArn. Re-replay acm-certificate-error-paths#setup#1 and confirm its blocked follow-on step runs.
3. Cause 2: acm.import_certificate(Certificate=<two concatenated PEM certificates>, PrivateKey=<leaf key>) — observe Error.Code InvalidParameterException / 'the Certificate must be one PEM-encoded X.509 certificate'; AWS answers ValidationException 'The certificate field contains more than one certificate. You can specify only one certificate in this field.' Re-replay acm-import-certificate-get-and-reimport-cycle#task#4.
4. Cause 3: acm.request_certificate(DomainName='<d>', ValidationMethod='DNS'), confirm PENDING_VALIDATION, then acm.get_certificate(CertificateArn=<arn>) — compare Error.Message to 'Certificate <arn> in account <account-id> not yet issued'. Re-replay acm-certificate-export-renew-eligibility-edge-cases#task#7.
5. Cause 4: acm.request_certificate(DomainName='<d>', ValidationMethod='EMAIL') and immediately acm.resend_validation_email(CertificateArn=<arn>, Domain='<d>', ValidationDomain='<d>') — compare Error.Message to 'The certificate request is still in progress. Try again later'. Re-replay acm-certificate-export-renew-eligibility-edge-cases#task#5.
6. Cause 5: acm.renew_certificate(CertificateArn=<a PENDING_VALIDATION certificate>) — observe RequestInProgressException; AWS answers ResourceNotFoundException 'Could not find an issued certificate <arn>'. Re-replay acm-certificate-export-renew-eligibility-edge-cases#task#9.
7. Cause 6: acm.update_certificate_options(CertificateArn=<a PENDING_VALIDATION certificate>, Options={'ValidationMethod': 'DNS'}) — observe InvalidStateException 'The selected certificate is not Issued and/or Valid.'; AWS answers ValidationException 'Certificate is not issued'. Re-replay acm-request-certificate-dns-lifecycle#task#4.
8. All six fixture#step bindings (Issue-Trace/Issue-Step plus trace_ids) must replay clean before closing.
