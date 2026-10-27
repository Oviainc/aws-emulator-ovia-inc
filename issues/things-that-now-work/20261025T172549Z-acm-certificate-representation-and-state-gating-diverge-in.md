Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.list-certificates-summary-fields-populated acm.import-certificate-ec-key-algorithm-derived acm.import-certificate-unsupported-key-size-rejected acm.import-certificate-san-extension-parsed-from-der acm.list-certificates-key-type-filter-excludes-others acm.acm-reimport-into-an-amazon-issued-certificate-arn-rejected acm.list-tags-for-resource-rejects-certificate-arn acm.untag-resource-removes-tag-added-via-addtags acm.revoke-certificate-ineligible-type-fails acm.revoke-certificate-unexported-import-fails acm.resend-validation-email-state-gated acm.resend-validation-email-already-issued-rejected acm.update-certificate-options-invalid-state-rejected acm.op-contract-export-certificate acm.e2e-import-export-delete-consequence-flow acm.renew-certificate-ineligible-fails
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.list-certificates-summary-fields-populated|run#8|AssertionError|assert summary["KeyAlgorithm"] == "RSA-2048", f"expected KeyAlgorithm='RSA-2048', got {summary!r}"
Issue-Key: acm.import-certificate-ec-key-algorithm-derived|run#24|AssertionError|assert summaries[p256_arn]["KeyAlgorithm"] == "EC-prime256v1", (
Issue-Key: acm.import-certificate-unsupported-key-size-rejected|run#15|AssertionError|assert big_summary["KeyAlgorithm"] == "RSA-4096", (
Issue-Key: acm.import-certificate-san-extension-parsed-from-der|run#12|AssertionError|assert ":" in serial_txt, f"Serial must be colon-grouped hex, got {serial_txt!r}"
Issue-Key: acm.list-certificates-key-type-filter-excludes-others|run#19|AssertionError|assert eku_desc["ExtendedKeyUsages"] == [
Issue-Key: acm.acm-reimport-into-an-amazon-issued-certificate-arn-rejected|run#24|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: acm.list-tags-for-resource-rejects-certificate-arn|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: acm|UntagResource|ValidationException
Issue-Key: acm.revoke-certificate-ineligible-type-fails|run#6|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: acm.revoke-certificate-unexported-import-fails|run#6|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: acm.resend-validation-email-state-gated|run#8|AssertionError|assert code == "InvalidStateException", code
Issue-Key: acm.resend-validation-email-already-issued-rejected|run#11|AssertionError|assert code == "InvalidStateException", code
Issue-Key: acm.update-certificate-options-invalid-state-rejected|_error#8|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: acm.op-contract-export-certificate|run#6|AssertionError|assert code_missing == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code_missing}"
Issue-Key: acm.e2e-import-export-delete-consequence-flow|run#11|AssertionError|assert code == "ValidationException", f"ExportCertificate of an IMPORTED cert: expected ValidationException, got {code!r}"
Issue-Key: acm.renew-certificate-ineligible-fails|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: 3fffece81be5f9cdcba63f22f8252b41b774e64d

# ACM certificate representation and state-gating diverge in ten places: ListCertificates KeyAlgorithm spelling, Serial format, missing EKU OID, four wrong refusal codes/messages, an over-permissive UpdateCertificateOptions, and two remaining 501 stubs

