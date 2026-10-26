Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# Hoist the "omit an empty tags member" inquiry projection into one place instead of three handler copies

_Filed: 2026-10-25T10:50:07Z_
_Operation: CreateComplianceInquiry | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three inquiry handlers in this crate each write out the same projection rule by hand — "an untagged resource reports no `tags` member at all, rather than an empty map" — as the expression `(!inquiry.tags.is_empty()).then(|| inquiry.tags_map())`.

The crate already centralises the *map build* on the record (`ComplianceInquiry::tags_map`, src/state.rs:1124) but not the *omit-when-empty* decision, so that decision exists three times. The comments above the three copies each assert it is ONE projection ("the same projection `CreateComplianceInquiry` returns", "the same projection every other inquiry operation returns"), which is precisely the invariant three independent copies can break: changing one of them — e.g. to emit `Some(empty_map)` — makes the create, the metadata read and the export disagree about an untagged inquiry while every other copy keeps the old answer.

The crate has already applied this exact remedy twice for the same reason: the inquiry NAME rule is hoisted into `ComplianceInquiry::display_name` ("One rule, applied in one place, so the create, the listing, the metadata read and the export cannot disagree about what an inquiry is called", src/state.rs:1085-1087), and the five-times-duplicated page walk was hoisted into `crate::state::walk_page` (src/state.rs:2381) by the already-fixed issue `20261024T223115Z-artifact-listreports-maintainability`.

Fix: add one accessor beside `tags_map` (e.g. `ComplianceInquiry::reported_tags(&self) -> Option<HashMap<String,String>>`) that owns the omit-when-empty decision, and have the three handlers call it. Every response byte stays identical and no existing test changes.

## Evidence

Copy 1 — services/service-com-amazonaws-artifact-artifact/src/ops/create_compliance_inquiry.rs:340-341:

    // An untagged resource reports no `tags` member at all, rather than an empty map.
    tags: (!inquiry.tags.is_empty()).then(|| inquiry.tags_map()),

Copy 2 — services/service-com-amazonaws-artifact-artifact/src/ops/get_compliance_inquiry_metadata.rs:188-190:

    // An untagged resource reports no `tags` member at all, rather than an empty
    // map — the same projection `CreateComplianceInquiry` returns.
    tags: (!inquiry.tags.is_empty()).then(|| inquiry.tags_map()),

Copy 3 — services/service-com-amazonaws-artifact-artifact/src/ops/export_compliance_inquiry.rs:203-205 (and the claim it repeats at 265-266):

    // Read off the one stored record, so the tags this answers with are the tags the
    // create and the metadata read report.
    let tags = (!inquiry.tags.is_empty()).then(|| inquiry.tags_map());
    ...
    // An untagged resource reports no `tags` member at all, rather than an empty
    // map — the same projection every other inquiry operation returns.
    tags,

The helper the copies stop short of — services/service-com-amazonaws-artifact-artifact/src/state.rs:1123-1129:

    /// The inquiry's tags, in the `TagsMap` shape every operation returns them in.
    pub fn tags_map(&self) -> ::std::collections::HashMap<String, String> {

Precedent inside the same crate that a shared projection belongs in one place — src/state.rs:1085-1087:

    /// only for content that derives none. One rule, applied in one place, so the
    /// create, the listing, the metadata read and the export cannot disagree about what
    /// an inquiry is called.

Test coverage that pins the behaviour the hoist must not change:
- tests/all/compliance_inquiry_export.rs:229 — "untagged submission, because an untagged resource reports no `tags` member at all"; lines 470-482 assert the export's `tags` equal the metadata read's.
- tests/all/resource_tags.rs:15 — "the member being absent"; line 594-607 asserts the absent-member case.
- tests/all/compliance_inquiry_lifecycle.rs:442 and :472 — assert the create's `tags` and the metadata read's `tags` agree.

## Steps to Reproduce

1. grep -n '(!inquiry.tags.is_empty()).then' services/service-com-amazonaws-artifact-artifact/src/ops/ — three hits, in create_compliance_inquiry.rs, get_compliance_inquiry_metadata.rs and export_compliance_inquiry.rs.
2. Observe that services/service-com-amazonaws-artifact-artifact/src/state.rs:1124 already owns the map build (`tags_map`) but not the omit-when-empty decision, so the decision is duplicated three times.
3. Edit only ONE of the three copies to `Some(inquiry.tags_map())` and run `cargo test -p service-com-amazonaws-artifact-artifact`: the untagged-resource assertions fail for that operation alone while the other two operations keep answering the old way — the drift the three copies permit.
4. Revert, add `ComplianceInquiry::reported_tags()` beside `tags_map()` in src/state.rs, call it from all three handlers, and re-run the suite: every test passes unchanged.
