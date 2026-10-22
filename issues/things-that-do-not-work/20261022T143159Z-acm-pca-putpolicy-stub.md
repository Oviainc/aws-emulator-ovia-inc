Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm-pca.concurrent-delete-policy-race-conflict acm-pca.delete-policy-malformed-arn-rejected acm-pca.delete-policy-wrong-ca-state-fails acm-pca.put-policy-document-round-trip acm-pca.put-policy-invalid-principal acm-pca.put-policy-missing-permission-denied
Issue-Shape-Id: com.amazonaws.acmpca#ACMPrivateCA
Issue-Key: acm-pca|PutPolicy|NotImplemented
Issue-Key: acm-pca.put-policy-invalid-principal|run#6|AssertionError|assert refusal == ("InvalidPolicyException", "InvalidPolicy: Principal not found."), refusal
Issue-Key: acm-pca.put-policy-missing-permission-denied|run#21|AssertionError|assert refusal == (
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Implement acm-pca PutPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutPolicy

## Description
`PutPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-acmpca-acmprivateca/src/ops/put_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `PutPolicy` call, so each one unblocks only when `PutPolicy` works: `acm-pca.concurrent-delete-policy-race-conflict`, `acm-pca.delete-policy-malformed-arn-rejected`, `acm-pca.delete-policy-wrong-ca-state-fails`, `acm-pca.put-policy-document-round-trip`, `acm-pca.put-policy-invalid-principal`, `acm-pca.put-policy-missing-permission-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-acmpca-acmprivateca/src/ops/put_policy.rs`. Replaying the corpus, the emulator refused the `PutPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutPolicy operation: operation PutPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the acm-pca emulator
Call PutPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
