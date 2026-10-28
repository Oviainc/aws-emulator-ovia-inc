Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# A newly created Outpost flips itself to ACTIVE half a second after CreateOutpost, with no order placed or rack installed

_Filed: 2026-10-24T02:09:44Z_
_Operation: GetOutpost | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateOutpost` stores the Outpost with `LifeCycleStatus=PENDING` and an `active_at` instant 500 ms in the future; every read path (`GetOutpost`, `ListOutposts`, `UpdateOutpost`) calls `settle_outposts()`, which rewrites the status to `ACTIVE` once that instant has passed. Because an HTTP round trip plus any client-side work easily exceeds 500 ms, a workflow that creates an Outpost and then reads it back observes `ACTIVE`.

On the real service, the logical Outpost an account creates with `CreateOutpost` has no capacity until an order is placed and the hardware is delivered, installed and activated; its lifecycle status stays `PENDING` until then — it cannot transition on a timer inside the API. The emulator therefore reports a status the real service would not report for the same resource, and the divergence is also visible through `ListOutposts?LifeCycleStatusFilter=PENDING`, which stops matching an Outpost half a second after it was created.

Expected: an Outpost created by `CreateOutpost` (with no order fulfilled — note every order operation in this crate is still a 501 scaffold, so no order can be fulfilled) keeps reporting `PENDING` on every subsequent read. Actual: it reports `ACTIVE` from ~500 ms after creation onwards.

## Evidence

Spec/semantics: the model declares `Outpost$LifeCycleStatus` (len 1..20, pattern `^[ A-Za-z]+$`) with no value set, so the values come from service behaviour; AWS Outposts documents the Outpost lifecycle as driven by order fulfilment and rack installation (an Outpost is PENDING until the hardware is installed and activated), and this crate implements no order path at all — `CreateOrder`/`GetOrder`/`StartCapacityTask` are scaffolded 501 handlers.

Handler/state: `src/state.rs` `OUTPOST_PROVISIONING = Duration::from_millis(500)` and `settle_outposts()` (lines ~110-118), called from `src/ops/get_outpost.rs` (line ~158), `src/ops/list_outposts.rs` (line ~173) and `src/ops/update_outpost.rs` (line ~171). The duration's own doc comment admits "AWS publishes no duration for provisioning an Outpost, so this span is the emulator's own".

Tests: `tests/all/site_and_outpost_projection.rs::a_created_outpost_provisions_then_settles_on_the_clock` pins the current timer-driven transition, so the behaviour is asserted as-is rather than being an oversight.

## Steps to Reproduce

1. CreateSite, keep the SiteId.
2. CreateOutpost `{"Name":"lifecycle","SiteId":<site>}` — the response reports LifeCycleStatus PENDING.
3. Wait ~1 second (or simply issue the next call from a normal SDK client) and call GetOutpost on the new OutpostId.
4. Observe LifeCycleStatus ACTIVE, although no order was ever created or fulfilled; the real service still reports PENDING.
5. Call `GET /outposts?LifeCycleStatusFilter=PENDING` — the Outpost is no longer returned.
