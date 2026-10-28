Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.export-non-exportable-certificate-types-rejected
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.export-non-exportable-certificate-types-rejected|run#30|AssertionError|assert exportable_code == "ResourceNotFoundException", (
Issue-Commit: c55de139d049c24eb73a834719d1f6cb55a92322

# ExportCertificate on an export-enabled certificate that is not yet ISSUED answers RequestInProgressException instead of ResourceNotFoundException "Could not find an issued certificate <arn>"

> _Note: this issue was identified against the code base at commit `c55de139d049c24eb73a834719d1f6cb55a92322`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportCertificate

## Description
One cause.

Cause 1 of 1: ExportCertificate misclassifies the refusal for a certificate that WAS created with Options={"Export":"ENABLED"} but has not reached ISSUED (still PENDING_VALIDATION). Failing check id: acm.export-non-exportable-certificate-types-rejected.

AWS: ExportCertificate resolves an ISSUED certificate. When the ARN names an export-enabled certificate that has not been issued, ACM refuses the call as "no issued certificate under this ARN": error code ResourceNotFoundException (HTTP 400) with the message `Could not find an issued certificate <CertificateArn>`. Fixed text of that message: `Could not find an issued certificate ` ; the single variable part is the `CertificateArn` taken verbatim from the request. Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json): ExportCertificate models BOTH `ResourceNotFoundException` ("The specified certificate cannot be found in the caller's account or the caller's account cannot be found.") and `RequestInProgressException` ("The certificate request is in process and the certificate in your account has not yet been issued."), each HTTP 400; the model does not say WHICH of the two covers an unissued export-enabled certificate, so the choice of ResourceNotFoundException and the message wording above are recorded behavior only. The same recording confirms the two sibling paths the emulator already gets right: a default (no Export opt-in) AMAZON_ISSUED certificate and an IMPORTED certificate are both refused with ValidationException and the message `Certificate ARN: <CertificateArn> is not an export enabled certificate`, and neither refusal mutates the certificate — those assertions passed.

Emulator: the call is refused with error code `RequestInProgressException` (HTTP 400) carrying the fixed `crate::state::REQUEST_IN_PROGRESS_MESSAGE` sentence, so both the code and the message diverge. The handler reaches this because it treats "ACM holds no private-key material for this record" as the terminal case and returns RequestInProgressException unconditionally once the export-allowed gate passes; it never classifies "this ARN names no ISSUED certificate" for an export-enabled record, and it never renders the `Could not find an issued certificate <arn>` message anywhere.

Why: a client that distinguishes the two modeled errors is pushed onto the wrong path. RequestInProgressException reads as "retry, issuance is still running", so SDK/CLI callers and waiters poll ExportCertificate indefinitely for a certificate whose DNS validation will never complete, instead of taking the ResourceNotFoundException branch that tells them there is no issued certificate to export yet. Error-code-driven branching and retry policies written against AWS therefore behave differently against the emulator, and the message a caller surfaces to an operator names the wrong condition.

Fix: in the ExportCertificate handler's refusal classification — services/service-com-amazonaws-acm-certificatemanager/src/ops/export_certificate.rs, the terminal branch after the `certificate.export_allowed()` gate — plus a not-yet-issued message builder alongside `crate::state::certificate_not_found_message` / `crate::state::not_export_enabled_message` in services/service-com-amazonaws-acm-certificatemanager/src/state.rs. The gate must distinguish an export-enabled record whose status is not ISSUED (ResourceNotFoundException with the `Could not find an issued certificate <arn>` template) from a genuinely in-flight issuance, rather than returning RequestInProgressException for every export-enabled record.

Traces: acm.export-non-exportable-certificate-types-rejected (assertion at check line 93: "an Export=ENABLED certificate must be refused as not-yet-issued, not with the ValidationException of a non-exportable type, got 'RequestInProgressException'").

## Evidence
Check: acm.export-non-exportable-certificate-types-rejected [mismatch]

AssertionError: an Export=ENABLED certificate must be refused as not-yet-issued, not with the ValidationException of a non-exportable type, got 'RequestInProgressException'
  at check_acm_export_non_exportable_certificate_types_rejected.py line 93:
    assert exportable_code == "ResourceNotFoundException"
  followed by:
    expected_message = f"Could not find an issued certificate {exportable_arn}"
    assert exportable_message == expected_message

Expected (AWS, recorded): HTTP 400, Error.Code = ResourceNotFoundException, Error.Message = "Could not find an issued certificate <exportable_arn>".
Actual (emulator):        HTTP 400, Error.Code = RequestInProgressException, Error.Message = the fixed REQUEST_IN_PROGRESS_MESSAGE sentence ("the certificate request is in process ... not yet been issued").

Earlier assertions in the same check PASSED against the emulator, so the divergence is isolated to the export-enabled/unissued path:
  - default AMAZON_ISSUED cert -> ValidationException, "Certificate ARN: <arn> is not an export enabled certificate" (match)
  - IMPORTED cert -> ValidationException, same template (match), certificate state unmutated (match)

Source: services/service-com-amazonaws-acm-certificatemanager/src/ops/export_certificate.rs lines 227-255 — after the `export_allowed()` ValidationException gate, the handler returns RequestInProgressException unconditionally; no branch produces ResourceNotFoundException for an export-enabled record that is not ISSUED.
Model: ExportCertificate declares InvalidArnException, RequestInProgressException, ResourceNotFoundException, ThrottlingException, ValidationException (all HTTP 400) — ResourceNotFoundException is available and modeled.

## Steps to Reproduce
1. acm.request_certificate(DomainName="<unique-label>.example.com", ValidationMethod="DNS", Options={"Export": "ENABLED"}) and keep the returned CertificateArn. The domain is one that never validates, so the certificate stays PENDING_VALIDATION.
2. acm.export_certificate(CertificateArn=<that arn>, Passphrase=b"Sim2RealPass123").
3. Observe the refusal: the emulator returns Error.Code = RequestInProgressException; AWS returns Error.Code = ResourceNotFoundException with Error.Message = "Could not find an issued certificate <that arn>".
4. Full replay: run behavioral check acm.export-non-exportable-certificate-types-rejected (requires the `openssl` tool for the IMPORTED leg).
