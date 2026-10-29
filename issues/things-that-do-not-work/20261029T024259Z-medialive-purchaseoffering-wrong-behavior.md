Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# A RequestId spent on a reservation the account has since deleted still replays that DELETED reservation instead of purchasing a new one

_Filed: 2026-10-29T02:42:59Z_
_Operation: PurchaseOffering | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteReservation` is a tombstone delete: `delete_reservation` sets `deleted = true` and the record stays in `self.reservations`, so `DescribeReservation`/`ListReservations` keep reporting it in the terminal `DELETED` state. The reservation idempotency-token map is never cleared when that happens, and `reservation_for_token` resolves a token purely by looking the recorded id up in `self.reservations`, which still holds the tombstone.

So a later `PurchaseOffering` carrying the `RequestId` of a reservation that has since been deleted takes the handler's replay branch and answers 200 with the DELETED reservation — one whose `State` is `DELETED` and that `UpdateReservation` refuses with 404 "already deleted" — while buying and storing nothing. The caller is told its purchase succeeded and hands back an id it can neither use nor change.

That contradicts the rule this crate states in the shared token helper and implements for channels, networks, clusters, signal maps and the four template/template-group slices: a token must not outlive the resource it minted, so a retry past the delete mints a new resource. The purchase handler's own comment beside `record_reservation_token` asserts exactly that invariant and nothing implements it: `reservation_tokens` is absent from every `.forget(` call site in the crate, and `delete_reservation` — the one place a reservation dies — forgets nothing.

## Evidence

The rule, from the shared token helper the reservation token map is an instance of — services/service-com-amazonaws-medialive-medialive/src/state.rs:1790-1792:
```
    /// Drop every token that created `resource_id`, so a token never outlives
    /// the resource it minted.
    fn forget(&mut self, resource_id: &str) {
```
and the harm the crate names for not doing it, from the channel arm of `settle` — src/state.rs:2459-2465:
```
        // A token never outlives the channel it minted: once that channel has
        // reached the terminal `DELETED` state its idempotency window is over, so
        // a retried create mints a new channel rather than answering with a
        // deleted one.
        for id in just_deleted {
            self.channel_tokens.forget(&id);
        }
```

The purchase handler ASSERTS the same invariant for its own token — src/ops/purchase_offering.rs:282-286:
```
        if let Some(token) = input.request_id {
            // Bounded, evicting write: a caller-chosen token can neither
            // accumulate past the cap nor outlive the reservation it purchased.
            state.record_reservation_token(token, &id);
        }
```
but the delete keeps the record and clears no token — src/state/reservations.rs:572-585:
```
    /// The record is KEPT, in the terminal `DELETED` state its own enum
    /// declares, so a reader goes on reporting the reservation there instead of
    /// losing it, and the delete is idempotent: a reservation already in that
    /// state is the state the caller asked for, so a repeat writes nothing and
    /// answers the same reservation.
    pub fn delete_reservation(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Result<Reservation, ReservationDeleteRefusal> {
        let record = self
            .reservations
            .get_mut(id)
            .ok_or(ReservationDeleteRefusal::Unknown)?;
```
so the token lookup still answers for the tombstone — src/state/reservations.rs:472-479:
```
    /// The reservation a stored `RequestId` purchased, if that reservation still
    /// exists.
    pub fn reservation_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.reservation_tokens.created(request_id)?;
        self.reservations
            .get(id)
            .map(|reservation| reservation.id.as_str())
    }
```
(and `created` is documented src/state.rs:1785-1788 as "The id `token` created, whether or not that resource still exists" — i.e. the filtering is meant to come from `forget`). `grep -n '\.forget(' src` lists only `cluster_tokens` (src/state/clusters.rs:478), `channel_tokens` (src/state.rs:2464), `network_tokens` (src/state.rs:2484), `event_bridge_rule_template_group_tokens` (src/state.rs:3146), `signal_map_tokens` (src/state.rs:3858), `alarm_template_tokens` (src/state/alarm_templates.rs:595), `event_bridge_rule_template_tokens` (src/state/event_bridge_rule_templates.rs:498) and `alarm_template_group_tokens` (src/state/alarm_template_groups.rs:253) — `reservation_tokens` is absent.

The replay branch that answers with the tombstone — src/ops/purchase_offering.rs:238-254:
```
        // A retried purchase (same `@idempotencyToken` `RequestId`) must answer
        // with the reservation the first one created — "Unique request ID to be
        // specified. This is needed to prevent retries from creating multiple
        // resources" — not buy a second one.
        if let Some(existing) = input
            .request_id
            .as_deref()
            .and_then(|token| state.reservation_for_token(token))
            .map(str::to_string)
        {
            let reservation = state
                .reservation_description(&existing, now)
                .ok_or_else(unavailable)?;
            return Ok(PurchaseOfferingOutput {
                reservation: Some(reservation),
            });
        }
```
and the sibling that proves the replayed id is unusable — src/ops/update_reservation.rs:188-212 answers `NotFoundException` with `already_deleted_message` for exactly that record, over src/state/reservations.rs:550-553 (`.filter(|record| !record.is_deleted())?`), while src/state/reservations.rs:176-196 derives its `State` as the terminal `DELETED`.

Test coverage: the purchase idempotency test covers only live-resource rows and never re-presents a token after a delete — tests/all/reservation_purchase.rs:682-737 (`a_retried_purchase_returns_the_reservation_the_first_one_created`, rows `("another token", Some("token-b"))` and `("no token at all", None)`), while the same rule past the delete IS pinned for a resource that implements it — tests/all/signal_map_deletion.rs:481-489 (`a_deleted_signal_map_is_not_resurrected_by_its_idempotency_token`).

## Steps to Reproduce

1. POST /prod/offerings/{offeringId}/purchase with {"count":1,"name":"r1","requestId":"token-a","start":"<an expired-by-now start inside the allowed window>"} -> 200; note the reservation id R.
2. Wait until GET /prod/reservations/R reports reservation.state == "EXPIRED" (derived from the lease end, src/state/reservations.rs:176-189).
3. DELETE /prod/reservations/R -> 200; GET /prod/reservations/R now reports reservation.state == "DELETED" (the record is kept, src/state/reservations.rs:572-576).
4. POST /prod/offerings/{anotherOfferingId}/purchase again with {"count":5,"name":"r2","requestId":"token-a"} -> the emulator answers 200 carrying reservation R with "state":"DELETED" and stores nothing; GET /prod/reservations shows no new reservation. Expected, per src/state.rs:1790-1792 and the invariant src/ops/purchase_offering.rs:283-284 asserts, a freshly purchased reservation — which is what POST /prod/signal-maps and POST /prod/clusters give for a token whose resource was deleted.
5. PUT /prod/reservations/R with {"name":"x"} -> 404 "already deleted" (src/ops/update_reservation.rs:203-211), so the 200 in step 4 named a resource the caller can neither use nor change.
