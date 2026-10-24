Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# ListEnabledControls ignores includeChildren and never reports controls inherited by child OUs/accounts

_Filed: 2026-10-24T07:41:10Z_
_Operation: ListEnabledControls | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`includeChildren` is a modeled input member (Boolean, default false) documented as "Specifies whether to include enabled controls from child organizational units and child accounts in the response", and the operation itself is documented as listing "the controls enabled by AWS Control Tower on the specified organizational unit AND THE ACCOUNTS IT CONTAINS". The handler reads the member nowhere: the response is byte-identical for includeChildren=true and includeChildren=false.

Expected (real AWS): a control enabled on an OU applies to every account and child OU that OU contains. Listing with `targetIdentifier` = the OU and `includeChildren=true` returns the OU-level enablement PLUS one entry per inherited child enablement, each carrying `parentIdentifier` set to the OU-level `EnabledControl` ARN (which is also what the `filter.parentIdentifiers` filter is for); with `includeChildren=false` only the enablement on the named target is returned. Listing with `targetIdentifier` = an account inside that OU returns the inherited enablement.

Actual (emulator): `includeChildren` is destructured away and never consulted (the handler's own comment says it "adds nothing to this set because this service stores no hierarchy to widen it with"), and `EnableControl` always stores `parent_identifier: None`, so no inherited enablement ever exists. Consequently includeChildren=true adds nothing, `filter.parentIdentifiers` can never select anything, and a listing scoped to an account inside a governed OU returns an empty `enabledControls` list where AWS returns the inherited control.

## Evidence

Smithy model: ListEnabledControlsInput.includeChildren (target Boolean, default false) with the doc quoted above; the operation doc states it lists the controls enabled "on the specified organizational unit and the accounts it contains"; `EnabledControlSummary$parentIdentifier`/`EnabledControlDetails$parentIdentifier` are documented as "the ARN of the parent enabled control from which this control inherits its configuration", and `EnabledControlFilter$parentIdentifiers` exists to filter on it — the model therefore models control inheritance as observable output. OU-level inheritance of Control Tower controls is well-established service behavior.

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/list_enabled_controls.rs lines 224-264 — the filter sets are built from `filter.*` and the only target narrowing is `control.target_identifier == target`; `input.include_children` is never read (see the comment at lines 244-247). src/ops/enable_control.rs line 321 hardcodes `parent_identifier: None`, and src/state.rs:290-295 documents it as "None for every control this emulator enables".

Tests: tests/all/enabled_control_lifecycle.rs lines 412-418 deliberately pin the current no-op behavior ("includeChildren true adds nothing: no stored control is a child of another") and line 492 pins that `filter.parentIdentifiers` selects nothing; line 239 asserts summaries carry no `parentIdentifier`. No test asserts inherited child enablements or an includeChildren-dependent response, so the divergence is encoded in the suite rather than covered.

## Steps to Reproduce

1. POST /create-landingzone with a manifest whose governedRegions/organizationStructure describe an OU, and wait past the settle span.
2. POST /enable-control with {"controlIdentifier":"arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES","targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"} (an OU target).
3. POST /list-enabled-controls with {"targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a","includeChildren":true} and then with includeChildren:false — the emulator returns exactly the same single entry for both; AWS returns additional inherited entries (child accounts/OUs, each with parentIdentifier set) for includeChildren=true.
4. POST /list-enabled-controls with {"targetIdentifier":"arn:aws:organizations::123456789012:account/o-exampleorg/123456789012"} for an account inside that OU — emulator returns an empty enabledControls list; AWS returns the control inherited from the parent OU.
5. POST /list-enabled-controls with {"filter":{"parentIdentifiers":["<the OU-level EnabledControl ARN>"]} } — emulator always returns an empty list because no stored control ever carries a parentIdentifier.
