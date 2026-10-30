Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.concurrent-quota-share-updates-are-serialized batch.delete-quota-share-unknown-id-fails batch.list-quota-shares-reflects-create-and-delete batch.list-quota-shares-reflects-created batch.quota-share-delete-then-describe-fails batch.quota-share-lifecycle-create-active-delete batch.quota-share-lifecycle-create-update-delete-404 batch.quota-share-lifecycle-crud batch.quota-share-lifecycle-state-transitions
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|DeleteQuotaShare|NotImplemented
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Implement batch DeleteQuotaShare: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteQuotaShare

## Description
`DeleteQuotaShare` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/delete_quota_share.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `DeleteQuotaShare` call, so each one unblocks only when `DeleteQuotaShare` works: `batch.concurrent-quota-share-updates-are-serialized`, `batch.delete-quota-share-unknown-id-fails`, `batch.list-quota-shares-reflects-create-and-delete`, `batch.list-quota-shares-reflects-created`, `batch.quota-share-delete-then-describe-fails`, `batch.quota-share-lifecycle-create-active-delete`, `batch.quota-share-lifecycle-create-update-delete-404`, `batch.quota-share-lifecycle-crud`, `batch.quota-share-lifecycle-state-transitions`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/delete_quota_share.rs`. Replaying the corpus, the emulator refused the `DeleteQuotaShare` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_batch_concurrent_quota_share_updates_are_serialized.py", line 85, in run
    assert code == "ClientException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ClientException deleting an ENABLED quota share, got NotImplemented

## Steps to Reproduce
Boot the batch emulator
Call DeleteQuotaShare and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
