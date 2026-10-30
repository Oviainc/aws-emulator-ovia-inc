Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 160eac1be171dea85ae8441ad5f20b2dc0c6ebbb

# UpdateQueue restarts a reserved queue's 12-month commitment even when no additional capacity is purchased

_Filed: 2026-10-28T19:34:03Z_
_Operation: UpdateQueue | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `160eac1be171dea85ae8441ad5f20b2dc0c6ebbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateQueue` replaces a reserved queue's `ReservationPlan` unconditionally whenever the request carries `ReservationPlanSettings`, and the replacement is always built with `now` as the purchase instant. `crate::state::reservation_plan` then derives BOTH `PurchasedAt` (= now) and `ExpiresAt` (= now + RESERVATION_TERM) from that instant.

The handler itself only refuses a DECREASE of `ReservedSlots` (`if settings.reserved_slots < stored_slots`), so a request that restates the SAME slot count — or that changes only `RenewalType`/`Commitment` — is accepted and silently resets the queue's commitment term to a fresh 12 months. The account's original `PurchasedAt` is erased and `ExpiresAt` is pushed out a year, although no additional capacity was bought.

The model conditions the new commitment on an INCREASE: `ReservationPlan$ReservedSlots` says the new 12-month commitment begins "when you purchase the additional capacity". The handler's own comment states the same rule ("by increasing the number of RTS") and the code applies the restart when the number does not increase at all — so the comment and the code under it disagree.

## Evidence

Smithy, `ReservationPlan$ReservedSlots` (smithy_query doc): "When you increase this number, you extend your existing commitment with a new 12-month commitment for a larger number of RTS. **The new commitment begins when you purchase the additional capacity.** You can't decrease the number of RTS in your reserved queue."

Smithy, `ReservationPlanSettings$ReservedSlots`: "You can increase the number of RTS by extending your existing commitment with a new 12-month commitment for the larger number. The new commitment begins when you purchase the additional capacity."

Smithy, `ReservationPlan$PurchasedAt`: "The timestamp in epoch seconds for when you set up the current pricing plan for this reserved queue."

Handler, services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/update_queue.rs:299-304 — the comment states the rule, the code ignores its condition:
```
        if let Some(settings) = input.reservation_plan_settings.as_ref() {
            // "When you add capacity to your queue by increasing the number of RTS, you
            // extend the term of your commitment to 12 months from when you add capacity":
            // the term restarts from this call.
            record.reservation_plan = Some(crate::state::reservation_plan(settings, now));
        }
```

The guard above it (update_queue.rs:256-268) only rejects a strict decrease, so `settings.reserved_slots == stored_slots` reaches line 303:
```
            if let Some(stored_slots) = record.reserved_slots() {
                if settings.reserved_slots < stored_slots {
                    return Err(... reserved_slots_decrease_message ...)
```

state.rs:2254-2266 derives both timestamps from the instant handed in:
```
pub fn reservation_plan(
    settings: &ReservationPlanSettings,
    purchased_at: SystemTime,
) -> ReservationPlan {
    ReservationPlan {
        ...
        expires_at: Some(epoch_seconds(purchased_at + RESERVATION_TERM)),
        purchased_at: Some(epoch_seconds(purchased_at)),
```
with `pub const RESERVATION_TERM: Duration = Duration::from_secs(365 * 24 * 60 * 60);` (state.rs:458).

Test coverage: tests/all/queue_capacity_validation.rs:131-137 drives exactly this input and expects it to be APPLIED:
```
        SlotCase {
            label: "the same slot count: not a decrease",
            stored: 4,
            requested: 4,
            renewal_type: "AUTO_RENEW",
            outcome: SlotOutcome::Applied,
        },
```
and its assertions (queue_capacity_validation.rs:213-238) only check `purchased >= created_purchase` and `expires - purchased == term`, both of which still hold if the stored purchase instant is preserved — so nothing in the suite pins the reset.

## Steps to Reproduce

1. CreateQueue with `{"name":"q","pricingPlan":"RESERVED","reservationPlanSettings":{"commitment":"ONE_YEAR","renewalType":"EXPIRE","reservedSlots":4}}` and record `queue.reservationPlan.purchasedAt` / `expiresAt` from the 201 body.
2. Wait (or advance the clock) so a later call has a different `now`.
3. UpdateQueue `PUT /2017-08-29/queues/q` with `{"reservationPlanSettings":{"commitment":"ONE_YEAR","renewalType":"AUTO_RENEW","reservedSlots":4}}` — the same slot count, so no additional capacity is purchased.
4. Observe the 200 body (and a follow-up GetQueue): `reservationPlan.purchasedAt` has moved to the update instant and `expiresAt` is a fresh 12 months out, even though `reservedSlots` did not increase.
5. Expected per `ReservationPlan$ReservedSlots` ("The new commitment begins when you purchase the additional capacity"): with no increase there is no new commitment, so the stored `purchasedAt`/`expiresAt` of the current plan are unchanged.
