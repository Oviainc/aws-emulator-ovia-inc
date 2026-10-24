Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7bcb53c96ea0ea6c32faf644dec1ae2940515247

# StartCapacityTask accepts an OrderId and an AssetId that name no order and no asset, and stores the dangling reference on the task

_Filed: 2026-10-24T13:42:49Z_
_Operation: StartCapacityTask | Class: validation_

> _Note: this issue was identified against the code base at commit `7bcb53c96ea0ea6c32faf644dec1ae2940515247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartCapacityTask` resolves only its `OutpostIdentifier`. The optional `OrderId` ("The ID of the Amazon Web Services Outposts order associated with the specified capacity task") and `AssetId` ("The ID of the Outpost asset. An Outpost asset can be a single server within an Outposts rack or an Outposts server configuration") are taken as free-form strings: `OrderId` is used only as a key for the one-active-task-per-order rule and `AssetId` is copied straight onto the stored task, with neither value ever resolved against anything the service holds.

Expected (real Outposts): a capacity task is started for an order the account actually placed, or against an asset actually installed on the named Outpost. An `OrderId` or `AssetId` that names nothing is refused with the modelled `NotFoundException` (HTTP 404) — the operation declares that error precisely so these references can be refused — and no task is created.

Actual: HTTP 200, a `cap-…` task is minted, persisted and echoed back carrying `OrderId`/`AssetId` values that resolve to nothing; the task then runs its clock-owed progression to `COMPLETED`, so `GetCapacityTask` reports a completed capacity task against a non-existent order/asset. Note the same crate refuses these very identifiers elsewhere: `GetOutpostSupportedInstanceTypes` answers `NotFoundException` with `orderId=<value>` for ANY `OrderId` (no order is ever persisted) and with `assetId=<value>` for an `AssetId` that is not one of the Outpost's assets (`src/ops/get_outpost_supported_instance_types.rs` lines ~239-270), so the two operations disagree about whether these references must exist.

## Evidence

Spec: Smithy model `com.amazonaws.outposts#StartCapacityTask` declares `OrderId` (`^oo-[a-f0-9]{17}$`) and `AssetId` (`^\d{10}$`) as inputs and `NotFoundException` (HTTP 404) among its errors; the member docs (smithy_query doc StartCapacityTaskInput/OrderId and /AssetId) tie each to an existing Outposts order and to an asset of the Outpost. Well-established AWS behaviour: an API that takes the id of another resource resolves it and refuses a dangling reference rather than persisting it.

Handler: `src/ops/start_capacity_task.rs` lines ~221-332 — after `resolve_outpost_id`, `order_id` is only passed to `store.active_capacity_task_on_order(&order_id)` and `asset_id: crate::state::non_empty(input.asset_id)` is stored verbatim; nothing consults `store.outposts[..].assets` and the store holds no orders at all.

Tests: `tests/all/capacity_task_lifecycle.rs` lines 56-61 and 220-310 deliberately pin the current accepting behaviour (`const ORDER_ID: &str = "oo-1111111111111111a"` with the comment "Orders are not persisted by this slice", plus assertions that `OrderId`/`AssetId` are echoed back), so no test asserts the refusal.

## Steps to Reproduce

1. CreateSite, then CreateOutpost against it; keep the returned OutpostId.
2. POST /outposts/{OutpostId}/capacity with {"InstancePools":[{"InstanceType":"m5.4xlarge","Count":1}],"OrderId":"oo-1111111111111111a","AssetId":"1234567890"} — the order and the asset exist nowhere in the service.
3. Observe HTTP 200 with a minted CapacityTaskId and the bogus OrderId/AssetId echoed, instead of a 404 NotFoundException.
4. GET /outposts/{OutpostId}/capacity/{CapacityTaskId} — the task is reported (and later COMPLETED) against an order and an asset that do not exist.
5. For contrast, GET /outposts/{OutpostId}/supportedInstanceTypes?OrderId=oo-1111111111111111a — the same crate answers 404 NotFoundException with Message `orderId=oo-1111111111111111a`.
