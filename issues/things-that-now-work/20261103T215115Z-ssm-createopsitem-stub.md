Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.associate-ops-item-related-item-duplicate-rejected ssm.create-opsitem-defaults-to-open-status ssm.e2e-ops-item-related-items-and-event-trail-flow ssm.e2e-opsitem-related-items-events-resolution-flow ssm.get-nonexistent-opsitem-fails ssm.get-ops-summary-aggregates-opsitems ssm.list-ops-item-events-limit-exceeded-rejected ssm.ops-item-delete-and-events-list ssm.ops-item-related-item-association ssm.ops-item-related-items-lifecycle ssm.opsitem-events-list-after-related-item-association ssm.update-opsitem-status-transition
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|CreateOpsItem|NotImplemented
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# Implement ssm CreateOpsItem: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOpsItem

## Description
`CreateOpsItem` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/create_ops_item.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `CreateOpsItem` call, so each one unblocks only when `CreateOpsItem` works: `ssm.associate-ops-item-related-item-duplicate-rejected`, `ssm.create-opsitem-defaults-to-open-status`, `ssm.e2e-ops-item-related-items-and-event-trail-flow`, `ssm.e2e-opsitem-related-items-events-resolution-flow`, `ssm.get-nonexistent-opsitem-fails`, `ssm.get-ops-summary-aggregates-opsitems`, `ssm.list-ops-item-events-limit-exceeded-rejected`, `ssm.ops-item-delete-and-events-list`, `ssm.ops-item-related-item-association`, `ssm.ops-item-related-items-lifecycle`, `ssm.opsitem-events-list-after-related-item-association`, `ssm.update-opsitem-status-transition`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/create_ops_item.rs`. Replaying the corpus, the emulator refused the `CreateOpsItem` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateOpsItem operation: operation CreateOpsItem is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ssm emulator
Call CreateOpsItem and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
