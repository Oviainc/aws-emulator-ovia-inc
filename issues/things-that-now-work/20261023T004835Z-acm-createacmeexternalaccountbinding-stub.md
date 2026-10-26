Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.acme-account-describe-list acm.acme-account-double-revoke-is-idempotent acm.acme-account-identity-is-the-account-key-not-the-binding acm.acme-account-revoke-lifecycle acm.acme-account-revoke-terminal-transition acm.acme-account-revoke-transitions-state acm.acme-contact-required-endpoint-refuses-a-registration-without-contact acm.acme-external-account-binding-key-base64-round-trip acm.acme-external-account-binding-lifecycle acm.acme-new-account-with-a-forged-eab-mac-rejected acm.acme-replayed-nonce-rejected-and-the-error-carries-a-usable-nonce acm.concurrent-create-acme-external-account-binding-same-key-single-winner acm.concurrent-create-external-account-binding-duplicate-name-conflict acm.concurrent-revoke-acme-account-idempotent-terminal acm.concurrent-revoke-acme-account-single-winner acm.create-acme-external-account-binding-duplicate-key-conflict acm.create-acme-external-account-binding-unassumable-role-rejected acm.create-domain-validation-on-revoked-account-blocked acm.describe-acme-account-revoked-status-not-notfound acm.describe-nonexistent-acme-external-account-binding-rejected acm.e2e-external-account-binding-credential-revocation-flow acm.e2e-external-account-binding-issuance-flow acm.eab-create-describe-list-credentials acm.eab-credentials-are-server-generated-immutable acm.eab-credentials-server-generated-not-client-suppliable acm.eab-delete-then-describe-404 acm.eab-hmac-key-base64-round-trip acm.eab-revoke-already-deleted-conflicts acm.external-account-binding-credentials-server-generated acm.get-acme-external-account-binding-credentials-notfound-after-delete acm.get-acme-external-account-binding-credentials-server-generated-hmac acm.get-credentials-after-revoke-external-account-binding-blocked acm.get-eab-credentials-after-revoke-not-found acm.list-acme-accounts-pagination-server-enforced acm.revoke-acme-account-sets-terminal-state acm.revoke-already-revoked-acme-account-is-idempotent acm.revoke-external-account-binding-twice-preserves-revoked-at acm.revoked-eab-credentials-access-rejected
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|CreateAcmeExternalAccountBinding|NotImplemented
Issue-Key: acm.revoke-acme-account-sets-terminal-state|_create_eab_binding_with_retry#6|AssertionError|raise AssertionError(
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Implement acm CreateAcmeExternalAccountBinding: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAcmeExternalAccountBinding

## Description
`CreateAcmeExternalAccountBinding` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-acm-certificatemanager/src/ops/create_acme_external_account_binding.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

38 recorded unit(s) fail on their `CreateAcmeExternalAccountBinding` call, so each one unblocks only when `CreateAcmeExternalAccountBinding` works: `acm.acme-account-describe-list`, `acm.acme-account-double-revoke-is-idempotent`, `acm.acme-account-identity-is-the-account-key-not-the-binding`, `acm.acme-account-revoke-lifecycle`, `acm.acme-account-revoke-terminal-transition`, `acm.acme-account-revoke-transitions-state`, `acm.acme-contact-required-endpoint-refuses-a-registration-without-contact`, `acm.acme-external-account-binding-key-base64-round-trip`, `acm.acme-external-account-binding-lifecycle`, `acm.acme-new-account-with-a-forged-eab-mac-rejected`, `acm.acme-replayed-nonce-rejected-and-the-error-carries-a-usable-nonce`, `acm.concurrent-create-acme-external-account-binding-same-key-single-winner`, `acm.concurrent-create-external-account-binding-duplicate-name-conflict`, `acm.concurrent-revoke-acme-account-idempotent-terminal`, `acm.concurrent-revoke-acme-account-single-winner`, `acm.create-acme-external-account-binding-duplicate-key-conflict`, `acm.create-acme-external-account-binding-unassumable-role-rejected`, `acm.create-domain-validation-on-revoked-account-blocked`, `acm.describe-acme-account-revoked-status-not-notfound`, `acm.describe-nonexistent-acme-external-account-binding-rejected` (+18 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-acm-certificatemanager/src/ops/create_acme_external_account_binding.rs`. Replaying the corpus, the emulator refused the `CreateAcmeExternalAccountBinding` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAcmeExternalAccountBinding operation: operation CreateAcmeExternalAccountBinding is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the acm emulator
Call CreateAcmeExternalAccountBinding and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
