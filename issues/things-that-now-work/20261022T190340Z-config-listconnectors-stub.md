Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.concurrent-put-connector-name-conflict config.concurrent-put-connector-name-race config.concurrent-put-connector-name-race-single-winner config.connector-lifecycle-create-active-delete config.connector-lifecycle-create-get-delete-404 config.connector-listing-reflects-created-and-deleted-connectors config.connector-missing-provider-config-rejected config.connector-put-nonexistent-azure-tenant-rejected config.delete-connector-nonexistent-rejected config.duplicate-connector-configuration-rejected config.e2e-connector-malformed-arn-rejected config.e2e-connector-write-read-delete-flow config.list-connectors-pagination-completeness config.put-connector-duplicate-configuration-conflict-rejected config.put-connector-duplicate-name-conflict-rejected config.put-connector-max-number-exceeded-blocked
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config-service|ListConnectors|NotImplemented
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Implement config ListConnectors: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListConnectors

## Description
`ListConnectors` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/list_connectors.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

16 recorded unit(s) fail on their `ListConnectors` call, so each one unblocks only when `ListConnectors` works: `config.concurrent-put-connector-name-conflict`, `config.concurrent-put-connector-name-race`, `config.concurrent-put-connector-name-race-single-winner`, `config.connector-lifecycle-create-active-delete`, `config.connector-lifecycle-create-get-delete-404`, `config.connector-listing-reflects-created-and-deleted-connectors`, `config.connector-missing-provider-config-rejected`, `config.connector-put-nonexistent-azure-tenant-rejected`, `config.delete-connector-nonexistent-rejected`, `config.duplicate-connector-configuration-rejected`, `config.e2e-connector-malformed-arn-rejected`, `config.e2e-connector-write-read-delete-flow`, `config.list-connectors-pagination-completeness`, `config.put-connector-duplicate-configuration-conflict-rejected`, `config.put-connector-duplicate-name-conflict-rejected`, `config.put-connector-max-number-exceeded-blocked`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/list_connectors.rs`. Replaying the corpus, the emulator refused the `ListConnectors` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListConnectors operation: operation ListConnectors is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the config emulator
Call ListConnectors and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
