Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.conflict-exception-concurrent-state logs.create-delivery-duplicate-conflict-fails logs.create-delivery-duplicate-pairing-is-idempotent logs.create-delivery-lifecycle logs.create-delivery-source-duplicate-name-conflict logs.delete-delivery-source-in-use-conflict-fails logs.delete-delivery-source-in-use-rejected logs.delete-delivery-source-with-active-delivery-rejected logs.delivery-destination-delete-blocked-by-dependent-delivery-rejected logs.delivery-full-chain-lifecycle logs.delivery-full-lifecycle-transitions logs.delivery-lifecycle-create-update-delete logs.delivery-link-create-update-delete-lifecycle logs.delivery-source-and-destination-lifecycle logs.delivery-source-conflict-different-resource-rejected logs.delivery-source-destination-lifecycle logs.delivery-source-lifecycle logs.get-delivery-nonexistent-fails
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.create-delivery-source-duplicate-name-conflict|run#12|AssertionError|assert len(oks) >= 1, f"expected at least one PutDeliverySource racer to succeed, outcomes={outcomes}"
Issue-Related: 20261023T113357Z-cloudwatch-logs-putdeliverysource-stub.md
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement logs PutDeliverySource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliverySource

## Description
`PutDeliverySource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_delivery_source.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

18 recorded unit(s) fail on their `PutDeliverySource` call, so each one unblocks only when `PutDeliverySource` works: `logs.conflict-exception-concurrent-state`, `logs.create-delivery-duplicate-conflict-fails`, `logs.create-delivery-duplicate-pairing-is-idempotent`, `logs.create-delivery-lifecycle`, `logs.create-delivery-source-duplicate-name-conflict`, `logs.delete-delivery-source-in-use-conflict-fails`, `logs.delete-delivery-source-in-use-rejected`, `logs.delete-delivery-source-with-active-delivery-rejected`, `logs.delivery-destination-delete-blocked-by-dependent-delivery-rejected`, `logs.delivery-full-chain-lifecycle`, `logs.delivery-full-lifecycle-transitions`, `logs.delivery-lifecycle-create-update-delete`, `logs.delivery-link-create-update-delete-lifecycle`, `logs.delivery-source-and-destination-lifecycle`, `logs.delivery-source-conflict-different-resource-rejected`, `logs.delivery-source-destination-lifecycle`, `logs.delivery-source-lifecycle`, `logs.get-delivery-nonexistent-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_delivery_source.rs`. Replaying the corpus, the emulator refused the `PutDeliverySource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutDeliverySource operation: operation PutDeliverySource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call PutDeliverySource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
