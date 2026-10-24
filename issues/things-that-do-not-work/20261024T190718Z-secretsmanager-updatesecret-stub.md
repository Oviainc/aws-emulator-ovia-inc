Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.last-changed-date-advances-on-tag-put-and-update-but-not-on-reads secretsmanager.update-secret-client-request-token-replay-is-idempotent-and-reuse-with-a-new-value-rejected secretsmanager.update-secret-creates-new-current-version secretsmanager.update-secret-evicts-a-separately-staged-awspending-label secretsmanager.update-secret-kms-key-change-re-encrypts-so-the-old-key-is-no-longer-needed secretsmanager.update-secret-on-deleted-secret-fails secretsmanager.write-calls-on-a-missing-secret-rejected-and-create-nothing
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secrets-manager|UpdateSecret|NotImplemented
Issue-Key: secretsmanager.write-calls-on-a-missing-secret-rejected-and-create-nothing|run#6|AssertionError|assert got == _NOT_FOUND, f"{op} on a never-created secret returned {got!r}, expected {_NOT_FOUND!r}"
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Implement secretsmanager UpdateSecret: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSecret

## Description
`UpdateSecret` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `UpdateSecret` call, so each one unblocks only when `UpdateSecret` works: `secretsmanager.last-changed-date-advances-on-tag-put-and-update-but-not-on-reads`, `secretsmanager.update-secret-client-request-token-replay-is-idempotent-and-reuse-with-a-new-value-rejected`, `secretsmanager.update-secret-creates-new-current-version`, `secretsmanager.update-secret-evicts-a-separately-staged-awspending-label`, `secretsmanager.update-secret-kms-key-change-re-encrypts-so-the-old-key-is-no-longer-needed`, `secretsmanager.update-secret-on-deleted-secret-fails`, `secretsmanager.write-calls-on-a-missing-secret-rejected-and-create-nothing`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/update_secret.rs`. Replaying the corpus, the emulator refused the `UpdateSecret` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateSecret operation: operation UpdateSecret is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the secretsmanager emulator
Call UpdateSecret and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
