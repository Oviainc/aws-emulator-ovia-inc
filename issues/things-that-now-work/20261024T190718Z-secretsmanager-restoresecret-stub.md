Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.concurrent-delete-and-restore-race-deterministic-outcome secretsmanager.deep-secrets-and-params-restore-secret-preserves-the-whole-staging-map-and-every-version secretsmanager.delete-secret-again-while-scheduled-reschedules-and-force-delete-overrides-the-window secretsmanager.delete-secret-schedules-deletion secretsmanager.e2e-secret-create-use-delete-restore-flow secretsmanager.e2e-secret-lifecycle-value-and-tombstone secretsmanager.get-secret-value-scheduled-deletion-fails secretsmanager.list-secrets-filter-and-planned-deletion secretsmanager.put-secret-value-on-deleted-secret-fails secretsmanager.recovery-window-condition-keys-forbid-a-force-delete-and-a-short-window secretsmanager.restore-secret-after-permanent-deletion-rejected secretsmanager.restore-secret-clears-deletion
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secrets-manager|RestoreSecret|NotImplemented
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Implement secretsmanager RestoreSecret: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestoreSecret

## Description
`RestoreSecret` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/restore_secret.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `RestoreSecret` call, so each one unblocks only when `RestoreSecret` works: `secretsmanager.concurrent-delete-and-restore-race-deterministic-outcome`, `secretsmanager.deep-secrets-and-params-restore-secret-preserves-the-whole-staging-map-and-every-version`, `secretsmanager.delete-secret-again-while-scheduled-reschedules-and-force-delete-overrides-the-window`, `secretsmanager.delete-secret-schedules-deletion`, `secretsmanager.e2e-secret-create-use-delete-restore-flow`, `secretsmanager.e2e-secret-lifecycle-value-and-tombstone`, `secretsmanager.get-secret-value-scheduled-deletion-fails`, `secretsmanager.list-secrets-filter-and-planned-deletion`, `secretsmanager.put-secret-value-on-deleted-secret-fails`, `secretsmanager.recovery-window-condition-keys-forbid-a-force-delete-and-a-short-window`, `secretsmanager.restore-secret-after-permanent-deletion-rejected`, `secretsmanager.restore-secret-clears-deletion`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/restore_secret.rs`. Replaying the corpus, the emulator refused the `RestoreSecret` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RestoreSecret operation: operation RestoreSecret is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the secretsmanager emulator
Call RestoreSecret and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
