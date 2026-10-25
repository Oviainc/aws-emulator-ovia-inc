Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# Monitor.client_token is a second, never-read copy of the idempotency token whose doc claims it decides retries

_Filed: 2026-10-25T04:06:39Z_
_Operation: CreateMonitor | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::Monitor` carries a `client_token` field whose doc comment says it is "what makes a repeat of that exact request a retry rather than a conflict", and `CreateMonitor` fills it on every create. Nothing ever reads it: the retry/conflict decision is taken entirely from the separate `client_tokens` map via `client_token_record`. So the same fact — which token minted which monitor — is stored in two places in this crate, and only one of them is consulted.

The two copies can and do drift: `prune_client_tokens` (TTL / ceiling) and `forget_client_tokens_of_monitor` (on DeleteMonitor) remove the map entry only, leaving the monitor record still claiming a token that replays nothing. A later reader of `Monitor::client_token` — the field is `pub` — would reach the opposite conclusion from the code that actually decides. The handler module doc has the same ambiguity ("a RETRY only when it repeats the stored `clientToken`"), which reads as the field but is implemented against the map.

Fix direction: drop the unread copy (or make the single retry rule read it). Either way no response body and no existing test changes — no test and no response member mentions a monitor's stored client token.

## Evidence

The unread copy and its claim — `services/service-com-amazonaws-networkmonitor-networkmonitor/src/state.rs:716-719`:
```
    /// The `clientToken` of the `CreateMonitor` that made it, when one was
    /// given — what makes a repeat of that exact request a retry rather than a
    /// conflict.
    pub client_token: Option<String>,
```
written at `src/ops/create_monitor.rs:330`: `client_token: input.client_token.clone(),`.

The rule as actually implemented, against the OTHER copy — `src/ops/create_monitor.rs:248-252`:
```
        if let Some(token) = input.client_token.as_deref() {
            let replayed = store
                .client_token_record(&region, token)
                .filter(|record| {
                    record.probe_id.is_none() && record.monitor_name == input.monitor_name
```
backed by `src/state.rs:796-799` (`client_tokens: BTreeMap<(String, String), ClientTokenRecord>`), `src/state.rs:855-858` (`client_token_record`) and `src/ops/create_monitor.rs:337-347` (`store.record_client_token(...)`).

The drift: `src/state.rs:868-872` `forget_client_tokens_of_monitor` and `src/state.rs:1053-1068` `prune_client_tokens` retain/remove entries of `self.client_tokens` only; neither touches `Monitor::client_token`.

Proof that the field is read nowhere: a crate-wide grep for `client_token` matches only the two type definitions (`src/ops/types/create_monitor_types.rs:20`, `src/ops/types/create_probe_types.rs:19`), the `client_tokens` map/helpers in `src/state.rs`, the writes/lookups in `src/ops/create_monitor.rs` and `src/ops/create_probe.rs`, and `src/ops/delete_monitor.rs:165` — no read of `monitor.client_token`. No test file references it either (`tests/all/monitor_and_probe.rs:665` and `tests/all/region_isolation.rs:408` exercise the token through the request only).

## Steps to Reproduce

1. Read src/state.rs:716-719 — the field doc states it is what makes a repeat create a retry.
2. Read src/ops/create_monitor.rs:248-258 — the retry decision reads store.client_token_record (the client_tokens map), never monitor.client_token.
3. grep -rn 'client_token' services/service-com-amazonaws-networkmonitor-networkmonitor — the only occurrence of the monitor's own field is the write at src/ops/create_monitor.rs:330 plus the declaration; there is no read.
4. Note the drift path: after prune_client_tokens drops the map entry (TTL or MAX_CLIENT_TOKENS) the monitor record still carries the token, so the two stored copies of the same fact disagree.
