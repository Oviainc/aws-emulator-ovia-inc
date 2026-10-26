Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.acme-replayed-nonce-rejected-and-the-error-carries-a-usable-nonce acm.create-domain-validation-on-revoked-account-blocked acm.e2e-external-account-binding-credential-revocation-flow acm.e2e-external-account-binding-issuance-flow acm.eab-credentials-server-generated-not-client-suppliable acm.eab-hmac-key-base64-round-trip acm.get-acme-external-account-binding-credentials-notfound-after-delete acm.get-credentials-after-revoke-external-account-binding-blocked acm.list-acme-accounts-pagination-server-enforced acm.revoked-eab-credentials-access-rejected
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|GetAcmeExternalAccountBindingCredentials|NotImplemented
Issue-Commit: b17317436abe1a5dae1bd1120020acb35c8cff22

# Implement acm GetAcmeExternalAccountBindingCredentials: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b17317436abe1a5dae1bd1120020acb35c8cff22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAcmeExternalAccountBindingCredentials

## Description
`GetAcmeExternalAccountBindingCredentials` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-acm-certificatemanager/src/ops/get_acme_external_account_binding_credentials.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `GetAcmeExternalAccountBindingCredentials` call, so each one unblocks only when `GetAcmeExternalAccountBindingCredentials` works: `acm.acme-replayed-nonce-rejected-and-the-error-carries-a-usable-nonce`, `acm.create-domain-validation-on-revoked-account-blocked`, `acm.e2e-external-account-binding-credential-revocation-flow`, `acm.e2e-external-account-binding-issuance-flow`, `acm.eab-credentials-server-generated-not-client-suppliable`, `acm.eab-hmac-key-base64-round-trip`, `acm.get-acme-external-account-binding-credentials-notfound-after-delete`, `acm.get-credentials-after-revoke-external-account-binding-blocked`, `acm.list-acme-accounts-pagination-server-enforced`, `acm.revoked-eab-credentials-access-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-acm-certificatemanager/src/ops/get_acme_external_account_binding_credentials.rs`. Replaying the corpus, the emulator refused the `GetAcmeExternalAccountBindingCredentials` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAcmeExternalAccountBindingCredentials operation: operation GetAcmeExternalAccountBindingCredentials is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the acm emulator
Call GetAcmeExternalAccountBindingCredentials and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
