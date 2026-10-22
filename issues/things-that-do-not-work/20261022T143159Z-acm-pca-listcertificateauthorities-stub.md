Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.create-ca-algorithm-pairing-rejected-and-classified acm-pca.create-ca-idempotency-token-replays-same-authority acm-pca.create-certificate-authority-invalid-signing-algorithm acm-pca.describe-certificate-authority-malformed-arn acm-pca.list-cas-invalid-next-token-is-not-silent-restart acm-pca.list-certificate-authorities-includes-created acm-pca.list-certificate-authorities-invalid-next-token
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|ListCertificateAuthorities|NotImplemented
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Implement acm-pca ListCertificateAuthorities: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCertificateAuthorities

## Description
`ListCertificateAuthorities` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-acmpca-acmprivateca/src/ops/list_certificate_authorities.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `ListCertificateAuthorities` call, so each one unblocks only when `ListCertificateAuthorities` works: `acm-pca.create-ca-algorithm-pairing-rejected-and-classified`, `acm-pca.create-ca-idempotency-token-replays-same-authority`, `acm-pca.create-certificate-authority-invalid-signing-algorithm`, `acm-pca.describe-certificate-authority-malformed-arn`, `acm-pca.list-cas-invalid-next-token-is-not-silent-restart`, `acm-pca.list-certificate-authorities-includes-created`, `acm-pca.list-certificate-authorities-invalid-next-token`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-acmpca-acmprivateca/src/ops/list_certificate_authorities.rs`. Replaying the corpus, the emulator refused the `ListCertificateAuthorities` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListCertificateAuthorities operation: operation ListCertificateAuthorities is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the acm-pca emulator
Call ListCertificateAuthorities and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
