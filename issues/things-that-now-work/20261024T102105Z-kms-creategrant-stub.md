Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.concurrent-create-grant-idempotent-tokens kms.concurrent-grant-creation-no-duplicate kms.concurrent-grant-revoke-double-revoke-rejected kms.create-grant-concurrent-same-params-distinct-grants kms.create-grant-dry-run-disabled-key-still-rejected kms.create-grant-dry-run-does-not-persist kms.create-grant-exceeding-key-policy-permissions-rejected kms.create-list-grant kms.e2e-grant-based-access-then-retire-blocks-further-use kms.grant-constraint-mismatched-context-rejected kms.grant-constraints-encryption-context-subset-round-trip kms.grant-constraints-server-enforced-on-encrypt kms.list-retirable-grants-pagination-marker-order kms.op-unknown-grant-id-rejected kms.retire-grant-and-list-retirable kms.retired-grant-cannot-be-used kms.sep-kms-key-scope-grant-context-equals-matches-reordered-context-but-absent-context-denied kms.sep-kms-key-scope-grant-on-one-key-does-not-authorize-another-key-denied
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|CreateGrant|NotImplemented
Issue-Key: kms.create-grant-dry-run-disabled-key-still-rejected|run#8|AssertionError|assert code_enabled == ("DryRunOperationException", MSG_DRY_RUN), code_enabled
Issue-Key: kms.create-grant-dry-run-does-not-persist|run#8|AssertionError|assert code == ("DryRunOperationException", MSG_DRY_RUN), (
Issue-Key: kms.create-grant-exceeding-key-policy-permissions-rejected|run#20|AssertionError|assert got == expected, (
Issue-Key: kms.e2e-grant-based-access-then-retire-blocks-further-use|_create_grant_with_retry#7|AssertionError|raise AssertionError(f"create_grant never succeeded for {role_arn}: {last_err}")
Issue-Key: kms.grant-constraint-mismatched-context-rejected|_create_grant_with_retry#8|AssertionError|raise AssertionError(f"create_grant never succeeded for {role_arn}: {last_err}")
Issue-Key: kms.grant-constraints-server-enforced-on-encrypt|_create_grant_with_retry#8|AssertionError|raise AssertionError(f"create_grant never succeeded for {role_arn}: {last_err}")
Issue-Key: kms.retire-grant-and-list-retirable|_create_grant_with_retry#7|AssertionError|raise AssertionError(f"create_grant never succeeded for grantee {grantee}: {last_err}")
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# Implement kms CreateGrant: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGrant

## Description
`CreateGrant` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kms-trentservice/src/ops/create_grant.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

18 recorded unit(s) fail on their `CreateGrant` call, so each one unblocks only when `CreateGrant` works: `kms.concurrent-create-grant-idempotent-tokens`, `kms.concurrent-grant-creation-no-duplicate`, `kms.concurrent-grant-revoke-double-revoke-rejected`, `kms.create-grant-concurrent-same-params-distinct-grants`, `kms.create-grant-dry-run-disabled-key-still-rejected`, `kms.create-grant-dry-run-does-not-persist`, `kms.create-grant-exceeding-key-policy-permissions-rejected`, `kms.create-list-grant`, `kms.e2e-grant-based-access-then-retire-blocks-further-use`, `kms.grant-constraint-mismatched-context-rejected`, `kms.grant-constraints-encryption-context-subset-round-trip`, `kms.grant-constraints-server-enforced-on-encrypt`, `kms.list-retirable-grants-pagination-marker-order`, `kms.op-unknown-grant-id-rejected`, `kms.retire-grant-and-list-retirable`, `kms.retired-grant-cannot-be-used`, `kms.sep-kms-key-scope-grant-context-equals-matches-reordered-context-but-absent-context-denied`, `kms.sep-kms-key-scope-grant-on-one-key-does-not-authorize-another-key-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kms-trentservice/src/ops/create_grant.rs`. Replaying the corpus, the emulator refused the `CreateGrant` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateGrant operation: operation CreateGrant is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kms emulator
Call CreateGrant and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
