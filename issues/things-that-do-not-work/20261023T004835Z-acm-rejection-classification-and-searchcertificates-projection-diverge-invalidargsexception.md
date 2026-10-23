Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.list-certificate-domain-validations-invalid-args-rejected acm.list-certificate-domain-validations-malformed-arn-rejected acm.op-contract-update-certificate-options acm.search-certificates-invalid-filter-rejected acm.search-certificates-filters-by-status-server-side
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.list-certificate-domain-validations-invalid-args-rejected|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: acm.list-certificate-domain-validations-malformed-arn-rejected|run#14|AssertionError|assert bad_token_code == "ValidationException", f"expected ValidationException, got {bad_token_code!r}"
Issue-Key: acm.op-contract-update-certificate-options|run#10|AssertionError|assert code_validation_method == "ValidationException", (
Issue-Key: acm.search-certificates-invalid-filter-rejected|run#11|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: acm.search-certificates-filters-by-status-server-side|run#14|AssertionError|assert ec_only[0]["X509Attributes"]["KeyAlgorithm"] == "EC_prime256v1"
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# ACM rejection classification and SearchCertificates projection diverge: InvalidArgsException/InvalidStateException for ValidationException, generic union message, wrong KeyAlgorithm (4 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCertificateDomainValidations

## Description
Four distinct root causes, all in operations that DO answer (no stubs involved): three mis-classify a refusal and one mis-projects a search result. None is covered by an open issue — 20261022T143245Z-acm-certificate-representation-dns-validation-and-error-message.md covers the ValidationException MESSAGE envelope (Cause 7 there) and the certificate projection of DescribeCertificate, not the error CODE chosen by these operations nor SearchCertificates results.

Cause 1 of 4: ListCertificateDomainValidations answers InvalidArgsException where AWS answers ValidationException.
AWS: recorded behavior — a NextToken the service never issued is refused with HTTP 400, Error.Code 'ValidationException', message 'Invalid next token.'; a malformed CertificateArn is likewise refused with 400 ValidationException. (Smithy note: the operation's modeled error list is AccessDeniedException/InvalidArgsException/ResourceNotFoundException/ThrottlingException and does NOT list ValidationException, so the model alone would predict the emulator's choice; the recorded AWS responses are the requirement, and the service shape ValidationException exists in the same model.)
Emulator: HTTP 400 with Error.Code 'InvalidArgsException' for both the bogus pagination token and the malformed ARN.
Why: the error code is the part clients branch on. An SDK retry/except clause written for ACM (`except acm.exceptions.ValidationException`) never matches, so a bad pagination token surfaces as an unhandled error class; the two codes are also modeled as different exception types, so typed handlers are mutually exclusive.
Fix: the refusal classification in services/service-com-amazonaws-acm-certificatemanager/src/ops/list_certificate_domain_validations.rs (and the shared input/ARN and pagination-token validation it calls) — raise ValidationException for a malformed ARN and an unissued NextToken.
Traces: acm.list-certificate-domain-validations-invalid-args-rejected, acm.list-certificate-domain-validations-malformed-arn-rejected.

Cause 2 of 4: UpdateCertificateOptions answers InvalidStateException where AWS answers ValidationException.
AWS: Smithy UpdateCertificateOptions(CertificateArn required, Options required) -> Unit models BOTH InvalidStateException and ValidationException (each HTTP 400). Recorded behavior — changing Options on a certificate that is still PENDING_VALIDATION is refused with 400 ValidationException, message 'Certificate is not issued' (and, for Options.Export, 'Export option for certificates cannot be updated'); the certificate's status and options are left unchanged.
Emulator: 400 with Error.Code 'InvalidStateException' for update_certificate_options(CertificateArn=<PENDING_VALIDATION cert>, Options={'ValidationMethod': 'DNS'}).
Why: both codes are modeled, so a client can and does distinguish them — one is 'your request is invalid for this certificate', the other 'retry when the resource settles'. Clients that retry on InvalidStateException will loop forever against the emulator on a request AWS refuses outright.
Fix: the state gate in services/service-com-amazonaws-acm-certificatemanager/src/ops/update_certificate_options.rs — classify the not-yet-issued case as ValidationException.
Traces: acm.op-contract-update-certificate-options.

