Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 1b771983778e24aa6377b2fcf224b11c2656e2db

# Hoist the "resolve the inquiry a request targets, refuse with the front door's 401, then materialise" rule into one place instead of four handler copies

_Filed: 2026-10-29T13:44:23Z_
_Operation: GetComplianceInquiryMetadata | Class: maintainability_

> _Note: this issue was identified against the code base at commit `1b771983778e24aa6377b2fcf224b11c2656e2db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate hoists every rule that more than one handler needs into `crate::state`, and it says so: the two-stage report-target rule lives in `catalog_report`/`report_version` precisely "so the operations that need only this stage and the ones that go on to resolve a VERSION cannot disagree about the same `reportId`", and the taggable-resource lookup lives in `tagged_resource`/`tagged_resource_mut`.

The COMPLIANCE-INQUIRY counterpart of that same rule is not hoisted. Four handlers each write the identical three-step sequence inline:

1. `store.inquiries.get_mut(&input.compliance_inquiry_id)`;
2. on `None`, `return Err(crate::scaffolded::Scaffolded::unauthorized())` — the front door's 401 rather than the modelled 404;
3. on `Some`, `inquiry.materialise(now)` — the clock-owed transition a reader may apply.

The copies live in `get_compliance_inquiry_metadata.rs`, `list_compliance_inquiry_queries.rs`, `export_compliance_inquiry.rs` and `put_compliance_inquiry_feedback.rs`. Because the rule is written out four times, the four can drift: a fifth inquiry-targeting operation (or an edit to one of these) can resolve the inquiry without the 401 refusal, or without `materialise`, and nothing in the crate would catch it — which is exactly the drift the report side was hoisted to prevent, and which `crate::state`'s own comment says the two sides must not differ on ("an id the account holds no inquiry for is answered with the front door's 401 ... exactly as a `reportId` the catalog does not publish is").

The fix is the one the crate already applies on the report side: one `ComAmazonawsArtifactArtifactState` method (e.g. `targeted_inquiry_mut(&mut self, id: &str, now: SystemTime) -> Result<&mut ComplianceInquiry, crate::scaffolded::FrontDoorRefusal>`) that does the lookup, hands back the refusal for the caller to lift (exactly as `catalog_report` already does), and materialises the record before returning it. Every handler then states one call instead of three steps. No response changes and no test changes: the status, the wire code, the message and the materialised projection are all identical.

NOTE this is strictly the duplication, not the behaviour: the 401-instead-of-404 choice and the materialise-on-read choice are the crate's stated rules (and the 401 choice is pinned by a fixed issue), so the hoist must preserve both exactly.

## Evidence

The precedent the crate already states — services/service-com-amazonaws-artifact-artifact/src/state.rs:1672-1683:
```
    /// Decided here so the operations that need only this stage and the ones that go on to
    /// resolve a VERSION ([`Self::report_version`]) cannot disagree about the same
    /// `reportId`. The refusal is handed back for the caller to lift into its own error
    /// channel, exactly as [`resume_position`] hands back its own.
    pub fn catalog_report(
        &self,
        report_id: &str,
    ) -> Result<&Report, crate::scaffolded::FrontDoorRefusal> {
        self.reports
            .get(report_id)
            .ok_or_else(crate::scaffolded::FrontDoorRefusal::unauthorized)
    }
```

The crate stating that the inquiry side obeys the SAME rule — src/state.rs:2422-2427:
```
// There is deliberately no not-found payload for a compliance inquiry id: an id the
// account holds no inquiry for is answered with the front door's 401
// `UnauthorizedException` (`crate::scaffolded::Scaffolded::unauthorized`) rather than the
// modelled 404, exactly as a `reportId` the catalog does not publish is — the gateway
// authorizes the target before the state is consulted, so the absence of a resource the
// caller may not read is never confirmed.
```

Copy 1 — src/ops/get_compliance_inquiry_metadata.rs:174-184:
```
        let mut store = state.lock();
        let Some(inquiry) = store.inquiries.get_mut(&input.compliance_inquiry_id) else {
            // Authorization of the targeted inquiry precedes the state lookup as far as a
            // caller can tell: ...
            return Err(crate::scaffolded::Scaffolded::unauthorized());
        };

        // The only write a read may make: the transition the clock had already made due.
        inquiry.materialise(now);
