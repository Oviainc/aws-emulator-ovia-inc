Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.list-certificate-domain-validations-nonexistent-cert-not-found acm.list-certificate-domain-validations-reflects-pending acm.list-certificate-domain-validations-returns-options
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|ListCertificateDomainValidations|NotImplemented
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Implement acm ListCertificateDomainValidations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCertificateDomainValidations

## Description
`ListCertificateDomainValidations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-acm-certificatemanager/src/ops/list_certificate_domain_validations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ListCertificateDomainValidations` call, so each one unblocks only when `ListCertificateDomainValidations` works: `acm.list-certificate-domain-validations-nonexistent-cert-not-found`, `acm.list-certificate-domain-validations-reflects-pending`, `acm.list-certificate-domain-validations-returns-options`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-acm-certificatemanager/src/ops/list_certificate_domain_validations.rs`. Replaying the corpus, the emulator refused the `ListCertificateDomainValidations` call: File "check_acm_list_certificate_domain_validations_nonexistent_cert_not_found.py", line 35, in run
    _assert_succeeds(
    ~~~~~~~~~~~~~~~~^
        lambda: acm.list_certificate_domain_validations(CertificateArn=real_arn),
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        "real certificate ARN",
        ^^^^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "check_acm_list_certificate_domain_validations_nonexistent_cert_not_found.py", line 59, in _assert_succeeds
    raise AssertionError(f"{label} unexpectedly failed: {code}") from err
AssertionError: real certificate ARN unexpectedly failed: NotImplemented

## Steps to Reproduce
Boot the acm emulator
Call ListCertificateDomainValidations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
