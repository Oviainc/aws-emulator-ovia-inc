Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# The caller's policy document is re-parsed WHILE the store lock is held, breaking the rule both policy handlers state and the "Called ONCE" contract of policy_topology

_Filed: 2026-10-28T19:26:49Z_
_Operation: PutCoreNetworkPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutCoreNetworkPolicy` locks the store at line 163 and then, at line 273, calls `CoreNetworkRecord::add_policy_version(input.policy_document, ...)`. That function parses the caller-supplied document with `serde_json::from_str` — twice: once for the document being stored and once for the LIVE version's document (`policy_topology(&live.document)` / `policy_topology(&document)`, src/state.rs:1998-2003). `PolicyDocument` is modelled `len 0..10000000`, so this is up to 10 MB of caller-chosen JSON parsed under the single global store mutex, serialising every other request to this service for the duration.

That is exactly what the handler's own comment says it does NOT do, and the state module states the same contract twice:
* `put_core_network_policy.rs:144-148` — "The document is parsed and checked BEFORE the store is locked: it is the one part of this request whose size the caller chooses, so no other caller waits on it." The handler only runs `validate_policy_document` pre-lock; the topology/change-set derivation it leaves to `add_policy_version`, under the lock.
* `policy_topology`'s own doc (src/state.rs:7700-7702) — "Called ONCE, where the document is accepted, and the answer is stored on the record beside it" — is false: `add_policy_version` calls it twice per put and `execute_policy_version` calls it again.
* `CreateCoreNetwork` carries the same claim even more explicitly — "parsed, checked and expanded into its topology BEFORE the store is locked ... and it is derived exactly once here" — and then calls the same `add_policy_version` under its own lock, so the document it already expanded pre-lock is expanded a SECOND time while holding the mutex.

The crate demonstrably knows the distinction: `execute_policy_version` parses under the lock too, but carries an explicit justification for it — "the document is the one this service stored ... a value no caller of THIS request chose". On the put/create path the document IS the value this caller chose in this very request, so that justification does not apply.

## Evidence

The rule the handler states, and the lock-held call that breaks it — src/ops/put_core_network_policy.rs:144-148, :163, :273:
```
        // The document is parsed and checked BEFORE the store is locked: it is the one
        // part of this request whose size the caller chooses, so no other caller waits on
        // it. A document that does not hold up is refused with the modelled
        // `CoreNetworkPolicyException` (HTTP 400) and no version is minted, exactly as
        // `CreateCoreNetwork` refuses the document it carries.
...
        let mut store = state.lock();
...
        let stored = core_network.add_policy_version(
            input.policy_document,
```
What `add_policy_version` does with it — src/state.rs:1998-2003:
```
        let in_force = self
            .live_policy_version
            .and_then(|live| self.policy_versions.get(&live))
            .map(|live| policy_topology(&live.document))
            .unwrap_or_default();
        let changes = policy_changes(&in_force, &policy_topology(&document));
```
and `policy_topology` is a full JSON parse whose own doc claims it is called once — src/state.rs:7700-7705:
```
/// Called ONCE, where the document is accepted, and the answer is stored on the record
/// beside it: a read of a core network reports the derived topology and never re-parses
/// the document.
pub fn policy_topology(document: &str) -> PolicyTopology {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
```
The sibling create making the "exactly once" claim and then doing it again under the lock — src/ops/create_core_network.rs:143-149, :165, :286:
```
        // The policy document is parsed, checked and expanded into its topology BEFORE the
        // store is locked: it is the one part of this request whose size the caller
        // chooses, so no other caller waits on it, and it is derived exactly once here
        // rather than on every later read of the core network.
        let topology = match input.policy_document.as_deref() {
            Some(document) => match crate::state::validate_policy_document(document) {
                Ok(()) => Some(crate::state::policy_topology(document)),
...
        let mut store = state.lock();
...
            record.add_policy_version(document, None, None, now, true);
```
The crate's own statement of when lock-held parsing IS acceptable — src/state.rs:2051-2054:
```
        // The document is the one this service stored, already capped by
        // [`validate_policy_document`] where it was accepted, so deriving its topology here
        // is bounded work on a value no caller of THIS request chose.
        let topology = policy_topology(&version.document);
```
The member's modelled size, from the handler's own contract block (src/ops/put_core_network_policy.rs:47-48): "`PolicyDocument` (SynthesizedJsonCoreNetworkPolicyDocument, len 0..10000000 ...) **required**".

Test coverage: tests/all/core_network_policy_versions.rs and tests/all/core_network_lifecycle.rs exercise the put/create policy paths; neither pins where the document is parsed, so moving the derivation out of the lock changes no response.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a policy document
2. PutCoreNetworkPolicy on that core network with a large (multi-megabyte) PolicyDocument that `validate_policy_document` accepts
3. Read src/ops/put_core_network_policy.rs:163 → the store mutex is taken; line 273 then calls `add_policy_version`, which at src/state.rs:2001 and :2003 runs `policy_topology` (a `serde_json::from_str` of that same caller-supplied document, plus the LIVE document) with the mutex still held
4. Compare with the handler's own comment at src/ops/put_core_network_policy.rs:144-148 ("parsed and checked BEFORE the store is locked ... so no other caller waits on it") and with src/state.rs:7700-7702 ("Called ONCE, where the document is accepted") — both are contradicted
5. The same contradiction exists in CreateCoreNetwork, whose comment at src/ops/create_core_network.rs:145-146 says the topology "is derived exactly once here" while line 286 derives it again under the lock
