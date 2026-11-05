Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.associate-ops-item-related-item-duplicate-rejected ssm.ops-item-related-item-association ssm.ops-item-related-items-lifecycle ssm.opsitem-events-list-after-related-item-association
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|DisassociateOpsItemRelatedItem|NotImplemented
Issue-Commit: bd1d039abc40ef8a9373a2b76b9b7f1b159abc82

# Implement ssm DisassociateOpsItemRelatedItem: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bd1d039abc40ef8a9373a2b76b9b7f1b159abc82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateOpsItemRelatedItem

## Description
`DisassociateOpsItemRelatedItem` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/disassociate_ops_item_related_item.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `DisassociateOpsItemRelatedItem` call, so each one unblocks only when `DisassociateOpsItemRelatedItem` works: `ssm.associate-ops-item-related-item-duplicate-rejected`, `ssm.ops-item-related-item-association`, `ssm.ops-item-related-items-lifecycle`, `ssm.opsitem-events-list-after-related-item-association`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/disassociate_ops_item_related_item.rs`. Replaying the corpus, the emulator refused the `DisassociateOpsItemRelatedItem` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DisassociateOpsItemRelatedItem operation: operation DisassociateOpsItemRelatedItem is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ssm emulator
Call DisassociateOpsItemRelatedItem and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