```

Copy 2 — src/ops/list_compliance_inquiry_queries.rs:200-214:
```
        let mut store = state.lock();
        let Some(inquiry) = store.inquiries.get_mut(&input.compliance_inquiry_id) else {
            ...
            return Err(crate::scaffolded::Scaffolded::unauthorized());
        };

        // The only write a read may make: the transition the clock had already made due.
        ...
        inquiry.materialise(now);
```

Copy 3 — src/ops/export_compliance_inquiry.rs:191-203:
```
        let Some(inquiry) = store.inquiries.get_mut(&input.compliance_inquiry_id) else {
            ...
            return Err(crate::scaffolded::Scaffolded::unauthorized());
        };

        // The only write this operation makes to the INQUIRY: the transition the clock
        // had already made due. ...
        inquiry.materialise(now);
```

Copy 4 — src/ops/put_compliance_inquiry_feedback.rs:233-242:
```
        let Some(inquiry) = store.inquiries.get_mut(&input.compliance_inquiry_id) else {
            ...
            return Err(crate::scaffolded::Scaffolded::unauthorized());
        };

        // The only clock-owed write: the transition this inquiry was already due.
        inquiry.materialise(now);
```

The sibling mutable lookup that IS hoisted, showing the shape the inquiry rule should take — src/state.rs:1713-1717:
```
    pub fn tagged_resource_mut(&mut self, resource_arn: &str) -> Option<&mut ComplianceInquiry> {
        self.inquiries
            .values_mut()
            .find(|inquiry| inquiry.arn == resource_arn)
    }
```

Test coverage that pins the behaviour the hoist must leave unchanged — tests/all/implemented_inquiry_operations.rs:6-11:
```
//! 1. A well-formed `InquiryId` the account holds no inquiry for names no inquiry the
//!    caller may reach, so EVERY operation targeting an inquiry answers it with the front
//!    door's HTTP 401 `UnauthorizedException` and that gateway's constant `Unauthorized`
//!    text — not a modelled 404, which would confirm the resource's absence. The judgement
//!    is on the TARGET, which is what the same four operations serving an inquiry the
//!    account does hold is the evidence for.
```
with `const UNAUTHORIZED: (u16, &str, &str) = (401, "UnauthorizedException", "Unauthorized");` (line 41), the four routes driven by `every_inquiry_operation` (lines 97, 105, 114, 124 — `getMetadata`, `listQueries`, `export`, `putFeedback`) and the two tests `every_inquiry_operation_refuses_a_well_formed_id_the_account_holds_no_inquiry_for` (line 236) and `every_inquiry_operation_serves_an_inquiry_the_account_holds` (line 267). The materialise half is pinned by tests/all/inquiry_terminal_status_rule.rs (`an_inquiry_reports_what_became_of_the_content_it_was_submitted_with`, line 329; `only_a_submission_with_nothing_to_process_reports_a_processing_failure`, line 430), which reads status through `metadata()` (line 174) and `queries()` (line 191). All of these keep passing unchanged under the hoist.

## Steps to Reproduce

1. grep -n "store.inquiries.get_mut(&input.compliance_inquiry_id)" services/service-com-amazonaws-artifact-artifact/src/ops/*.rs — four hits, one per inquiry-targeting handler.
2. For each hit, read the next ~12 lines: all four follow it with the same `return Err(crate::scaffolded::Scaffolded::unauthorized())` on absence and the same `inquiry.materialise(now)` on presence.
3. grep -n "pub fn catalog_report\|pub fn report_version\|pub fn tagged_resource_mut" services/service-com-amazonaws-artifact-artifact/src/state.rs — the report-target and tag-target lookups each live in exactly one place, with doc text saying that is why; there is no equivalent `targeted_inquiry*` method for the inquiry target.
4. Confirm the drift surface: edit any one of the four copies to drop `inquiry.materialise(now)` and run `cargo test -p service-com-amazonaws-artifact-artifact` — only that one operation's assertions fail, so nothing keeps the four copies agreeing.
