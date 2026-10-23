Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.acme-domain-validation-create-describe-list acm.acme-domain-validation-delete-then-describe-404 acm.acme-domain-validation-full-lifecycle acm.acme-domain-validation-punycode-domain-persists acm.acme-domain-validation-terminal-removal acm.acme-domain-validation-update-transitions-state acm.concurrent-update-acme-domain-validation-conflict acm.concurrent-update-acme-domain-validation-no-lost-update acm.create-acme-domain-validation-duplicate-conflict acm.create-acme-domain-validation-duplicate-domain-rejected acm.create-acme-domain-validation-duplicate-rejected acm.create-acme-domain-validation-for-unowned-endpoint-denied acm.create-acme-domain-validation-generates-server-managed-dns-record acm.delete-acme-domain-validation-while-validating-succeeds acm.delete-acme-endpoint-cascades-domain-validations acm.delete-acme-endpoint-cascades-through-active-domain-validation acm.delete-acme-endpoint-cascades-to-dependent-removal acm.delete-acme-endpoint-in-use-conflict acm.delete-acme-endpoint-with-validating-domain-validation-cascades acm.describe-nonexistent-acme-domain-validation-rejected acm.duplicate-domain-validation-rejected acm.e2e-acme-endpoint-binding-revoke-flow acm.e2e-requested-certificate-stays-pending-independent-of-acme-endpoint acm.list-acme-domain-validations-excludes-other-endpoint-detail acm.list-acme-domain-validations-reflects-lifecycle acm.update-acme-domain-validation-status-only-server-transitionable acm.update-acme-domain-validation-transitions-status
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|CreateAcmeDomainValidation|NotImplemented
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Implement acm CreateAcmeDomainValidation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAcmeDomainValidation

## Description
`CreateAcmeDomainValidation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-acm-certificatemanager/src/ops/create_acme_domain_validation.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

27 recorded unit(s) fail on their `CreateAcmeDomainValidation` call, so each one unblocks only when `CreateAcmeDomainValidation` works: `acm.acme-domain-validation-create-describe-list`, `acm.acme-domain-validation-delete-then-describe-404`, `acm.acme-domain-validation-full-lifecycle`, `acm.acme-domain-validation-punycode-domain-persists`, `acm.acme-domain-validation-terminal-removal`, `acm.acme-domain-validation-update-transitions-state`, `acm.concurrent-update-acme-domain-validation-conflict`, `acm.concurrent-update-acme-domain-validation-no-lost-update`, `acm.create-acme-domain-validation-duplicate-conflict`, `acm.create-acme-domain-validation-duplicate-domain-rejected`, `acm.create-acme-domain-validation-duplicate-rejected`, `acm.create-acme-domain-validation-for-unowned-endpoint-denied`, `acm.create-acme-domain-validation-generates-server-managed-dns-record`, `acm.delete-acme-domain-validation-while-validating-succeeds`, `acm.delete-acme-endpoint-cascades-domain-validations`, `acm.delete-acme-endpoint-cascades-through-active-domain-validation`, `acm.delete-acme-endpoint-cascades-to-dependent-removal`, `acm.delete-acme-endpoint-in-use-conflict`, `acm.delete-acme-endpoint-with-validating-domain-validation-cascades`, `acm.describe-nonexistent-acme-domain-validation-rejected` (+7 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-acm-certificatemanager/src/ops/create_acme_domain_validation.rs`. Replaying the corpus, the emulator refused the `CreateAcmeDomainValidation` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAcmeDomainValidation operation: operation CreateAcmeDomainValidation is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the acm emulator
Call CreateAcmeDomainValidation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
