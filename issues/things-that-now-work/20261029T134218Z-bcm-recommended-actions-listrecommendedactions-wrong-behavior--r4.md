Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1b771983778e24aa6377b2fcf224b11c2656e2db

# Boot inventory's lastUpdatedTimeStamp is stamped by the first reader's clock, contradicting the crate's own "installed at emulator start, not by a reader" rule

_Filed: 2026-10-29T13:42:18Z_
_Operation: ListRecommendedActions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1b771983778e24aa6377b2fcf224b11c2656e2db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both this crate's state module and its one handler state, as the rule a reader of the code is told to rely on, that the computed recommendation inventory is part of the store's INITIAL value and is NOT something a reader installs on first use. The code under those comments does the opposite: the only path either of them has to the store is `crate::state::service_state`, which resolves the store through `engine::InMemoryEngine::state::<...>()` — an accessor whose contract is "initializing with `Default` if absent" via `entry().or_insert_with(...)`. Since `ListRecommendedActions` is the ONLY operation this service binds, the first caller of that read is always what constructs `RecommendedActionsStore::default()`, and `computed_recommendations()` stamps every record's `last_updated_timestamp` with `std::time::SystemTime::now()` at that instant.

The observable consequence is that the value reported for `RecommendedAction$lastUpdatedTimeStamp` — modelled as "The time when the recommended action status was last updated" — is the wall-clock instant of the FIRST ListRecommendedActions request rather than an instant tied to when the emulator (or the service's recommendation generator) produced the inventory. An emulator that has been up for an hour and is then listed for the first time reports every boot record as having been updated just now; the same emulator listed immediately at start reports a different instant for the identical, unchanged inventory. So a reported member of the response is a function of when the first read happened, which is exactly the property the two comments claim the design avoids, and a future author reading either comment will reason about an inventory installed at startup that the crate never installs at startup.

What must stay unchanged: repeated reads of unchanged records still have to answer byte-identically (`repeated_reads_of_the_boot_inventory_answer_with_identical_bytes`), and the boot records must keep a non-empty `lastUpdatedTimeStamp` (`the_boot_inventory_is_reported_and_is_filterable`).

## Evidence

Rule side, in this crate:
- services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/state.rs:265-268 — "/// The inventory the service computed for this account — see / /// [`computed_recommendations`]. Part of the store's INITIAL value rather than / /// something a reader installs on first use, so `ListRecommendedActions` stays / /// a pure read: two callers see the same records whichever of them called first."
- src/state.rs:213 — "/// The recommended actions this emulator holds when it starts."
- src/ops/list_recommended_actions.rs:14-18 (handler doc) — "The records come from the store, which already holds the inventory this account has when the emulator starts ([`crate::state::RecommendedActionsStore::default`])".

Code side, the crate's own state-access path:
- src/state.rs:325-331 — `pub fn service_state<Eng ...>(engine: &Eng) -> Option<...>` ... `.map(::engine::InMemoryEngine::state::<ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>)` (line 330) — the single seam the handler uses.
- src/ops/list_recommended_actions.rs:702 — `let state = crate::state::service_state(&*self.engine).ok_or_else(...)` — the first and only time this service touches its store is inside a read.
- vendor/engine/src/lib.rs:122 — "/// Get a handle to a service's state, initializing with `Default` if absent." and vendor/engine/src/lib.rs:134-136 — `let arc = map.entry(TypeId::of::<T>()).or_insert_with(|| Box::new(Arc::new(Mutex::new(T::default()))))` — so `Default` runs on FIRST ACCESS, i.e. inside the first reader, not at emulator construction (`InMemoryEngine::new` builds an empty map, vendor/engine/src/lib.rs:106-111).
- src/state.rs:228-229 — `fn computed_recommendations() -> Vec<StoredAction> { let computed_at = iso8601(std::time::SystemTime::now());` — every boot record's timestamp is that first-access instant, passed to `computed(..., &computed_at)` (src/state.rs:209: `last_updated_timestamp: computed_at.to_string()`).

Model text for the member whose value this decides:
- `smithy_query(query="shape", shape="RecommendedAction")` — `"lastUpdatedTimeStamp": {"target": "String", "doc": "<p>The time when the recommended action status was last updated.</p>"}`.

Test coverage that pins what may not change: tests/all/context_map_order.rs:301 `repeated_reads_of_the_boot_inventory_answer_with_identical_bytes` (two reads of the boot inventory must be byte-identical) and tests/all/list_recommended_actions.rs:1256-1261 (each boot record must carry a non-empty `lastUpdatedTimeStamp`). No test pins the VALUE of the boot timestamp, so nothing currently detects that it is the first request's clock reading.

## Steps to Reproduce

1. Read services/.../src/state.rs:265-268 and src/ops/list_recommended_actions.rs:14-18: both state the inventory is the store's initial value, installed at emulator start and not by a reader.
2. Read src/state.rs:325-331: the only way this crate reaches its store is `::engine::InMemoryEngine::state::<...>`.
3. Read vendor/engine/src/lib.rs:122 and 134-136: that accessor builds `T::default()` lazily, on first access, via `entry().or_insert_with(...)`; `InMemoryEngine::new()` (lines 106-111) starts with an empty map.
4. Read src/ops/list_recommended_actions.rs:702: the first access is inside the handler of the service's only operation — a read — so the first reader is what runs `RecommendedActionsStore::default()`.
5. Read src/state.rs:228-229 and 209: `computed_recommendations()` takes `SystemTime::now()` at that moment and copies it into every record's `last_updated_timestamp`, which the projection reports as `RecommendedAction$lastUpdatedTimeStamp` (src/ops/list_recommended_actions.rs:619).
6. Conclude: start an emulator, wait, then call ListRecommendedActions once — every boot record reports `lastUpdatedTimeStamp` equal to the instant of that first call, i.e. the reported field is a function of when the first read happened, which both comments say the design avoids.
