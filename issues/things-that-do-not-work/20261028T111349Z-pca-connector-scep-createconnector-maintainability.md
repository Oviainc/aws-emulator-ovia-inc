Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: fa255a5b72d32034e1d300bb9323da879d85edac

# De-duplicate the test harness: the pending-transition clock move is reimplemented per suite and the copies have already diverged

_Filed: 2026-10-28T11:13:49Z_
_Operation: CreateConnector | Class: maintainability_

> _Note: this issue was identified against the code base at commit `fa255a5b72d32034e1d300bb9323da879d85edac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`tests/all.rs` compiles all 16 suites into ONE test crate, and each suite re-implements the same harness logic privately instead of sharing one module: the pending-transition clock move (`set_transition_due` / `release_pending` / `hold_pending`), the `@httpLabel` percent-encoding rule (`label`), the signed-request builder (`async fn call`) and `new_router`. `label()` appears in 15 suites, `call()` in 15, `new_router()` in 13, `release_pending()` in 11, `set_transition_due()` in 6.

These are not repeated literals: each copy encodes a rule (which bytes are unreserved in a path label; how to move `PendingTransition::due_at` created by `create_connector.rs` without touching anything else on the record). The copies can drift, and two of them ALREADY have: one copy treats "nothing is owed any more" as a tolerated outcome and returns `false`, while another panics on exactly that state. A suite written against the tolerant contract and later copied from the panicking one silently changes what a case does.

The fix is a pure extraction into one shared `tests/all/<harness>.rs` module that the suites use: no handler, no response and no assertion changes, and every existing test keeps passing unchanged.

## Evidence

Two copies of the same clock-move rule, already divergent, in one test crate:

`tests/all/resource_tags.rs:84-107` — tolerant copy:
```
/// settling it. Answers `false` when nothing is owed any more (the record already settled, or
/// a teardown already completed), so a case about a transient state can skip its assertion
fn set_pending_due(...) -> bool {
    match store.connectors.get_mut(arn).and_then(|connector| connector.pending.as_mut()) {
        Some(pending) => { pending.due_at = due_at; true }
        None => false,
    }
}
```
`tests/all/resource_tags.rs:111-117`: `fn release_pending(...) { set_pending_due(engine, arn, std::time::SystemTime::now() - Duration::from_secs(1)); }`

`tests/all/challenge_metadata.rs:193-205` — panicking copy of the same rule:
```
fn release_pending(engine: &engine::InMemoryEngine, arn: &str) {
    let connector = store.connectors.get_mut(arn)
        .unwrap_or_else(|| panic!("the connector {arn:?} that was just created"));
    let pending = connector.pending.as_mut()
        .unwrap_or_else(|| panic!("the transition the clock owes {arn:?}"));
    pending.due_at = SystemTime::now() - Duration::from_secs(1);
}
```
Same body again at `tests/all/connector_deletion.rs:229-241` (`set_transition_due`), `tests/all/challenge_deletion.rs:175-192`, `tests/all/challenge_client_token.rs:184-204`, `tests/all/connector_delete_not_found.rs:162-177`, `tests/all/connector_lifecycle.rs:115-143`, `tests/all/resource_arn_validation.rs:110`, `tests/all/tag_resource.rs:95`.

The label rule, copied verbatim 15 times, e.g. `tests/all/challenge_deletion.rs:72-82` and `tests/all/connector_deletion.rs:101-111`:
```
fn label(value: &str) -> String {
    value.bytes().map(|byte| match byte {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
        other => format!("%{other:02X}"),
    }).collect()
}
```

The suites all live in one crate target, so a shared module is already reachable: `tests/all.rs:14-45` declares every one of them with `#[path = "all/<name>.rs"] mod <name>;`.

What it moves, on the service side: `PendingTransition { outcome, due_at }` is created by `src/ops/create_connector.rs:335-340` and materialised by `src/state.rs:278-297` (`StoredConnector::settle`).

Test coverage of the extracted code: every suite named above — `connector_deletion`, `challenge_deletion`, `challenge_client_token`, `challenge_metadata`, `resource_tags`, `tag_resource`, `connector_lifecycle`, `connector_delete_not_found`, `resource_arn_validation` — exercises these helpers on every run, so an extraction that keeps them passing is proof the behaviour is unchanged.

## Steps to Reproduce

1. grep -n '^fn label(value: &str) -> String {' services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/tests/all/*.rs — 15 identical definitions in one test crate
2. grep -n '^fn release_pending\|^fn set_transition_due\|^fn set_pending_due' services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/tests/all/*.rs — 11 + 6 + 1 copies of the same clock move
3. Open tests/all/resource_tags.rs:87-117 and tests/all/challenge_metadata.rs:193-205 side by side: the first returns `false` when no transition is owed, the second panics on the same state — the two copies of one rule have already drifted
4. Confirm all the suites are one crate target: tests/all.rs:14-45 declares each file with `#[path = "all/..."] mod ...;`, so a single shared harness module is reachable without changing any assertion
