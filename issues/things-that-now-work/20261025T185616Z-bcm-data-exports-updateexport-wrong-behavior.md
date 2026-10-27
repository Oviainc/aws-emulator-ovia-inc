Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# UpdateExport materialises due deliveries with a pre-validation instant, so a delivery that falls due during validation is silently discarded

_Filed: 2026-10-25T18:56:16Z_
_Operation: UpdateExport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateExport` captures its instant (`at`) at the TOP of the handler, before the lock-free validation of the submitted definition (which parses the caller's SQL on a freshly spawned thread), and then, inside the mutating critical section, calls `store.materialize_due(at)` with that now-stale instant before overwriting `stored.pending_delivery`.

Consequence: a pending delivery whose stored `initiate_at` falls in the window `(at, now]` — i.e. one that became due WHILE this request was being validated — is not initiated by `materialize_due(at)` (the `take_if(|delivery| delivery.initiate_at <= now)` predicate is false for it), and is then destroyed one line later when `stored.pending_delivery` is replaced with the delivery owed for the new definition. The execution record the clock already owed is lost: no later `ListExecutions`/`GetExecution` can ever report it, and the first definition is never delivered even though its instant had passed.

The handler's own comment claims the opposite behaviour, and the sibling `CreateExport` — which carries the identical "ONE instant answers the whole request" comment — takes its instant AFTER all validation, immediately before taking the lock, so it cannot lose a delivery this way. Every read handler (`GetExport`, `ListExports`, `ListTagsForResource`, `DeleteExport`, …) likewise materialises with `SystemTime::now()` taken at the lock. The same stale instant is also what `ExportStatus.LastUpdatedAt` is written from, so the update reports an instant from before the work it did.

## Evidence

Stale instant captured before validation, then used to materialise:
- `src/ops/update_export.rs:167` — `let at = ::std::time::SystemTime::now();` (validation follows at lines 219-257: `crate::state::validate_data_query(...)`, `validate_destination_configurations(...)`, `effective_table_configurations(...)`)
- `src/ops/update_export.rs:262-265` — comment: "Whatever the clock already owes is materialised BEFORE the definition is replaced, so a delivery that fell due while this request was being validated records the definition it was owed for rather than the one replacing it." followed by `store.materialize_due(at);`
- `src/ops/update_export.rs:290-294` — `stored.pending_delivery = Some(crate::state::delivery_owed_at(at, &stored.export, stored.deliveries_owed,));` — the un-materialised, already-due delivery is overwritten here.

The predicate that skips it: `src/state.rs` (`materialize_due`) — `export.pending_delivery.take_if(|delivery| delivery.initiate_at <= now)`; with `now == at < initiate_at <= wall-clock now` nothing is taken.

Sibling that does it correctly: `src/ops/create_export.rs:254-261` — "ONE instant answers the whole request: … the instant it materialises what the clock already owed …" with `let at = ::std::time::SystemTime::now();` placed AFTER `validate_data_query` (line 217), `validate_destination_configurations` (line 236) and `effective_table_configurations` (line 243), immediately before `let mut store = state.lock(); store.materialize_due(at);` (lines 261-265). Read siblings take the instant at the lock, e.g. `src/ops/get_export.rs:160` — `store.materialize_due(::std::time::SystemTime::now());`.

Test coverage: `tests/all/delivery_executions.rs:419` (`each_persisted_definition_is_owed_a_delivery_of_itself`) covers only an update whose pending delivery is held in the FUTURE (`hold_delivery(&engine, &arn)`) or already run (`settle_initiation` + a call), so the window between `at` and the lock is untested; moving the instant later does not change those rows.

## Steps to Reproduce

1. CreateExport an export — the service stores a pending delivery due DELIVERY_INITIATION_DELAY (500 ms) later (`src/state.rs`: `pub const DELIVERY_INITIATION_DELAY: Duration = Duration::from_millis(500);`).
2. About 450 ms later, call UpdateExport for that ARN with a valid replacement definition whose `QueryStatement` is large enough that parsing it (thread spawn + sqlparser on a 16 MiB stack) takes more than the remaining ~50 ms.
3. Wait past the superseding delivery's own instant and call ListExecutions for the ARN: only ONE execution is ever reported (the new definition's). The delivery owed for the first definition — whose stored `initiate_at` had already passed when the lock was taken — was neither initiated nor kept.
4. Confirm in code: `materialize_due(at)` at update_export.rs:265 is passed the instant from line 167, which precedes the first definition's `initiate_at`, so `take_if` leaves the pending delivery in place; line 290 then overwrites it.
