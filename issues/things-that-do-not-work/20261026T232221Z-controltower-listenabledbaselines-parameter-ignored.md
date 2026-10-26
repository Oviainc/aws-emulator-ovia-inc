Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# ListEnabledBaselines drops includeChildren and can never report an inherited (child) enabled baseline

_Filed: 2026-10-26T23:22:21Z_
_Operation: ListEnabledBaselines | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`includeChildren` is a modelled input member of `ListEnabledBaselines` documented as a switch that adds the child enablements to the response. The handler never reads `input.include_children` at all: the response is byte-identical for `includeChildren: true` and `includeChildren: false`. The matching output member `parentIdentifier` is hardcoded `None` for every summary, and the `filter.parentIdentifiers` member is matched with `selected_optional(&parent_identifiers, None)`, which by that helper's own definition can only ever select nothing — so a caller filtering on a parent identifier always receives an empty list. The module header admits the gap in so many words rather than leaving it unstated, which is what makes this a defect in existing code rather than an unattempted feature.

## Evidence

Model — `smithy_query(query="doc", shape="ListEnabledBaselinesInput", member="includeChildren")`:
```
"A value that can be set to include the child enabled baselines in responses. The default value is false."
```
and `EnabledBaselineDetails$parentIdentifier`:
```
"An ARN that represents the parent EnabledBaseline at the Organizational Unit (OU) level, from which the child EnabledBaseline inherits its configuration. The value is returned by GetEnabledBaseline."
```
OPERATIONS.md:107 lists the member as part of the input: `includeChildren` (boolean, default false, required).

Handler admits the gap — services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/list_enabled_baselines.rs:11-17:
```
//! One filter member selects by something this service holds no value for. An
//! `EnabledBaseline` here is applied directly to its target and this service records no
//! organizational hierarchy, so no enablement is the child of another one
//! (`parentIdentifier` absent) and `includeChildren` — which asks for the enablements the
//! target's member accounts inherited — widens the listing by nothing.
```
The member is never consulted: `grep include_children src/` matches only the input struct definition (src/ops/types/list_enabled_baselines_types.rs:20) — no handler reads it.

The filter can never match — src/ops/list_enabled_baselines.rs:233 `&& selected_optional(&parent_identifiers, None)` against src/state.rs:3087-3091:
```
pub fn selected_optional(filter: &Filter<'_>, actual: Option<&str>) -> bool {
    match (filter.as_ref(), actual) {
        (None, _) => true,
        (Some(_), None) => false,
```
and the summary hardcodes the parent — src/ops/list_enabled_baselines.rs:289-290:
```
// Every enablement here is an OU-level one, so none inherits from a parent.
parent_identifier: None,
```

Test coverage: tests/all/list_enabled_baselines.rs:433-434 pins the no-op ("includeChildren, which widens the listing by nothing") and :417-418 pins that `parentIdentifiers` selects nothing — the current behaviour is encoded in the suite, and no test asserts an inherited child enablement. (The same gap on the control side is already tracked separately as 20261024T074110Z-controltower-listenabledcontrols-parameter-ignored; this is the baseline-side operation, untracked.)

## Steps to Reproduce

1. POST /create-landingzone with a valid manifest and wait past the settle span.
2. POST /enable-baseline with {"baselineIdentifier":"<a catalog baseline ARN>","baselineVersion":"4.0","targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"} and wait past the settle span.
3. POST /list-enabled-baselines with {"includeChildren":true} and then with {"includeChildren":false} — the two responses are identical; no child/inherited enablement is ever reported and no summary carries parentIdentifier.
4. POST /list-enabled-baselines with {"filter":{"parentIdentifiers":["<the EnabledBaseline ARN>"]}} — always an empty enabledBaselines list, because selected_optional is called with a hardcoded None.
