Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# Hoist the "does this inquiry own that query identifier" rule (and the request-list-is-a-SET normalisation) into one place instead of two handler copies

_Filed: 2026-10-26T08:26:05Z_
_Operation: ExportComplianceInquiry | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two rules of the compliance-inquiry surface are written out twice, once in `ExportComplianceInquiry` and once in `PutComplianceInquiryFeedback`, even though both handlers already share the refusal payload (`crate::state::unknown_query_identifiers`) and every other inquiry rule lives as a method on `crate::state::ComplianceInquiry` (`feedback_for`, `accepts_another_feedback`, `display_name`, `reported_tags`).

1. THE OWNERSHIP RULE. "an identifier that names no query of that inquiry is refused" is implemented in two different ways: the export handler materialises a `Vec<i32>` of the inquiry's identifiers and tests `!owned.contains(id)`; the feedback handler scans `inquiry.queries` with `any(|query| query.identifier == identifier)`. The feedback handler's own module doc says this is the SAME refusal the export gives, so the two copies state one rule and can drift (e.g. if the set of queries an inquiry "owns" ever stops being exactly `inquiry.queries` — a withdrawn or failed query — one handler would be updated and the other not).

2. THE SET NORMALISATION. The "a list a request carried is a SET: ascending, no repeats" normalisation is written out twice (`covered.sort_unstable(); covered.dedup();` for `queryIdentifiers`, `reason_codes.sort_unstable(); reason_codes.dedup();` for `reasonCodes`), and the second copy's comment explicitly names the first as its precedent.

Fix shape: add the predicate (and the identifier list the export's "named none" arm needs) as methods on `ComplianceInquiry` in `src/state.rs` beside `feedback_for`, and a single normalisation helper in `src/state.rs` beside `page_size`/`walk_page`; call them from both handlers. No response body changes and no test changes: both handlers keep the same error payloads (`unknown_query_identifiers("queryIdentifiers")` / `unknown_query_identifiers("queryIdentifier")`), the same ordering of checks, and the same stored records.

## Evidence

Copy 1 of the ownership rule — services/service-com-amazonaws-artifact-artifact/src/ops/export_compliance_inquiry.rs:203-229:
```
        let owned: Vec<i32> = inquiry
            .queries
            .iter()
            .map(|query| query.identifier)
            .collect();
...
                if requested.iter().any(|id| !owned.contains(id)) {
...
                    return Err(ExportComplianceInquiryErrorModeled::ValidationException(
                        crate::state::unknown_query_identifiers("queryIdentifiers"),
```

Copy 2 of the SAME rule — src/ops/put_compliance_inquiry_feedback.rs:249-265:
```
        if let Some(identifier) = input.query_identifier {
            if !inquiry
                .queries
                .iter()
                .any(|query| query.identifier == identifier)
            {
...
                return Err(PutComplianceInquiryFeedbackErrorModeled::ValidationException(
                    crate::state::unknown_query_identifiers("queryIdentifier"),
```
and that file's own doc, src/ops/put_compliance_inquiry_feedback.rs:13-16, states the two are one rule: "`queryIdentifier` names WHICH of the inquiry's queries the feedback is on, so an identifier that names no query of that inquiry is refused with the modelled `ValidationException` naming the member and the requirement it failed — the same refusal `ExportComplianceInquiry` gives its own `queryIdentifiers`".

Copy 1 of the set normalisation — src/ops/export_compliance_inquiry.rs:230-236: "// Ascending and without repeats: which queries an export covers is a SET, // so neither the order a request listed them in nor a repeat is part of // it." followed by `let mut covered = requested; covered.sort_unstable(); covered.dedup();`
Copy 2 — src/ops/put_compliance_inquiry_feedback.rs:211-217: "// ... ascending and // without repeats, because which reasons were given is a SET — neither the order // the request listed them in nor a repeat is part of it. The same treatment // `ExportComplianceInquiry` gives its `queryIdentifiers`." followed by `reason_codes.sort_unstable(); reason_codes.dedup();`

The crate's precedent for hoisting such a rule into state.rs: src/state.rs:1064-1084 (`feedback_for`, `accepts_another_feedback`, `record_feedback`) and src/state.rs:1137-1146 (`reported_tags` — "One rule, applied in one place, so those operations cannot disagree").

Test coverage that must stay unchanged: tests/all/compliance_inquiry_export.rs:763 `query_identifiers_the_inquiry_does_not_own_are_refused` (asserting the clause at line 766 and the field name at line 822), tests/all/compliance_inquiry_feedback.rs:657 `a_query_identifier_the_inquiry_does_not_own_is_refused_with_a_validation_exception` (clause at line 660, fieldList at line 730), the export's covered-identifier assertions at tests/all/compliance_inquiry_export.rs:546-550, and the normalised reason codes pinned by tests/all/compliance_inquiry_feedback.rs:342-354 (`stored_reason_codes`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-artifact-artifact/src/ops/export_compliance_inquiry.rs and read lines 203-239: the inquiry's query identifiers are collected into `owned` and a requested identifier is refused with `unknown_query_identifiers("queryIdentifiers")` when `!owned.contains(id)`; the accepted list is then sorted and deduped.
2. Open src/ops/put_compliance_inquiry_feedback.rs and read lines 211-217 and 249-266: the same sort+dedup normalisation and the same ownership refusal are written again, the second with a direct `any(|query| query.identifier == identifier)` scan.
3. Grep src/state.rs for `impl ComplianceInquiry`: it already carries every other per-inquiry rule (`feedback_for`, `accepts_another_feedback`, `display_name`, `reported_tags`) but no `owns_query`/`query_identifiers` helper, so the two handlers each re-derive it.
4. Run the suite (`cargo test -p service-com-amazonaws-artifact-artifact`) before and after hoisting both rules into src/state.rs: every assertion listed in the evidence must pass unchanged, since the error payloads, the ordering of checks and the stored records are untouched.
