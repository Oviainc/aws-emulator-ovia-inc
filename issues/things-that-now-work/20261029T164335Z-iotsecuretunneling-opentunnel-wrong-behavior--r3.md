Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2eb36c3237995f28c1399a40c097f8e6870daea5

# OpenTunnel samples createdAt before taking the store lock while claiming creation_seq under it, so ListTunnels can report an older tunnel as newer than a younger one

_Filed: 2026-10-29T16:43:35Z_
_Operation: OpenTunnel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2eb36c3237995f28c1399a40c097f8e6870daea5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A tunnel's two creation facts are captured at two different moments that are NOT ordered with respect to each other:

* `created_at` (the value `DescribeTunnel`/`ListTunnels` report as `createdAt`) is sampled from the wall clock BEFORE the handler takes the shared store lock.
* `creation_seq` (the sole sort key `ListTunnels` orders by) is claimed from the store AFTER the lock is taken.

The lock serialises the sequence claims but not the clock samples, so two concurrent `OpenTunnel` requests can be stamped in one order and sequenced in the opposite order: request A samples `now = T1`, is descheduled before `state.lock()`, request B samples `now = T2 > T1` and acquires the lock first, so B gets the LOWER `creation_seq` and A the higher one. `ListTunnels` then reports A before B (A has the larger sequence, i.e. is treated as "newer") even though A's reported `createdAt` (T1) is EARLIER than B's (T2).

The response is then self-contradictory on the wire: the operation's documented order is "by creation time in descending order, newer tunnels will be listed before older tunnels", yet the `createdAt` values the same response carries are not in descending order, so a client that re-sorts the page by the `createdAt` it was given gets a different order from the one the service paged it in. Because `nextToken` encodes a `creation_seq` cutoff, the inverted pair is also paged in that same wrong order.

The same disagreement leaks into the record-eviction tie-break, which breaks equal `closed_at` values by `creation_seq` and so can evict the record that was created LATER of two closed in the same instant.

The fix is local to this handler: take the instant the record is stamped with from the same critical section that claims its sequence (i.e. sample the creation instant under the lock, beside `next_creation_seq()`), so the stamp order and the sequence order can never disagree. No constant, token format or response field changes.

## Evidence

The modelled order, `smithy_query(query="doc", shape="ListTunnels")` (also reproduced in the handler header at `src/ops/list_tunnels.rs:47-49`):

    "List all tunnels for an Amazon Web Services account. Tunnels are listed by creation time in descending order, newer tunnels will be listed before older tunnels."

The crate's own statement that the sequence IS creation time, `src/state.rs:303-306`:

    /// Creation order, so `ListTunnels` can report tunnels "by creation time in
    /// descending order" (the operation's own documentation) without depending on
    /// clock resolution for two tunnels opened in the same instant.
    pub creation_seq: u64,

The clock sample, taken BEFORE the lock — `src/ops/open_tunnel.rs:165` and `:187`:

    let now = ::std::time::SystemTime::now();
    let region = crate::state::request_region(ctx);
    ...
    let mut store = state.lock();

(between those two lines the handler also runs `stored_destination_config(...)` at `src/ops/open_tunnel.rs:180-185`, so the gap is real work, not a single instruction).

The sequence claim, taken AFTER the lock, and the stamp that reuses the pre-lock sample — `src/ops/open_tunnel.rs:287` and `:300`:

    let creation_seq = store.next_creation_seq();
    let tunnel = crate::state::StoredTunnel {
        ...
        created_at: now,

The listing orders by the sequence alone, `src/ops/list_tunnels.rs:194`:

    ordered.sort_by_key(|(creation_seq, _)| ::std::cmp::Reverse(*creation_seq));

and the page token is that same sequence, `src/ops/list_tunnels.rs:200-206` / `src/state.rs:798-800`:

    pub fn encode_next_token(seq: u64) -> String {
        format!("{NEXT_TOKEN_PREFIX}{seq:016x}")

The eviction tie-break that inherits the same key, `src/state.rs:487`:

    .min_by_key(|(_, closed_at, creation_seq)| (*closed_at, *creation_seq))

Test coverage: `tests/all/tunnel_lifecycle.rs:367 list_tunnels_filters_by_thing_name_and_reports_newest_first` opens its five tunnels one at a time (`for (thing_name, description) in plan { ... call(...).await }`, lines 380-392) and asserts only the id order (`assert_eq!(reported, newest_first, ...)`, lines 441-444) — it never issues two `OpenTunnel` calls concurrently and never compares the reported `createdAt` values against the order they were paged in, so the inversion is uncovered. `tests/all/record_eviction_order.rs` likewise builds its records sequentially.

## Steps to Reproduce

1. Mount the service on an `engine::InMemoryEngine` behind a `MultiServiceRouter` as `tests/all/tunnel_lifecycle.rs` does, on a multi-thread tokio runtime.
2. Issue many `OpenTunnel` requests CONCURRENTLY (e.g. `futures::future::join_all` over 50 calls with body `{"destinationConfig":{"thingName":"thing-race","services":["SSH"]}}`), instead of awaiting them one at a time.
3. Call `ListTunnels` with `{}` and read `tunnelSummaries` in the order returned.
4. Compare the `createdAt` values in that order: they are not monotonically non-increasing — at least one pair appears with the EARLIER `createdAt` listed first, because the summaries are ordered by `creation_seq` (claimed under the store lock, `src/ops/open_tunnel.rs:287`) while `createdAt` was sampled before the lock (`src/ops/open_tunnel.rs:165`, stored at `:300`).
5. Page the same listing with `{"maxResults": 1}` and follow `nextToken`: the inverted pair is paged in the same sequence order, so the client observes a page whose tunnel is documented as "newer" while carrying an older `createdAt` than the tunnel on the following page.
