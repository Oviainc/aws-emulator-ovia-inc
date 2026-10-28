Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: d9dd770f9e2574e0600226bd25d044e51daa0c38

# Delete the inlined `unanchored` pattern renderer in favour of `service-common`'s

_Filed: 2026-10-28T05:07:16Z_
_Operation: GetRule | Class: maintainability_

> _Note: this issue was identified against the code base at commit `d9dd770f9e2574e0600226bd25d044e51daa0c38`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` carries its own private copy of the "print a `@pattern` without the `^`/`$` anchors" rule (`fn unanchored`), which every handler's pattern refusal is rendered through (the `crate::service` validation gate -> `constraint_violations` -> `constraint_clause` -> `unanchored`). The workspace crate `service-common` (in `Cargo.lock`) already owns that exact rendering as `validation::unanchored`, and its own module documentation states that the inlined copies are to be deleted in favour of it and that nothing new should inline the rule again. This crate's copy even says the same thing about itself. Two copies of one rendering rule can drift, and they ALREADY have: the local copy strips a leading `^` even when there is no trailing `$` (`^abc` -> `abc`) and, for an escaped trailing `$`, returns the body with the `^` already removed, while the shared one drops the pair POSITIONALLY and returns the regex verbatim unless BOTH ends carry an anchor. Every `@pattern` in the rbin model is double-anchored (`^[0-9a-zA-Z]{11}$`, `^[\S ]{0,255}$`, `^arn:\S+:rbin:...$`, `^[A-Za-z0-9+/=]{1,2048}$`), so the two implementations agree on every string this service can render: switching `crate::state::constraint_clause` to call `service_common::validation::unanchored` (adding the already-locked `service-common` dependency) leaves every response byte and every existing test unchanged, and removes the second copy of the rule.

## Evidence

This crate's copy, which admits it is a copy to be deleted:

services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:974-992
```
/// MIGRATION NOTE, as `services/common/src/validation.rs` ("Still to migrate here") asks
/// of anything that inlines this again: this rule — like the subject half and the
/// requirement half of a clause — is the same for every service and belongs in that
/// shared module; it is inlined here because the migration is one change of its own, and
/// this copy is to be deleted by it rather than copied further.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => { ... }
        None => body,
    }
}
```
Its only caller, on the path every operation's pattern refusal takes:
services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:948 — `ViolationKind::Pattern(unanchored(pattern).to_string())`

The shared implementation that already exists, in a crate present in `Cargo.lock` (`Cargo.lock:15962` — `name = "service-common"`):

services/common/src/validation.rs:641-682
```
/// THE STAGED HOME for this rendering, which the module docs above list as one of the parts
/// of a validation message that are AWS's rather than any one service's. ~26 service crates
/// carry their own inlined copy (`grep -rln 'fn unanchored' services/*/src`), each pinned by
/// that service's own recordings; those copies are to be DELETED in favour of this one by the
/// change that owns that migration, and nothing new should inline the rule again.
pub fn unanchored(regex: &str) -> String {
    let Some(body) = regex.strip_prefix('^') else {
        return regex.to_string();
    };
    ...
}
```
services/common/src/validation.rs:103-107 — "LANDED here, while ~26 service crates still carry their own inlined copy ... deleting those copies in favour of this one is the migration's own change, and nothing new should inline the rule again."

Existing tests that pin the rendering (and must stay green, unchanged):
- services/service-com-amazonaws-rbin-amazonrecyclebin/tests/all/error_response_bodies.rs:66 `const RULE_IDENTIFIER_PATTERN: &str = "[0-9a-zA-Z]{11}";` with rows 232-241 ("GetRule: an identifier shorter than the pattern allows"), 242-251 (DeleteRule), 252-261 (UpdateRule), 312-324 (ListRules `NextToken`), 325-347 (TagResource / ListTagsForResource `RULE_ARN_PATTERN`), each asserting `Requirement::Exact("Member must satisfy regular expression pattern: ...")`.
- services/service-com-amazonaws-rbin-amazonrecyclebin/tests/all/retention_rule_fidelity.rs:579-582 — `assert!(!message.contains('^') && !message.contains('$'), "case {case:?}: a reported pattern carries no ^/$ anchors, ...")`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:979 and read `fn unanchored`, together with the MIGRATION NOTE above it.
2. Open services/common/src/validation.rs:674 and read `pub fn unanchored`, and the module note at lines 99-110 stating the inlined service copies are to be deleted in favour of it.
3. Confirm the shared crate is already in the build: `grep -n 'name = "service-common"' Cargo.lock` -> line 15962.
4. Observe both are reached for the same purpose: state.rs:948 renders a `ViolationKind::Pattern` through the local copy for every operation, e.g. `GET /rules/short` (error_response_bodies.rs:232-241) whose message must read `... Member must satisfy regular expression pattern: [0-9a-zA-Z]{11}`.
5. Note every `@pattern` in smithy-model/com-amazonaws-rbin-amazonrecyclebin.json is anchored at BOTH ends, so the local copy and the shared one produce identical text for this service — the duplication has no response impact today, only drift risk (the copies already disagree on one-sided anchors such as `^abc`).