> _Note: this issue was identified against the code base at commit `3fffece81be5f9cdcba63f22f8252b41b774e64d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCertificates

## Description
TEN distinct root causes in the ACM crate, all grounded in failing behavioral checks of this suite. A fixer must close ALL TEN before submitting; each cause below names its own failing check ids.

Cause 1 of 10: ListCertificates' CertificateSummary.KeyAlgorithm is serialized with the Smithy enum token instead of ACM's hyphenated wire value, and therefore disagrees with DescribeCertificate for the same ARN.
AWS: Smithy com.amazonaws.acm#CertificateSummary$KeyAlgorithm and com.amazonaws.acm#CertificateDetail$KeyAlgorithm both target the KeyAlgorithm enum (tokens RSA_1024, RSA_2048, RSA_3072, RSA_4096, EC_prime256v1, EC_secp384r1, EC_secp521r1). Recorded behavior: ACM renders this member on the wire in HYPHENATED form — 'RSA-2048', 'RSA-4096', 'EC-prime256v1' — and ListCertificates' summary and DescribeCertificate's detail ALWAYS agree for the same certificate. The rule is the general one: the wire value is the enum token with the separator rendered as '-', for every key type, derived from the imported key's actual algorithm and size.
Emulator: ListCertificates returns the underscore token ('RSA_2048', 'RSA_4096', 'EC_prime256v1') while DescribeCertificate for the very same ARN returns the hyphenated form, so one certificate reports two different key algorithms depending on which operation you ask.
Why: a client that filters or keys on the value AWS documents and DescribeCertificate returns never matches a ListCertificates row; a client that cross-checks the two views sees self-inconsistent state and cannot tell which is authoritative.
Fix: the ListCertificates handler and the CertificateSummary projection it serializes (services/service-com-amazonaws-acm-certificatemanager/src/ops/list_certificates.rs plus the summary builder in the crate's certificate state) must emit the same rendering DescribeCertificate already uses.
Traces: acm.list-certificates-summary-fields-populated, acm.import-certificate-ec-key-algorithm-derived, acm.import-certificate-unsupported-key-size-rejected.

Cause 2 of 10: DescribeCertificate's Serial is not colon-grouped hex.
AWS: Smithy models CertificateDetail$Serial as a String — 'The serial number of the certificate'. Recorded behavior: ACM renders the serial as zero-padded hex BYTES joined by colons — each byte exactly two lowercase hex digits, including the leading zero for a byte below 0x10 (so a serial whose first byte is 0x0a reads '0a:...'). The value itself is the imported certificate's own X.509 serial; only the rendering is the requirement.
Emulator: one unseparated hex run with no colons and no byte padding.
Why: callers that parse or display the serial byte-wise (matching against a CRL, an OpenSSL-printed serial, or a CA's own record) get a value that does not compare equal to the same certificate's serial anywhere else.
Fix: the DescribeCertificate serializer / the certificate record written by the ImportCertificate handler in services/service-com-amazonaws-acm-certificatemanager/src/ops/.
Traces: acm.import-certificate-san-extension-parsed-from-der.

Cause 3 of 10: DescribeCertificate's ExtendedKeyUsages entries omit the OID member.
AWS: Smithy com.amazonaws.acm#ExtendedKeyUsage is a structure with BOTH Name (ExtendedKeyUsageName enum) and OID (String); the OID member's own documentation enumerates the pairing, e.g. '1.3.6.1.5.5.7.3.1 (TLS_WEB_SERVER_AUTHENTICATION)'. ACM parses the X.509 v3 extendedKeyUsage extension out of the imported DER and publishes each purpose as {Name, OID}, with OID the dotted-decimal object identifier that corresponds to Name.
Emulator: the list entry carries Name only — [{'Name': 'TLS_WEB_SERVER_AUTHENTICATION'}] — with no OID key at all.
Why: a missing modeled member is a silent shape divergence; a client that reads the OID (the stable, enum-independent identifier, and the only way to express a purpose ACM reports as CUSTOM) gets nothing to read and cannot handle it as an error either.
Fix: the DescribeCertificate response projection and the DER extendedKeyUsage parsing behind it in the ACM crate.
Traces: acm.list-certificates-key-type-filter-excludes-others.

Cause 4 of 10: ImportCertificate aimed at an existing AMAZON_ISSUED certificate ARN is refused with the wrong error code.
AWS: Smithy models both InvalidParameterException and ResourceNotFoundException (each HTTP 400) for ImportCertificate. Recorded behavior: re-importing into an ACM-MANAGED (AMAZON_ISSUED) ARN answers 400 ResourceNotFoundException with message 'The certificate ARN <arn> is not an imported certificate ARN.' — the fixed text is everything but <arn>, which is the request's own CertificateArn. The refusal is deliberately 'not found' even though the ARN exists and DescribeCertificate resolves it: what is 'not found' is an IMPORTED certificate at that ARN.
Emulator: 400 InvalidParameterException.
Why: error-class-driven clients branch on the code. A caller recycling ARNs treats InvalidParameterException as 'my payload is wrong' and retries with a different payload forever, instead of recognising that the ARN names a managed certificate that import may never overwrite.
Fix: the ImportCertificate handler's ARN/type discrimination in services/service-com-amazonaws-acm-certificatemanager/src/ops/import_certificate.rs.
Traces: acm.acm-reimport-into-an-amazon-issued-certificate-arn-rejected.

Cause 5 of 10: the generic ListTagsForResource / UntagResource refusal for a certificate ARN is worded differently from AWS.
AWS: Smithy models ListTagsForResource(ResourceArn) and UntagResource(ResourceArn, TagKeys), both with ValidationException (400). Recorded behavior: a certificate ARN passed to either generic tagging operation is refused with the operation-specific sentence 'ListTagsForResource only supports non-certificate resource types. Resource <arn> is a certificate resource. Use ListTagsForCertificate API instead.' and 'UntagResource only supports non-certificate resource types. Resource <arn> is a certificate resource. Use RemoveTagsFromCertificate API instead.' The fixed text is every word except <arn>, which is the request's ResourceArn; the named replacement API is the certificate-scoped counterpart of the operation called.
Emulator: '<arn> names a certificate, which this operation does not apply to; use ListTagsForCertificate instead' and '<arn> names a certificate, which this operation does not apply to; use RemoveTagsFromCertificate instead' — a paraphrase that drops the 'only supports non-certificate resource types' clause and the 'API instead' form.
Why: operators and tooling match on this message text to route to the certificate-scoped API; a paraphrase breaks that and makes the refusal indistinguishable from a hand-written proxy's.
Fix: the message construction in the ListTagsForResource and UntagResource handlers in services/service-com-amazonaws-acm-certificatemanager/src/ops/. NOTE: open issue 20261025T052312Z-acm-acme-endpoint-contracts-diverge-deleteacmeendpoint-is-served.md covers a DIFFERENT UntagResource message (its ResourceNotFoundException for an ARN that names nothing); it does not cover this certificate-ARN refusal.
Traces: acm.list-tags-for-resource-rejects-certificate-arn, acm.untag-resource-removes-tag-added-via-addtags.

Cause 6 of 10: RevokeCertificate's refusal for a non-revocable certificate carries an export-related message.
AWS: Smithy models RevokeCertificate(CertificateArn, RevocationReason) with ValidationException (400). Recorded behavior: revoking an IMPORTED certificate (only ACM-issued public/private-CA certificates are revocable) is refused 400 ValidationException with message 'Certificate <arn> is not a public certificate.' — fixed text apart from <arn>, the request's CertificateArn.
Emulator: the code is right (ValidationException) but the message reads 'Export not allowed for certificate <arn>' — the diagnostic of an entirely different refusal class (export eligibility), on an operation that does not export anything.
Why: the message is the only part of the response that says WHY, and this one points the caller at export permissions instead of at the certificate's type; a human or a log-matching rule debugs the wrong thing.
Fix: the RevokeCertificate handler's eligibility check and its message in services/service-com-amazonaws-acm-certificatemanager/src/ops/revoke_certificate.rs (it appears to be reusing the export-eligibility refusal).
Traces: acm.revoke-certificate-ineligible-type-fails, acm.revoke-certificate-unexported-import-fails.

Cause 7 of 10: ResendValidationEmail answers ResourceNotFoundException for a certificate that exists.
AWS: Smithy models ResendValidationEmail(CertificateArn, Domain, ValidationDomain) with InvalidStateException and ResourceNotFoundException (both 400). Recorded behavior: for an existing certificate that is not PENDING_VALIDATION (an ISSUED/IMPORTED one) the refusal is 400 InvalidStateException with message 'Certificate <arn> is not in pending validation state' — fixed text apart from <arn>, the request's CertificateArn. ResourceNotFoundException is reserved for an ARN that names no certificate.
Emulator: 400 ResourceNotFoundException for an ARN that DescribeCertificate resolves in the same run (Status=ISSUED, Type=IMPORTED) — so the handler is not locating the certificate in state at all and collapses the state gate into a not-found.
Why: the two codes mean opposite things to a client. 'Not found' tells a caller its ARN is wrong or the certificate was deleted; 'InvalidState' tells it the certificate is fine but has no validation to nudge. Retry/reconciliation logic takes the wrong branch and can re-request a certificate that already exists.
Fix: the ResendValidationEmail handler's certificate lookup and state gate in services/service-com-amazonaws-acm-certificatemanager/src/ops/resend_validation_email.rs.
Traces: acm.resend-validation-email-state-gated, acm.resend-validation-email-already-issued-rejected.

Cause 8 of 10: UpdateCertificateOptions accepts an IMPORTED certificate instead of refusing it.
AWS: Smithy models UpdateCertificateOptions(CertificateArn required, Options required) -> Unit with InvalidStateException, ValidationException, ResourceNotFoundException, ConflictException, InvalidArnException, LimitExceededException (all 400). CertificateOptions carries only CertificateTransparencyLoggingPreference, which is a property of an ACM-issued public certificate's issuance; an IMPORTED certificate has no ACM-run issuance to configure and the call is refused with a modeled 400. Hypothesis: the specific code is InvalidStateException (the modeled error for a certificate whose state does not admit the change); the recorded check asserts only that a rejection is required, so the code is not pinned by the recording.
Emulator: HTTP 200 with an empty body for update_certificate_options(CertificateArn=<an IMPORTED cert>, Options={'CertificateTransparencyLoggingPreference': 'DISABLED'}).
Why: over-permissive. The client is told its configuration change was applied when ACM would have refused it, and nothing it can later read will ever reflect the setting — a silently lost write rather than a handled error.
Fix: the UpdateCertificateOptions handler's state/type gate in services/service-com-amazonaws-acm-certificatemanager/src/ops/update_certificate_options.rs.
Traces: acm.update-certificate-options-invalid-state-rejected.

Cause 9 of 10: ExportCertificate still answers HTTP 501 NotImplemented.
AWS: Smithy models ExportCertificate(CertificateArn required, Passphrase required @sensitive length 4..128) -> {Certificate, CertificateChain, PrivateKey} with errors InvalidArnException, RequestInProgressException, ResourceNotFoundException, ThrottlingException, ValidationException (all HTTP 400). A well-formed ARN naming no certificate is refused 400 ResourceNotFoundException; an existing but non-exportable certificate (an IMPORTED one) is refused 400 ValidationException.
Emulator: HTTP 501 with Error.Code 'NotImplemented' and message 'operation ExportCertificate is scaffolded but not implemented in this emulator'. services/service-com-amazonaws-acm-certificatemanager/src/ops/export_certificate.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and ends handle() with Err(crate::scaffolded::Scaffolded::not_implemented(..)).
Why: 501 is a server fault to every SDK, so a client cannot distinguish 'this certificate may not be exported' from 'the service is down', and retry logic hammers a call that will never succeed.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/export_certificate.rs (replace the not_implemented Err, delete the SCAFFOLDED marker). Open issue 20261022T143245Z-acm-seven-modeled-operations-answer-http-501-notimplemented.md describes this same stub as its Cause 1 but does NOT carry these two check ids as replay keys; this cause records them so the fix is gated on them too.
Traces: acm.op-contract-export-certificate, acm.e2e-import-export-delete-consequence-flow.

Cause 10 of 10: RenewCertificate still answers HTTP 501 NotImplemented.
AWS: Smithy models RenewCertificate(CertificateArn required) -> Unit with errors InvalidArnException, RequestInProgressException, ResourceNotFoundException, ValidationException (all HTTP 400). Renewing a certificate that is not eligible for ACM-managed renewal is refused with a modeled 400 ValidationException.
Emulator: HTTP 501 Error.Code 'NotImplemented' ('operation RenewCertificate is scaffolded but not implemented in this emulator'); services/service-com-amazonaws-acm-certificatemanager/src/ops/renew_certificate.rs still carries the SCAFFOLDED marker.
Why: renewal automation cannot tell an ineligible certificate from an outage, and backs off against a 5xx that is really a permanent client-side refusal.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/renew_certificate.rs. Same relationship to 20261022T143245Z-acm-seven-modeled-operations-answer-http-501-notimplemented.md (its Cause 4): that issue does not carry this check id.
Traces: acm.renew-certificate-ineligible-fails.

## Evidence
From the behavioral replay (suite simparity-behavioral-ga-prod-v1), ACM 80/151 passed:

Cause 1 — acm.list-certificates-summary-fields-populated: AssertionError: expected KeyAlgorithm='RSA-2048', got a CertificateSummary carrying 'KeyAlgorithm': 'RSA_2048'. acm.import-certificate-ec-key-algorithm-derived: AssertionError on summaries[p256_arn]['KeyAlgorithm'] == 'EC-prime256v1' — "ListCertificates: 'EC_prime256v1'". acm.import-certificate-unsupported-key-size-rejected: AssertionError: "ListCertificates KeyAlgorithm='RSA_4096' disagrees with DescribeCertificate".
Cause 2 — acm.import-certificate-san-extension-parsed-from-der: AssertionError: Serial must be colon-grouped hex, got '742e599b969252572e03acf3155f8da18d0f522a'.
Cause 3 — acm.list-certificates-key-type-filter-excludes-others: AssertionError on describe_certificate(...)['Certificate']['ExtendedKeyUsages'] == [{'Name': 'TLS_WEB_SERVER_AUTHENTICATION', 'OID': '1.3.6.1.5.5.7.3.1'}] — "got [{'Name': 'TLS_WEB_SERVER_AUTHENTICATION'}]".
Cause 4 — acm.acm-reimport-into-an-amazon-issued-certificate-arn-rejected: AssertionError: importing into an ACM-managed (AMAZON_ISSUED) certificate ARN must be refused with 'ResourceNotFoundException'; got 'InvalidParameterException'. (The check's positive control — the identical payload re-imported into an IMPORTED ARN — succeeded, so the payload is not at fault.)
Cause 5 — acm.list-tags-for-resource-rejects-certificate-arn: AssertionError: expected message 'ListTagsForResource only supports non-certificate resource types. Resource <cert-arn> is a certificate resource. Use ListTagsForCertificate API instead.', got '<cert-arn> names a certificate, which this operation does not apply to; use ListTagsForCertificate instead'. acm.untag-resource-removes-tag-added-via-addtags: the same divergence with 'UntagResource' / 'RemoveTagsFromCertificate'.
Cause 6 — acm.revoke-certificate-ineligible-type-fails and acm.revoke-certificate-unexported-import-fails: AssertionError: expected 'Certificate <arn> is not a public certificate.', got 'Export not allowed for certificate <arn>'.
Cause 7 — acm.resend-validation-email-state-gated and acm.resend-validation-email-already-issued-rejected: AssertionError at `assert code == "InvalidStateException"` with the actual code 'ResourceNotFoundException', on an ARN whose DescribeCertificate in the same check returned Status='ISSUED', Type='IMPORTED'. Expected message: 'Certificate <arn> is not in pending validation state'.
Cause 8 — acm.update-certificate-options-invalid-state-rejected: AssertionError: expected rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}}.
Cause 9 — acm.op-contract-export-certificate: AssertionError: expected ResourceNotFoundException, got NotImplemented. acm.e2e-import-export-delete-consequence-flow: AssertionError: ExportCertificate of an IMPORTED cert: expected ValidationException, got 'NotImplemented'.
Cause 10 — acm.renew-certificate-ineligible-fails: AssertionError: expected ValidationException, got 'NotImplemented'.

Source confirmation in the checkout: export_certificate.rs and renew_certificate.rs under services/service-com-amazonaws-acm-certificatemanager/src/ops/ both still carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (us-west-2).
2. Cause 1: mint and acm.import_certificate() an RSA-2048, an RSA-4096 and an EC prime256v1 self-signed leaf; for each ARN compare describe_certificate()['Certificate']['KeyAlgorithm'] with the matching row of list_certificates(Includes={'keyTypes': [...all 7...]})['CertificateSummaryList'] — the summary reports 'RSA_2048'/'RSA_4096'/'EC_prime256v1' while the detail reports the hyphenated form.
3. Cause 2: read describe_certificate()['Certificate']['Serial'] for an imported certificate — it contains no ':'.
4. Cause 3: import a leaf carrying the extendedKeyUsage serverAuth extension and read ['Certificate']['ExtendedKeyUsages'] — entries have Name but no OID.
5. Cause 4: acm.request_certificate() a name you do not control (AMAZON_ISSUED), then acm.import_certificate(CertificateArn=<that managed arn>, Certificate=..., PrivateKey=..., CertificateChain=...) — the code is InvalidParameterException, not ResourceNotFoundException.
6. Cause 5: acm.list_tags_for_resource(ResourceArn=<a certificate arn>) and acm.untag_resource(ResourceArn=<a certificate arn>, TagKeys=['k']) — compare the messages with the AWS wording quoted above.
7. Cause 6: acm.revoke_certificate(CertificateArn=<an imported cert>, RevocationReason='UNSPECIFIED') — ValidationException with the export-flavoured message.
8. Cause 7: acm.resend_validation_email(CertificateArn=<an imported, ISSUED cert>, Domain=<its domain>, ValidationDomain=<its domain>) — ResourceNotFoundException instead of InvalidStateException.
9. Cause 8: acm.update_certificate_options(CertificateArn=<an imported cert>, Options={'CertificateTransparencyLoggingPreference': 'DISABLED'}) — HTTP 200.
10. Cause 9: acm.export_certificate(CertificateArn=<a well-formed nonexistent arn>, Passphrase=b'Sim2RealPass123') and again against an imported certificate — HTTP 501 NotImplemented both times.
11. Cause 10: acm.renew_certificate(CertificateArn=<an imported cert>) — HTTP 501 NotImplemented.
12. Re-replay every check id in trace_ids to confirm.
