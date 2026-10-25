Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-inventory-e2e-configs-with-prefix-lifecycle
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|ListBucketInventoryConfigurations|NotImplemented
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# Implement s3 ListBucketInventoryConfigurations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBucketInventoryConfigurations

## Description
`ListBucketInventoryConfigurations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/list_bucket_inventory_configurations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListBucketInventoryConfigurations` call, so each one unblocks only when `ListBucketInventoryConfigurations` works: `s3.s3-inventory-e2e-configs-with-prefix-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/list_bucket_inventory_configurations.rs`. Replaying the corpus, the emulator refused the `ListBucketInventoryConfigurations` call: File "contextlib.py", line 162, in __exit__
    self.gen.throw(value)
    ~~~~~~~~~~~~~~^^^^^^^
  File "check_s3_s3_inventory_e2e_configs_with_prefix_lifecycle.py", line 276, in phase
    raise _PhaseError(
        f"[phase {i}/{len(self.phases)} {name}] {type(exc).__name__}: {exc}") from exc
_simparity_dyn_check_s3_s3_inventory_e2e_configs_with_prefix_lifecycle_3063cca0._PhaseError: [phase 1/4 deploy] ClientError: An error occurred (NotImplemented) when calling the ListBucketInventoryConfigurations operation: operation ListBucketInventoryConfigurations is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3 emulator
Call ListBucketInventoryConfigurations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
