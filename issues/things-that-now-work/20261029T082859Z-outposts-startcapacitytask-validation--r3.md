Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2afcd9ff504122666b9644d9cff6b63f96e63ba3

# StartCapacityTask accepts an OrderId naming an order placed against a DIFFERENT Outpost, then lets that foreign order block the real Outpost's own capacity task

_Filed: 2026-10-29T08:28:59Z_
_Operation: StartCapacityTask | Class: validation_

> _Note: this issue was identified against the code base at commit `2afcd9ff504122666b9644d9cff6b63f96e63ba3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartCapacityTask` resolves its optional `OrderId` by EXISTENCE IN THE REGION only: `store.order(&region, order_id).is_none()` is the whole check. It never asks whether that order was placed against the Outpost the task is being started on (the `OutpostIdentifier` the route addressed and that the handler has already resolved into `outpost_id`). An order of Outpost B is therefore accepted on `POST /outposts/{A}/capacity` and stored on A's task as its `OrderId`.

The sibling handler in this same crate resolves the SAME member against the SAME two facts and refuses exactly that case: `GetOutpostSupportedInstanceTypes` requires `record.order.outpost_id == outpost_id` and answers the modelled `NotFoundException` on `orderId` for "one placed against another Outpost". So one path in this crate applies the parentage check and the other, which persists the reference rather than merely reading through it, does not.

The damage is not confined to a cosmetic dangling field. The handler's own one-active-task rule is keyed on that stored `OrderId` through `state::active_capacity_task_on_order`, so the foreign reference makes a LEGITIMATE `StartCapacityTask` on Outpost B naming order O fail with `ConflictException` / `ResourceType: ORDER` / `ResourceId: O` — a conflict caused by a task that is not on B at all, and that `ListCapacityTasks?OutpostIdentifierFilter=B` will never show the caller, so there is nothing they can poll or cancel to clear it. `GetCapacityTask` on A likewise reports an `OrderId` that names an order of other hardware.

This is distinct from the two already-fixed `StartCapacityTask` validation gaps: the first was about an `OrderId`/`AssetId` naming nothing at all, the second about an order of another REGION. Both are now refused; an order of another OUTPOST in the SAME region still is not.

## Evidence

Model — `CapacityTaskSummary$OrderId` (`smithy_query(query="doc", shape="CapacityTaskSummary", member="OrderId")`) states what the member is:

    "The ID of the Amazon Web Services Outposts order of the host associated with the capacity task."

and `StartCapacityTaskInput$OutpostIdentifier` is "The ID or ARN of the Outposts associated with the specified capacity task." An order placed against another Outpost delivered no host of THIS Outpost, so it is not "the order of the host associated with the capacity task".

The check this handler makes — `services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/start_capacity_task.rs:295-308`:

        let order_id = crate::state::non_empty(input.order_id);
        if let Some(order_id) = order_id.as_deref() {
            if store.order(&region, order_id).is_none() {
                return Err(StartCapacityTaskErrorModeled::NotFoundException(
                    NotFoundException {
                        message: Some(crate::state::identifier_not_found(
                            crate::state::ORDER_ID_MEMBER,
                            order_id,
                        )),
                    },
                )
                .into());
            }
        }

The check the sibling handler makes for the same member — `services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/get_outpost_supported_instance_types.rs:254-281`:

        // An order scopes the report to the hardware THAT order is for, so it has to
        // be an order of this Outpost: one this store holds no record of, or one
        // placed against another Outpost, names no hardware here and is the same
        // not-found error, echoed on the member it arrived in.
        ...
        if let Some(order_id) = input.order_id.as_deref().filter(|id| !id.is_empty()) {
            let places_this_outpost = store
                .order(&region, order_id)
                .is_some_and(|record| record.order.outpost_id.as_deref() == Some(&outpost_id));
            if !places_this_outpost {
                return Err(
                    GetOutpostSupportedInstanceTypesErrorModeled::NotFoundException(
                        NotFoundException {
                            message: Some(crate::state::identifier_not_found(
                                crate::state::ORDER_ID_MEMBER,
                                order_id,
                            )),

What the unchecked reference then does — `src/ops/start_capacity_task.rs:385-419`:

        // "You can have one active capacity task for each order and each Outpost":
        // a second one is the conflict this operation models, naming the resource
        // the conflict is about in `ResourceId` / `ResourceType`.
        let conflict = store
            .active_capacity_task_on_outpost(&outpost_id)
            ...
            .or_else(|| {
                let order_id = order_id.clone()?;
                let active = store.active_capacity_task_on_order(&region, &order_id)?;
                Some((
                    crate::state::RESOURCE_TYPE_ORDER,
                    order_id,
                    active.capacity_task_id.clone(),
                ))
            });

and `src/state.rs:4199-4206`, which matches on the stored id alone:

    pub fn active_capacity_task_on_order(
        &self,
        region: &str,
        order_id: &str,
    ) -> Option<&CapacityTaskRecord> {
        self.capacity_tasks_in(region)
            .find(|task| task.order_id.as_deref() == Some(order_id) && task.is_active())
    }

Also stored on the task unconditionally — `src/ops/start_capacity_task.rs:453-462` (`order_id,` into `CapacityTaskRecord`), and reported back by `crate::capacity_task_projection!` (`src/state.rs:927`: `order_id: record.order_id.clone(),`).

Test coverage today: `tests/all/capacity_task_order_region.rs` exercises an `OrderId` of another REGION (the already-fixed gap) and `tests/all/capacity_task_lifecycle.rs` / `capacity_task_listing.rs` use an order of the same Outpost; no test starts a task on one Outpost naming an order placed against a second Outpost of the same region, so neither the acceptance nor the resulting cross-Outpost conflict is pinned with a stated reason.

## Steps to Reproduce

1. CreateSite, then CreateOutpost twice in the same Region against that site — call the Outposts A and B.
2. CreateOrder with OutpostIdentifier=B and one LineItem; note the returned OrderId, call it O. (GetOrder shows O.OutpostId == B.)
3. POST /outposts/{A}/capacity with {"OrderId":"O","InstancePools":[{"InstanceType":"<a type A's hardware runs>","Count":1}],"DryRun":false} — answered HTTP 200 and a task is minted on A carrying OrderId=O, although O was placed against B. GET /outposts/{A}/capacity/{CapacityTaskId} reports OrderId=O.
4. Compare the sibling: GET /outposts/{A}/supportedInstanceTypes?OrderId=O is answered HTTP 404 NotFoundException with body `orderId=O` — the same member, the same two records, the opposite verdict.
5. Immediately POST /outposts/{B}/capacity with {"OrderId":"O","InstancePools":[...],"DryRun":false} — B's own, correct request is refused HTTP 409 ConflictException with ResourceType=ORDER, ResourceId=O and a message naming A's capacity task.
6. GET /capacity/tasks?OutpostIdentifierFilter=B returns no task at all, so the caller is blocked by a task they cannot see, poll or cancel through B's routes.
