Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.concurrent-consumable-resource-updates-are-atomic
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.concurrent-consumable-resource-updates-are-atomic|run#7|AssertionError|assert len(oks) == n, f"expected all {n} concurrent ADD updates to succeed, got {len(oks)}: {errs}"
Issue-Commit: a577dca75911a15e8d14eba87791fcef54f4bfc0

# batch UpdateConsumableResource answers NotImplemented under concurrent ADD updates, so consumable-resource quantity accounting is not atomic (or even served)

> _Note: this issue was identified against the code base at commit `a577dca75911a15e8d14eba87791fcef54f4bfc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConsumableResource

## Description
Cause 1 of 1: UpdateConsumableResource is not served, so concurrent ADD updates to a consumable resource's quantity all fail and the resource's totalQuantity never advances.

Note on related filings: the open issue 20261024T120647Z-batch-updateconsumableresource-stub.md records that the operation is an unimplemented scaffold stub, and the replay report attributed only one check (batch.consumable-resource-add-remove-arithmetic) to it. The concurrency check below was NOT marked ALREADY FILED or OWNED by the report, and it carries a requirement beyond mere implementation — read-modify-write atomicity of the quantity under contention — so it is filed here and should be re-replayed after that stub is implemented.

AWS: Smithy (com-amazonaws-batch-awsbatchv20160810) models UpdateConsumableResource as POST /v1/updateconsumableresource with success code 200; input members consumableResource (required), operation, quantity (Long), clientToken (idempotency token, @length 1..64); output consumableResourceName (required), consumableResourceArn (required), totalQuantity (Long). Its only modeled errors are ClientException (400) and ServerException (500) — there is no modeled refusal for a well-formed ADD update against an existing REPLENISHABLE resource, so AWS answers 200 with the updated totalQuantity. The quantity mutation is a read-modify-write on server-side state: N successful ADD updates of q against a resource created with totalQuantity 0 must leave DescribeConsumableResource reporting totalQuantity == N*q, with no lost updates when the calls overlap (recorded behavior for the concurrency axis: 10 parallel ADDs of 3 settle at 30).

Emulator: every one of 10 parallel UpdateConsumableResource ADD calls is refused with error code NotImplemented ("operation UpdateConsumableResource is scaffolded but not implemented in this emulator"), i.e. 0 of 10 succeeded. The check fails before it can even observe the totalQuantity invariant, so the emulator additionally has no demonstrated atomicity for the quantity update path.

Why: a client that uses consumable resources to gate job dispatch cannot replenish or consume capacity at all — a modeled 200 response becomes a hard non-AWS NotImplemented refusal, breaking every workflow that adjusts a consumable resource. Once implemented, a non-atomic read-modify-write would be just as damaging in a different way: concurrent ADDs would silently lose updates, and the resource's totalQuantity/availableQuantity would under-report capacity, causing jobs to be starved or over-admitted with no error surfaced.

Fix: the UpdateConsumableResource handler for the batch service (services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/update_consumable_resource.rs) plus the consumable-resource state component it mutates — the ADD/REMOVE arithmetic must be applied under a single critical section per resource so overlapping updates serialize rather than read a stale quantity, and DescribeConsumableResource must report the resulting totalQuantity.

Traces: failing check batch.concurrent-consumable-resource-updates-are-atomic (10 parallel ADD updates of 3 against a REPLENISHABLE resource created with totalQuantity 0; expected all 10 to return 200 and totalQuantity to settle at 30, got 10x NotImplemented).

## Evidence
Check: batch.concurrent-consumable-resource-updates-are-atomic [mismatch]

Expected (AWS / Smithy): UpdateConsumableResource -> HTTP 200 with {consumableResourceName, consumableResourceArn, totalQuantity}; after 10 concurrent ADD updates of quantity=3 against a resource created with totalQuantity=0, DescribeConsumableResource reports totalQuantity == 30. Modeled errors are only ClientException (400) and ServerException (500).

Actual (emulator): all 10 concurrent calls raise error code NotImplemented:
  AssertionError: expected all 10 concurrent ADD updates to succeed, got 0: ['NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented']
(message text quoted as an example; the requirement is the status/code: a modeled 200 instead of a NotImplemented refusal, and no lost updates in the resulting totalQuantity.)

The totalQuantity atomicity assertion was never reached, so atomicity of the quantity read-modify-write is unverified in addition to the operation being unserved.

## Steps to Reproduce
1. Boot the emulator and point a batch client at it.
2. batch.create_consumable_resource(consumableResourceName=<unique>, totalQuantity=0, resourceType="REPLENISHABLE").
3. Issue 10 concurrent batch.update_consumable_resource(consumableResource=<name>, operation="ADD", quantity=3) calls from a thread pool.
4. Observe every call is refused with error code NotImplemented instead of returning HTTP 200 with the updated totalQuantity.
5. After the operation is implemented, re-run and assert batch.describe_consumable_resource(consumableResource=<name>)["totalQuantity"] == 30 (no lost updates under contention).

Replay: run_behavioral_suite and re-check batch.concurrent-consumable-resource-updates-are-atomic.
