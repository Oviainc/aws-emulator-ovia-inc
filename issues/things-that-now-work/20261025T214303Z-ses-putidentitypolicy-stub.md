Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.delete-identity-policy-nonexistent-policy-is-noop ses.e2e-identity-policy-grant-revoke-flow ses.identity-policy-invalid-json-rejected ses.identity-policy-invalid-policy-rejected ses.identity-policy-json-whitespace-preserved ses.identity-policy-put-get-delete-roundtrip ses.identity-policy-repeated-put-upserts-without-error ses.identity-policy-wildcard-resource-rejected ses.put-identity-policy-malformed-json-rejected ses.put-identity-policy-oversized-document-rejected
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|PutIdentityPolicy|NotImplemented
Issue-Key: ses.identity-policy-invalid-json-rejected|run#10|AssertionError|assert code == "InvalidPolicy", f"expected InvalidPolicy for a malformed policy document, got {code!r}: {msg!r}"
Issue-Key: ses.put-identity-policy-malformed-json-rejected|run#6|AssertionError|assert err == "InvalidPolicy", (
Issue-Key: ses.put-identity-policy-oversized-document-rejected|run#13|AssertionError|assert code == "InvalidPolicy", f"expected InvalidPolicy, got {code!r}: {msg!r}"
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# Implement ses PutIdentityPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIdentityPolicy

## Description
`PutIdentityPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/put_identity_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `PutIdentityPolicy` call, so each one unblocks only when `PutIdentityPolicy` works: `ses.delete-identity-policy-nonexistent-policy-is-noop`, `ses.e2e-identity-policy-grant-revoke-flow`, `ses.identity-policy-invalid-json-rejected`, `ses.identity-policy-invalid-policy-rejected`, `ses.identity-policy-json-whitespace-preserved`, `ses.identity-policy-put-get-delete-roundtrip`, `ses.identity-policy-repeated-put-upserts-without-error`, `ses.identity-policy-wildcard-resource-rejected`, `ses.put-identity-policy-malformed-json-rejected`, `ses.put-identity-policy-oversized-document-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/put_identity_policy.rs`. Replaying the corpus, the emulator refused the `PutIdentityPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutIdentityPolicy operation: operation PutIdentityPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ses emulator
Call PutIdentityPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
