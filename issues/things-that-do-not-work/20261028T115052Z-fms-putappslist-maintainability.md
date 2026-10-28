Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# The PreviousListVersion archival rule is hand-copied into PutAppsList and PutProtocolsList instead of living in crate::state

_Filed: 2026-10-28T11:50:52Z_
_Operation: PutAppsList | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "the version a write supersedes is one past what the archive already names, and it is archived only while that number still fits the model's `PreviousListVersion` key pattern" is implemented TWICE, verbatim, in two sibling handlers of this crate — including a second copy of the cap constant derived from the model (`^\d{1,2}$` -> 99). The two copies can drift: changing the cap, or changing how the superseded version number is computed, in one handler silently leaves the other list kind on the old rule, and nothing in the crate ties them together.

This is the last shared list rule still hand-copied. The crate's own convention is the opposite: `crate::state` is where a rule two handlers share is written once, and the apps-list / protocols-list twins of every OTHER shared rule already live there (`apps_list_in_namespace` / `protocols_list_in_namespace`, `delete_customer_apps_list` / `delete_customer_protocols_list`, `duplicate_tag_key_message`, `page_size`, `refresh_tag_index`).

The fix is a single generic helper in `crate::state` (one that takes the existing history map and the superseded array and applies the version/cap rule), called from both handlers. It changes no response and no existing test: both handlers already compute exactly the same value from exactly the same inputs.

## Evidence

Copy 1 — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_apps_list.rs:155 and :270-278:
```
const MAX_PREVIOUS_LIST_VERSION: usize = 99;
...
                let mut history = existing.list.previous_apps_list.clone().unwrap_or_default();
                let superseded_version = history.len() + 1;
                if superseded_version <= MAX_PREVIOUS_LIST_VERSION {
                    history.insert(
                        superseded_version.to_string(),
                        existing.list.apps_list.clone(),
                    );
                }
                list.previous_apps_list = Some(history);
```

Copy 2 — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_protocols_list.rs:156 and :272-284 (same rule, same cap, same computation):
```
const MAX_PREVIOUS_LIST_VERSION: usize = 99;
...
                let mut history = existing
                    .list
                    .previous_protocols_list
                    .clone()
                    .unwrap_or_default();
                let superseded_version = history.len() + 1;
                if superseded_version <= MAX_PREVIOUS_LIST_VERSION {
                    history.insert(
                        superseded_version.to_string(),
                        existing.list.protocols_list.clone(),
                    );
                }
                list.previous_protocols_list = Some(history);
```

Both copies justify the cap from the SAME modelled constraint, so there is one rule, not two — put_apps_list.rs:150-155: "its key is `com.amazonaws.fms#PreviousListVersion`, whose pattern is `^\d{1,2}$`, so a version number is at most two digits"; put_protocols_list.rs:151-156 repeats that sentence word for word.

The crate states the single-source-of-truth convention this breaks — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:7-11: "the helpers below are the bits of derivation several handlers share ... kept in one place so two handlers cannot disagree."

And it already applies that convention to the apps/protocols-list TWINS of the other list rules — state.rs:2029-2031: "[`Self::delete_customer_apps_list`] for protocols lists: the same namespace rule, so the two deleters cannot disagree about what a customer may remove."

Test coverage that must stay unchanged (both assert the archived version number and the archived array):
- tests/all/apps_list_lifecycle.rs:366-370 — `assert_eq!(updated["AppsList"]["PreviousAppsList"], json!({ "1": case.apps }), "...the superseded applications array must be archived as version 1...")`, and :388 for the second update;
- tests/all/protocols_list_lifecycle.rs:334 and :361 — the same two assertions for `PreviousProtocolsList`;
- tests/all/apps_list_reporting.rs:279 and :296 — a created list reports no history, an updated one reports `expected_history`.

## Steps to Reproduce

1. Read services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_apps_list.rs lines 150-155 and 264-278: the cap constant and the archival computation.
2. Read services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_protocols_list.rs lines 151-156 and 266-284: the same constant and the same computation, differing only in the member names `previous_protocols_list` / `protocols_list`.
3. Grep the crate for `MAX_PREVIOUS_LIST_VERSION`: it is declared twice, once per handler, and in no shared module — unlike `page_size`, `duplicate_tag_key_message`, `refresh_tag_index` and `delete_customer_apps_list` / `delete_customer_protocols_list`, which all live in src/state.rs.
4. Change the cap or the version computation in ONE of the two handlers: the crate still compiles and the other list kind keeps the old rule, which is the drift this duplication allows.