Cause 3 of 4: SearchCertificates refuses a multi-member CertificateFilterStatement with a generic deserializer message instead of ACM's member-naming message.
AWS: Smithy SearchCertificates(FilterStatement: CertificateFilterStatement union) models ValidationException (400). Recorded behavior — setting more than one union member is refused with 400 ValidationException whose message is built from the request and names the offending members: 'Each CertificateFilterStatement must contain exactly one member. A CertificateFilterStatement in your request contains the members [And, Or].' The rule (name the union, state the exactly-one constraint, list the members actually present, in request order) is the requirement, not the recorded sample text.
Emulator: status 400 and Error.Code 'ValidationException' are correct, but the message is the internal deserializer's text: "union 'CertificateFilterStatement' had more than one member set" — it names no member and is not built from the request.
Why: the message is the only place the offending members are reported for a union, so a caller debugging a generated filter cannot tell WHICH members collided; tooling that surfaces ACM's message to users shows an internal deserializer string.
Fix: validate the FilterStatement union in services/service-com-amazonaws-acm-certificatemanager/src/ops/search_certificates.rs (or the union deserializer's error mapping for this service) and build the message from the members present in the request.
Traces: acm.search-certificates-invalid-filter-rejected.

Cause 4 of 4: SearchCertificates results carry the wrong X509Attributes.KeyAlgorithm.
AWS: Smithy CertificateSearchResult{CertificateArn, X509Attributes, CertificateMetadata}; X509Attributes.KeyAlgorithm targets the KeyAlgorithm enum. Recorded behavior — for a certificate requested with KeyAlgorithm='EC_prime256v1' the result's X509Attributes.KeyAlgorithm is 'EC_prime256v1' (and 'RSA_2048' for the RSA one), i.e. the projection reflects the key algorithm the certificate was requested with, consistent with the X509AttributeFilter.KeyAlgorithm that selected it.
Emulator: the filter leg works (exactly one result, the expected ARN), but ec_only[0]['X509Attributes']['KeyAlgorithm'] != 'EC_prime256v1' — the member is present with some other value, so a result selected by KeyAlgorithm='EC_prime256v1' does not report that algorithm.
Why: the result set and the attribute disagree, so a client that re-checks the attribute it filtered on (or renders it in an inventory) sees the wrong algorithm for every certificate and cannot trust the projection.
Fix: the CertificateSearchResult/X509Attributes projection in services/service-com-amazonaws-acm-certificatemanager/src/ops/search_certificates.rs — carry the certificate's key algorithm from certificate state into X509Attributes.KeyAlgorithm in the enum form AWS returns.
Traces: acm.search-certificates-filters-by-status-server-side.

## Evidence
Cause 1: acm.list-certificate-domain-validations-invalid-args-rejected — AssertionError: expected ValidationException, got InvalidArgsException, on acm.list_certificate_domain_validations(CertificateArn=<fresh cert>, NextToken='not-a-real-pagination-token') (expected message 'Invalid next token.'); acm.list-certificate-domain-validations-malformed-arn-rejected — AssertionError: expected ValidationException, got 'InvalidArgsException'.
Cause 2: acm.op-contract-update-certificate-options — AssertionError: expected ValidationException, got InvalidStateException, on acm.update_certificate_options(CertificateArn=<PENDING_VALIDATION cert>, Options={'ValidationMethod': 'DNS'}) (expected message 'Certificate is not issued').
Cause 3: acm.search-certificates-invalid-filter-rejected — AssertionError: expected 'Each CertificateFilterStatement must contain exactly one member. A CertificateFilterStatement in your request contains the members [And, Or].', got "union 'CertificateFilterStatement' had more than one member set"; the preceding assertion on code == 'ValidationException' passed. The request sets both And and Or with client-side parameter validation disabled, so the refusal is the service's.
Cause 4: acm.search-certificates-filters-by-status-server-side — AssertionError at `assert ec_only[0]['X509Attributes']['KeyAlgorithm'] == 'EC_prime256v1'`, after `len(ec_only) == 1` and `ec_only[0]['CertificateArn'] == arn_ec` both passed (the certificate was requested with KeyAlgorithm='EC_prime256v1' and selected by {'Filter': {'X509AttributeFilter': {'KeyAlgorithm': 'EC_prime256v1'}}}).
Smithy citations: ListCertificateDomainValidations errors [AccessDeniedException(403), InvalidArgsException(400), ResourceNotFoundException(400), ThrottlingException(400)]; UpdateCertificateOptions errors [ConflictException, InvalidArnException, InvalidStateException, LimitExceededException, ResourceNotFoundException, ValidationException] all 400; SearchCertificates errors [AccessDeniedException(403), ThrottlingException(400), ValidationException(400)], output Results: CertificateSearchResultList; X509Attributes.KeyAlgorithm: KeyAlgorithm enum.

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Cause 1: acm.request_certificate(DomainName=<d>), then acm.list_certificate_domain_validations(CertificateArn=<arn>, NextToken='not-a-real-pagination-token') — Error.Code is 'InvalidArgsException', expected 'ValidationException' / 'Invalid next token.'; repeat with a malformed CertificateArn.
3. Cause 2: request an EMAIL certificate, confirm Status=PENDING_VALIDATION, then acm.update_certificate_options(CertificateArn=<arn>, Options={'ValidationMethod': 'DNS'}) — Error.Code 'InvalidStateException', expected 'ValidationException'.
4. Cause 3: with botocore Config(parameter_validation=False), acm.search_certificates(FilterStatement={'And': [{'Filter': {'CertificateArn': <arn>}}], 'Or': [{'Filter': {'CertificateArn': <arn>}}]}) — correct 400/ValidationException but the message names no member.
5. Cause 4: request two certificates with KeyAlgorithm RSA_2048 and EC_prime256v1, search with {'And': [<arn scope>, {'Filter': {'X509AttributeFilter': {'KeyAlgorithm': 'EC_prime256v1'}}}]} and read Results[0].X509Attributes.KeyAlgorithm — not 'EC_prime256v1'.
6. Re-replay the five checks in trace_ids.
