Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.list-quota-shares-invalid-filter-combination-fails batch.list-quota-shares-pagination-is-consistent batch.list-quota-shares-reflects-created-shares
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|ListQuotaShares|NotImplemented
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement batch ListQuotaShares: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListQuotaShares

## Description
`ListQuotaShares` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/list_quota_shares.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ListQuotaShares` call, so each one unblocks only when `ListQuotaShares` works: `batch.list-quota-shares-invalid-filter-combination-fails`, `batch.list-quota-shares-pagination-is-consistent`, `batch.list-quota-shares-reflects-created-shares`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/list_quota_shares.rs`. Replaying the corpus, the emulator refused the `ListQuotaShares` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListQuotaShares operation: operation ListQuotaShares is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the batch emulator
Call ListQuotaShares and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
