Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.op-contract-update-consumable-resource
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|UpdateConsumableResource|ClientException
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# batch: UpdateConsumableResource refuses a REMOVE larger than the current total instead of clamping totalQuantity to 0

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConsumableResource

## Description
REOPENS 20261024T120647Z-batch-updateconsumableresource-stub.md: its fix left the call refused.

AWS: Smithy: UpdateConsumableResource (POST /v1/updateconsumableresource) takes a required consumableResource plus operation (String) and quantity (Long), returns consumableResourceName, consumableResourceArn and totalQuantity, and declares only ClientException (httpError 400) and ServerException (500). The model places no bound relating quantity to the resource's stored totalQuantity, so the clamping rule is recorded behavior only: the recording shows that operation="REMOVE" with a quantity GREATER than the resource's current totalQuantity is ACCEPTED with HTTP 200 and clamps totalQuantity to 0 rather than erroring or going negative — the response's totalQuantity is 0 and an independent DescribeConsumableResource reads back 0. The two success paths in the same recording (ADD adds exactly quantity, SET pins totalQuantity to exactly quantity) are served correctly by the emulator; only the over-removal boundary diverges.

Emulator: answers HTTP 400 ClientException with the emulator-authored message "quantity <the submitted quantity> is more than the totalQuantity <the stored total> of <consumableResourceName>". That message is not a wording divergence to be corrected — the whole refusal is one AWS does not make, so there is no AWS text for it. The operation was stood up by the fix for the stub issue named above, and that implementation added a guard AWS does not have.

Why: a client draining a consumable resource with a deliberately over-sized REMOVE (the idiomatic way to zero it, since the current total is racy to read first) gets a 400 instead of a zeroed resource. It must then read totalQuantity and compute an exact REMOVE, which races every concurrent ADD — so the drain either fails repeatedly or leaves a non-zero residue.

Fix: the REMOVE arm of the UpdateConsumableResource handler in the batch crate — drop the over-removal guard and saturate the subtraction at 0, returning the clamped totalQuantity in the response and persisting it.

Traces: batch.op-contract-update-consumable-resource.

## Evidence
Replay of batch.op-contract-update-consumable-resource: CreateConsumableResource(resourceType='REPLENISHABLE', totalQuantity=10) -> DescribeConsumableResource totalQuantity 10 (ok) -> UpdateConsumableResource(operation='ADD', quantity=5) -> totalQuantity 15 (ok) -> UpdateConsumableResource(operation='SET', quantity=100) -> totalQuantity 100 (ok) -> UpdateConsumableResource(operation='REMOVE', quantity=1000) raised botocore.errorfactory.ClientException: An error occurred (ClientException) when calling the UpdateConsumableResource operation: quantity 1000 is more than the totalQuantity 100 of sim2real-batch-cr-<id>.
Expected: HTTP 200 with totalQuantity == 0, and a subsequent DescribeConsumableResource also reporting totalQuantity == 0. The recorded AWS behavior is asserted directly by the check: "a REMOVE larger than the current total succeeds (it does not error) and clamps totalQuantity to 0 rather than going negative".

## Steps to Reproduce
1. CreateConsumableResource(consumableResourceName=<name>, resourceType='REPLENISHABLE', totalQuantity=10).
2. UpdateConsumableResource(consumableResource=<arn>, operation='ADD', quantity=5) — expect totalQuantity 15.
3. UpdateConsumableResource(consumableResource=<arn>, operation='SET', quantity=100) — expect totalQuantity 100.
4. UpdateConsumableResource(consumableResource=<arn>, operation='REMOVE', quantity=1000) — expect HTTP 200 with totalQuantity 0; the emulator raises ClientException instead.
5. DescribeConsumableResource(consumableResource=<arn>) — expect totalQuantity 0.
