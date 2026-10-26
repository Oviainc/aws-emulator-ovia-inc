Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3af8ecaf4ce2325789e9d5260e121bf04ff83dc7

# StartCapacityTask resolves its OrderId account-wide instead of in the request's region, so an order of another region is accepted and stored as a dangling reference

_Filed: 2026-10-26T08:11:11Z_
_Operation: StartCapacityTask | Class: validation_

> _Note: this issue was identified against the code base at commit `3af8ecaf4ce2325789e9d5260e121bf04ff83dc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other path in this crate resolves an `OrderId` inside the region the request is addressed to, because the crate's stated invariant is that a resource belongs to the region it was created in and to no other. `StartCapacityTask` is the exception: it checks the optional `OrderId` with `state::holds_order`, which is a bare `self.orders.contains_key(order_id)` with no region argument at all.

Consequences, both observable over the wire:

* A `StartCapacityTask` addressed to region A naming an `OrderId` that exists only in region B is accepted (200), and the id is persisted on the task. `GetCapacityTask` in region A then reports an `OrderId` that `GetOrder`/`CancelOrder` in region A answer 404 `NotFoundException` for — exactly the dangling reference the existence check was added to prevent, just displaced from "an id nothing holds" to "an id this region's endpoint does not serve".
* The one-active-task-per-order rule is region-blind for the same reason (`active_capacity_task_on_order` takes no region), so a task still active on an order in region B refuses a start in region A that names the same order id, with a `ConflictException` naming a resource region A cannot show the caller.

This is the region-scoping residual of the already-fixed `StartCapacityTask` validation gap (which was about ids that name nothing anywhere); it is not that gap re-filed.

## Evidence

The region-blind check the handler uses — `src/state.rs:3498-3508`:

    /// Whether this store holds the Amazon Web Services Outposts order `order_id`.
    ///
    /// An order is a TOP-LEVEL resource of the account (`CreateOrder` persists one
    /// and `GetOrder` reads it back at `/orders/{OrderId}`), so this asks only
    /// whether the account placed it. ...
    pub fn holds_order(&self, order_id: &str) -> bool {
        self.orders.contains_key(order_id)
    }

called at `src/ops/start_capacity_task.rs:273-286`:

        let order_id = crate::state::non_empty(input.order_id);
        if let Some(order_id) = order_id.as_deref() {
            if !store.holds_order(order_id) {
                return Err(StartCapacityTaskErrorModeled::NotFoundException(...

The rule the rest of the crate applies to the SAME member and the SAME store, with file:line on both sides:

* `src/state.rs:2490-2493` — `/// The order `order_id` names in `region`. See [`Self::site`].` / `pub fn order(&self, region: &str, order_id: &str) -> Option<&OrderRecord> { in_region(&self.orders, region, order_id) }`
* `src/state.rs:3082-3084` — of `outstanding_order_on_outpost`: `/// Scoped to `region` for the reason [`RegionalRecord`] gives;`
* `src/ops/get_order.rs:143-144` — `// The region this request is addressed to: an order is part of its own` / `// region's inventory and of no other's, so that is where the id is resolved.`
* `src/ops/cancel_order.rs:148-150` — the same sentence, and `store.order_mut(&region, &input.order_id)` at line 162.
* `src/ops/get_outpost_supported_instance_types.rs:252-255` — the sibling that takes the very same optional `OrderId` resolves it region-scoped AND against the Outpost in hand: `let places_this_outpost = store.order(&region, order_id).is_some_and(|record| record.order.outpost_id.as_deref() == Some(&outpost_id));`

The conflict rule is region-blind too — `src/state.rs:3068-3072`: `pub fn active_capacity_task_on_order(&self, order_id: &str) -> Option<&CapacityTaskRecord> { self.capacity_tasks.values().find(|task| task.order_id.as_deref() == Some(order_id) && task.is_active()) }`.

Test coverage: `tests/all/order_cancellation.rs:24-27` pins the regional rule for orders — "An `OrderId` this region's inventory holds no order for is 404 ... including an order of ANOTHER region, which this region's endpoint does not serve" — and `tests/all/regional_scope.rs:7-13` states the rule for "every route of the implemented slice that names a stored resource". Neither exercises `StartCapacityTask$OrderId`: the task that suite starts carries no `OrderId` at all (`tests/all/regional_scope.rs:183-190`, body `{"InstancePools": [pool("m5.4xlarge", 1)]}`), so nothing pins the current cross-region acceptance.

## Steps to Reproduce

1. Sign for us-west-2: CreateSite, CreateOutpost on it, then POST /orders with that OutpostIdentifier, PaymentOption ALL_UPFRONT and one LineItem. Keep the returned OrderId (oo-…).
2. Sign for us-east-1: CreateSite and CreateOutpost there; keep that OutpostId.
3. Still in us-east-1, POST /outposts/{us-east-1 OutpostId}/capacity with {"InstancePools":[{"InstanceType":"m5.4xlarge","Count":1}],"OrderId":"<the us-west-2 OrderId>"} — observe HTTP 200 and the task echoing that OrderId, instead of the 404 NotFoundException this region's endpoint gives for the same id everywhere else.
4. In us-east-1, GET /orders/{that OrderId} — 404 NotFoundException. GET /outposts/{OutpostId}/capacity/{CapacityTaskId} in us-east-1 still reports the task carrying the unreachable OrderId.
5. While that task is active, sign for us-west-2 and POST /outposts/{us-west-2 OutpostId}/capacity with the same OrderId — observe a 409 ConflictException naming an order/task of the other region.
