Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.op-contract-update-certificate-options
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.op-contract-update-certificate-options|run#10|AssertionError|assert code_validation_method == "ValidationException", (
Issue-Commit: 7feb740d187aee5730d4bf3c5bee10f4f6d55579

# ACM UpdateCertificateOptions refuses a not-yet-ISSUED certificate with InvalidStateException instead of ValidationException "Certificate is not issued" (1 cause)

> _Note: this issue was identified against the code base at commit `7feb740d187aee5730d4bf3c5bee10f4f6d55579`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCertificateOptions

## Description
REOPENS 20261029T190818Z-acm-certificate-control-plane-importcertificate-and-updatecertificateoptions-refuse.md: its fix left the call refused. That issue is marked fixed, but its Resolution explicitly DECLINED one half of its cause 2 — the modeled error class and sentence `UpdateCertificateOptions` must answer for a certificate that is not ISSUED — and the behavioral check it carried as a replay key, `acm.op-contract-update-certificate-options`, still fails on exactly that first assertion. The open WIRE filing 20261029T192604Z-acm-certificate-control-plane-refusals-a-valid-import.md records the same rule as its cause 6, but it is keyed on fixture#step bindings (acm-request-certificate-dns-lifecycle#task#4, acm-update-certificate-options-validation-method#task#3) and carries no behavioral check id, so this check is re-replayed by nothing open.

Cause 1 of 1: `UpdateCertificateOptions` on a certificate whose status is not ISSUED is refused with the processing-state class instead of the request-validation class, under different text.

AWS: Smithy `UpdateCertificateOptions(CertificateArn required Arn @length 20..2048 @pattern ^arn:...:acm:...$, Options required CertificateOptions) -> Unit` models ConflictException, InvalidArnException, InvalidStateException, LimitExceededException, ResourceNotFoundException and ValidationException, all client errors at HTTP 400 (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json). Of those, ValidationException is documented "The supplied input failed to satisfy constraints of an Amazon Web Services service" and InvalidStateException "Processing has reached an invalid state". Recorded behavior: changing an `Options` member — here migrating the renewal `ValidationMethod` from EMAIL to DNS — on a certificate still PENDING_VALIDATION is refused HTTP 400 `ValidationException` with the message `Certificate is not issued`. That message is entirely FIXED text: no ARN, domain, status or other request/state value is interpolated into it, and it carries no terminating period. The certificate's Status, its per-domain ValidationMethod and its Options are all left unchanged by the refusal. The companion request-level rule for the same operation — `Options.Export` naming a value the certificate does not already carry — is refused ValidationException `Export option for certificates cannot be updated`, also wholly fixed text; the emulator already answers that leg, and the check never reaches it because it aborts on the status leg first.

Emulator: the status precondition answers HTTP 400 `InvalidStateException` with the one sentence the emulator reports for that whole class, `The selected certificate is not Issued and/or Valid.` — the wrong modeled code, and fixed text that matches no ACM message for this operation. The handler decides this at services/service-com-amazonaws-acm-certificatemanager/src/ops/update_certificate_options.rs:280-287, which raises `InvalidStateException` with `crate::state::NOT_ISSUED_MESSAGE` for every non-ISSUED status, and the module doc at lines 16-24 states that choice deliberately.

