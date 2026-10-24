Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.create-acme-domain-validation-malformed-domain-rejected acm.domain-validation-optional-field-empty-vs-absent-preserved acm.list-acme-domain-validations-nonexistent-endpoint
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|ListAcmeDomainValidations|NotImplemented
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Implement acm ListAcmeDomainValidations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAcmeDomainValidations

## Description
`ListAcmeDomainValidations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-acm-certificatemanager/src/ops/list_acme_domain_validations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ListAcmeDomainValidations` call, so each one unblocks only when `ListAcmeDomainValidations` works: `acm.create-acme-domain-validation-malformed-domain-rejected`, `acm.domain-validation-optional-field-empty-vs-absent-preserved`, `acm.list-acme-domain-validations-nonexistent-endpoint`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-acm-certificatemanager/src/ops/list_acme_domain_validations.rs`. Replaying the corpus, the emulator refused the `ListAcmeDomainValidations` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListAcmeDomainValidations operation: operation ListAcmeDomainValidations is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the acm emulator
Call ListAcmeDomainValidations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
