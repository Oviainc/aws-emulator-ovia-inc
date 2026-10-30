Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0727557aa604deafa81f0cee75b9e46b63ce964

# A successful create overwrites the clientToken record of an earlier create that used the same token, so the earlier resource loses its idempotency and a legitimate retry conflicts or duplicates

_Filed: 2026-10-29T05:32:26Z_
_Operation: CreateMonitor | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0727557aa604deafa81f0cee75b9e46b63ce964`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`client_tokens` is keyed by `(region, clientToken)` and `record_client_token` writes it with `BTreeMap::insert`, so a second SUCCESSFUL create carrying a token some earlier create already recorded silently REPLACES that earlier record. Because a monitor and a probe keep no token of their own (the map is, by its own doc, "The ONLY record of which token minted which resource"), the earlier resource's idempotency is destroyed — permanently, with no way to recover it.

Two observable consequences:

1. CreateMonitor(name=A, token=T) succeeds; CreateMonitor(name=B, token=T) succeeds (B is a fresh name, so the replay lookup correctly misses) and clobbers T's record from A to B. A later RETRY of the first call — byte-identical, same token T, which is exactly what the handler doc says makes a repeat a retry — now misses (`monitor_minted_by_token` requires `record.monitor_name == monitor_name`) and is answered 409 ConflictException instead of replaying monitor A.

2. Worse, across the two create kinds: CreateProbe(monitor=M, token=T) mints probe P and records T -> (M, P); CreateMonitor(name=N, token=T) then succeeds and rewrites T -> (N, none). A retry of the CreateProbe now misses `probe_minted_by_token` and MINTS A SECOND PROBE — the idempotency token fails at the one thing it exists for, and a duplicate resource is created.

This contradicts the store's own doc, which enumerates the only ways a token record is forgotten — "TTL, ceiling, or the delete of the monitor it named" — and does not include "overwritten by an unrelated create". It is a distinct defect from the already-fixed "replay reports a stale state" issue, which was about the replay path skipping `settle_monitor`; here the replay record itself is destroyed, so no replay happens at all.

## Evidence

The store's own statement of when a record is forgotten — services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs:1188-1196:
```
    /// (region, `clientToken`) -> what it replays. A token is scoped to the
    /// region it was sent to, like the monitor it minted. Bounded by
    /// [`MAX_CLIENT_TOKENS`] and [`CLIENT_TOKEN_TTL`].
    ///
    /// The ONLY record of which token minted which resource: a monitor and a
    /// probe keep no token of their own, so there is nothing for this map to
    /// disagree with once a record is forgotten (TTL, ceiling, or the delete of
    /// the monitor it named).
    client_tokens: BTreeMap<(String, String), ClientTokenRecord>,
```
The fourth, unlisted way a record is forgotten — src/state.rs:1303-1307:
```
    /// Record what a `clientToken` sent to `region` replays.
    pub fn record_client_token(&mut self, region: &str, token: String, record: ClientTokenRecord) {
        self.client_tokens
            .insert((region.to_string(), token), record);
    }
```
`insert` on an occupied key discards the previous `ClientTokenRecord`; there is no occupancy check at either call site — src/ops/create_monitor.rs:386-396 (`store.record_client_token(&region, token, ClientTokenRecord { monitor_name: monitor.monitor_name.clone(), probe_id: None, recorded_at: now });`) and src/ops/create_probe.rs:340-350 (the same call with `probe_id: Some(probe_id.clone())`).

What the clobbered record then costs the caller — src/state.rs:1272-1283:
```
    pub fn monitor_minted_by_token(
        &self, region: &str, token: &str, monitor_name: &str,
    ) -> Option<&Monitor> {
        let record = self.client_token_record(region, token)?;
        if record.probe_id.is_some() || record.monitor_name != monitor_name {
            return None;
        }
```
and src/state.rs:1289-1301 (`probe_minted_by_token`, same `record.monitor_name != monitor_name` gate), against the handler's own promise — src/ops/create_monitor.rs:12-16:
```
//! A second create naming a monitor that already exists is a RETRY only when it
//! repeats the idempotency token recorded for that create
//! ([`...::monitor_minted_by_token`],
//! which reads the one record of which token minted which resource); any other
//! repeat is refused with `ConflictException`.
```
A retry of create A DOES repeat the token recorded for that create, yet is refused.

Test coverage: the clobbering sequence is already exercised but its consequence is never read back — tests/all/client_token_replay.rs:92-99:
```
        // The same token under ANOTHER name replays nothing: it minted the
        // monitor above, not whatever the caller asks for next.
        let other = format!("tok-{index}-other");
        let (status, fresh) = create_monitor(&router, &monitor_body(&other, &token, 60)).await;
        assert_eq!(status, 200, ...);
```
The test retries the FIRST create at lines 82-90, i.e. BEFORE this second create, so it never observes the destroyed record. Keeping the first record (first-writer-wins) leaves this assertion as it is: the create under `other` still misses the replay lookup (the names differ) and still answers 200 with the new name and period. tests/all/client_token_replay.rs:133-155 (`a_repeat_create_that_replays_no_monitor_token_conflicts`) is also unaffected, since each of its three cases is refused 409 before `record_client_token` is reached.

## Steps to Reproduce

1. POST /monitors with {"monitorName":"A","aggregationPeriod":30,"clientToken":"T"} — 200.
2. Repeat the identical call — 200, replayed (the token record still names A).
3. POST /monitors with {"monitorName":"B","aggregationPeriod":60,"clientToken":"T"} — 200, monitor B is created and T's record is overwritten to B.
4. Repeat step 1's identical call again — expected a replay of monitor A (create_monitor.rs:12-16: a repeat "repeat[ing] the idempotency token recorded for that create" is a retry); observed 409 ConflictException.
5. Cross-kind variant: POST /monitors/{M}/probes with clientToken="T2" — 200, probe P minted. Then POST /monitors with {"monitorName":"N","clientToken":"T2"} — 200, T2's record is overwritten. Then repeat the CreateProbe call verbatim — a SECOND probe is minted under M instead of replaying P, so GetMonitor(M) lists two identical probes for one idempotency token.
