Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# StartQuery's two replay paths derive the replayed query's status from two different clock readings in one request

_Filed: 2026-10-28T21:57:32Z_
_Operation: StartQuery | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartQuery answers a retry carrying an already-spent `clientToken` from the query that token started, reporting that query's derived status. The handler contains the replay block twice — once before the statement runs and once after the write lock is re-taken — and the two copies have drifted: the first derives the status from `submitted_at`, the single instant the handler read at its start, while the second re-reads the wall clock. The query's status is a pure function of the clock (SUBMITTED for one step, then RUNNING, then terminal), so the two paths can report different statuses for the same query in the same request: the statement run between them is unbounded by this handler, so the second reading can be a full QUERY_STEP later than the first.

This crate anchors "now" once per request precisely so one request cannot disagree with itself about it, and says so where it matters most. The second copy of the replay block is the only place in this handler that steps outside that anchor; it also means the answer a caller gets for a retry depends on which of the two identical-looking branches happened to catch it.

## Evidence

The handler's one anchor and its first replay path:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/start_query.rs:166 — "let submitted_at = ::std::time::SystemTime::now();"
  src/ops/start_query.rs:176-181 — "if let Some(existing) = replayed(&store, &input.workspace_name, &input) {
                return Ok(StartQueryOutput {
                    query_id: existing.query_id.clone(),
                    status: existing.status(submitted_at).to_string(),
                });"

The second copy, re-reading the clock for the same expression:
  src/ops/start_query.rs:237-242 — "if let Some(existing) = replayed(&store, &input.workspace_name, &input) {
            return Ok(StartQueryOutput {
                query_id: existing.query_id.clone(),
                status: existing.status(::std::time::SystemTime::now()).to_string(),
            });"

The status being derived purely from the clock:
  src/state.rs:3728-3738 — "pub fn status(&self, now: SystemTime) -> &'static str { ... (None, None) if now >= self.submitted_at + QUERY_STEP => QUERY_RUNNING, (None, None) => QUERY_SUBMITTED, }"

The rule the crate states about one instant per request:
  src/ops/batch_put_asset_property_value.rs:213-216 — "// The window every TQV's timestamp is judged against is anchored on ONE
        // instant for the whole request, so two entries of the same batch cannot
        // disagree about where \"now\" was.
        let now = SystemTime::now();"
  and the same discipline in this very handler, src/ops/start_query.rs:263-265 — "// The whole lifecycle is owed from here: the status is derived from this
            // instant rather than stored, and nothing is slept on.
            submitted_at,"

Test coverage: tests/all/workspace_queries.rs and tests/all/query_cancellation.rs exercise StartQuery's token replay and the derived statuses, but neither asserts which instant a replayed answer is derived from, so nothing pins the drift.

## Steps to Reproduce

1. CreateWorkspace W.
2. StartQuery in W with a clientToken T and a statement the engine runs -> 202, queryId Q, status SUBMITTED.
3. StartQuery in W again with the same clientToken T and the same statement: the request is answered by the replay block at start_query.rs:176-181, whose status comes from `submitted_at` (the instant this request started).
4. Issue the same retry so that it reaches the post-run replay block at start_query.rs:237-242 (e.g. two concurrent retries under T, where the loser re-locks after running the statement): its status comes from a fresh `SystemTime::now()` instead.
5. Observed: the status reported for one and the same query Q is derived from two different instants in one request path, so two retries of the same call can report SUBMITTED and RUNNING respectively for reasons internal to the handler.
6. Expected per the crate's own anchoring rule (batch_put_asset_property_value.rs:213-216, start_query.rs:263-265): both replay paths derive the status from the single instant the request was anchored at.