Why: the code is the branch a client takes. ACM answers the request-validation class for this precondition, so a caller that catches ValidationException (what the operation's contract directs for a rejected `Options` change) sees an unhandled InvalidStateException and aborts the whole options migration; conversely a caller that catches InvalidStateException treats the refusal as transient processing state and retries. Because ACM reports several distinct conditions under one code per class, the message is the only discriminator a client has, and `The selected certificate is not Issued and/or Valid.` tells a client nothing it can match against ACM's `Certificate is not issued`. An email-to-DNS validation migration — the documented purpose of this operation — is therefore unimplementable against the emulator by a client written to AWS.

Note for the fixer, since the declined resolution rested on it: four release-corpus units were cited as asserting `InvalidStateException` for this same call (acm.update-certificate-options-pending-validation-fails, acm.update-certificate-options-invalid-state-rejected, acm.update-certificate-options-validation-method-change-while-pending-rejected, acm.certificate-transparency-logging-preference-deprecated-field-ignored). Those four pass today. This check and the two wire steps of the open filing's cause 6 assert ValidationException for the same request shape, so the two recordings genuinely disagree and nothing in the request separates them; resolving this cause means deciding that conflict rather than only editing the handler. Hypothesis: the two groups were recorded against different ACM revisions and the three units demanding ValidationException are the newer, since they agree with the WIRE corpus as well as this behavioral corpus.

Fix: the status precondition in the UpdateCertificateOptions handler (services/service-com-amazonaws-acm-certificatemanager/src/ops/update_certificate_options.rs, the non-ISSUED branch) and the shared not-issued message constant it draws on in crate::state — not in deserialization or serialization, which are correct.

Traces: acm.op-contract-update-certificate-options.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the booted emulator: 221/265 passed; acm 220/255 with 35 failures, of which this is the only one not already owned by an open issue or filed behind an unimplemented-stub cluster.

[mismatch] acm.op-contract-update-certificate-options — AssertionError at check_acm_op_contract_update_certificate_options.py:42:
  assert code_validation_method == "ValidationException", (...)
  AssertionError: expected ValidationException, got InvalidStateException

The check's sequence: acm.request_certificate(DomainName=<unique>.example.com, ValidationMethod='EMAIL', Options={'CertificateTransparencyLoggingPreference':'ENABLED'}) -> DescribeCertificate confirms Status PENDING_VALIDATION and DomainValidationOptions[0].ValidationMethod EMAIL (both passed), then acm.update_certificate_options(CertificateArn=<arn>, Options={'ValidationMethod':'DNS'}). Expected Error.Code 'ValidationException' and Error.Message exactly 'Certificate is not issued'; observed Error.Code 'InvalidStateException' (HTTP 400) with 'The selected certificate is not Issued and/or Valid.'. Both strings are wholly fixed text — the ARN and domain in the request appear in neither, so nothing per-run is being compared.

Unreached behind the first assertion, and therefore ungraded by this run: the Options={'Export':'ENABLED'} leg (expects ValidationException 'Export option for certificates cannot be updated'; the handler does raise ValidationException with that constant at update_certificate_options.rs:258-269) and the post-refusal assertions that Status stays PENDING_VALIDATION, DomainValidationOptions[0].ValidationMethod stays EMAIL and Options.Export is unchanged.

Emulator source: services/service-com-amazonaws-acm-certificatemanager/src/ops/update_certificate_options.rs:280-287 returns UpdateCertificateOptionsErrorModeled::InvalidStateException { message: crate::state::NOT_ISSUED_MESSAGE } whenever certificate.status != crate::state::STATUS_ISSUED.

Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json): UpdateCertificateOptions input {CertificateArn: Arn required, @length 20..2048, @pattern ^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$; Options: CertificateOptions required}, output Unit, errors ConflictException/InvalidArnException/InvalidStateException/LimitExceededException/ResourceNotFoundException/ValidationException — all errorKind client, httpError 400.

## Steps to Reproduce
1. Boot the emulator and point botocore's acm client at it.
2. arn = acm.request_certificate(DomainName='<unique>.example.com', ValidationMethod='EMAIL', Options={'CertificateTransparencyLoggingPreference':'ENABLED'})['CertificateArn'].
3. Confirm acm.describe_certificate(CertificateArn=arn)['Certificate']['Status'] == 'PENDING_VALIDATION' and DomainValidationOptions[0]['ValidationMethod'] == 'EMAIL'.
4. Call acm.update_certificate_options(CertificateArn=arn, Options={'ValidationMethod':'DNS'}) and read the ClientError: the emulator answers HTTP 400 Error.Code 'InvalidStateException' / Error.Message 'The selected certificate is not Issued and/or Valid.'; AWS answers HTTP 400 Error.Code 'ValidationException' / Error.Message 'Certificate is not issued' (fixed text, nothing interpolated).
5. Then call acm.update_certificate_options(CertificateArn=arn, Options={'Export':'ENABLED'}) — AWS answers ValidationException 'Export option for certificates cannot be updated'.
6. Re-describe the certificate and confirm neither refusal changed Status, DomainValidationOptions[0].ValidationMethod or Options.Export.
7. Re-replay acm.op-contract-update-certificate-options; it must pass end to end (all three legs plus the unchanged-state assertions) before closing. Because the four release-corpus units named in the description assert the opposite code for the same call, confirm they still pass or that the conflict has been resolved deliberately.
