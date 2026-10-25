Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c03b32b2dcc5227aba431e790e0b1e2397cf2279

# Rescore's document-exclusivity refusals interpolate the caller's document Id, breaking the crate's own "never render the value" refusal rule

_Filed: 2026-10-25T07:06:21Z_
_Operation: Rescore | Class: error-response_

> _Note: this issue was identified against the code base at commit `c03b32b2dcc5227aba431e790e0b1e2397cf2279`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states ONE rule for how it words a refusal of a handler-enforced constraint: the sentence names where the offending value was and what it had to satisfy, and never renders the caller's value. `state.rs` implements that rule twice — once generically (`clause` / `constraint_report`, which deliberately feed a placeholder value so "the value never reaches the body") and once specifically for the `Rescore` document-body rule (`DOCUMENT_BODY_REQUIRED_MESSAGE`, documented as "A CONSTANT, request-independent sentence … the rule is about the document's shape, so nothing of the caller's values belongs in it").

The `Rescore` handler follows that rule in two of the three branches of the SAME validation loop and breaks it in the other two: the `Documents` batch-size refusal goes through `crate::state::constraint_report`, the missing-body refusal uses the constant, but the `Body`/`TokenizedBody` and `Title`/`TokenizedTitle` exclusivity refusals are hand-built `format!`s that interpolate `document.id` — a caller-supplied value up to 2048 characters (`Document$Id`, `length 1..2048`) — straight into the response body. So three refusals about the same document shape, decided three lines apart in one loop, are worded under three different (and mutually contradictory) policies, and the two hand-built ones reflect a request value back to the caller that the crate's own documentation says must never appear.

## Evidence

The crate's stated rule, `src/state.rs:44-48`:
```
//! A CONSTANT, request-independent sentence naming the members the document must
//! choose between, like [`PLAN_NOT_FOUND_MESSAGE`]: the rule is about the document's
//! shape, so nothing of the caller's values belongs in it.
pub const DOCUMENT_BODY_REQUIRED_MESSAGE: &str = "Document must have a Body or TokenizedBody.";
```
and `src/state.rs:76-77` (the rendering reached by every refusal this service words):
```
// * a clause names WHERE the offending value was and WHAT it had to satisfy, and
//   never renders the value itself;
```
and `src/state.rs:164-172`, the helper provided for exactly this case:
```
/// The report for one member that failed `kind`, for a handler stating a constraint the
/// model cannot express itself.
pub fn constraint_report(member: &str, kind: ViolationKind) -> String {
...
        // A PRESENT value selects the value-omitting clause rather than the `Value null`
        // one an absent member gets. What it holds is irrelevant: the value never
        // reaches the body.
```
The same loop in `src/ops/rescore.rs` obeys the rule twice — `rescore.rs:200-206` (`message: Some(crate::state::constraint_report(DOCUMENTS_MEMBER, …))`) and `rescore.rs:230-232` (`message: Some(crate::state::DOCUMENT_BODY_REQUIRED_MESSAGE.to_string())`) — then breaks it twice, `src/ops/rescore.rs:235-252`:
```
            if document.body.is_some() && document.tokenized_body.is_some() {
                return Err(RescoreErrorModeled::ValidationException(ValidationException {
                    message: Some(format!(
                        "Document {} provides both Body and TokenizedBody; provide one.",
                        document.id
                    )),
...
            if document.title.is_some() && document.tokenized_title.is_some() {
...
                        "Document {} provides both Title and TokenizedTitle; provide one.",
                        document.id
```
Test coverage: the echoing wording is currently pinned verbatim by `tests/all/wire_error_status.rs:363-385` ("Document d1 provides both Title and TokenizedTitle; provide one.", "Document d3 provides both Body and TokenizedBody; provide one."), whose module header (`wire_error_status.rs:21-27`) states a reason only for the PRECEDENCE of the two rules, not for interpolating the document id; the precedence assertions and the `ValidationException` code/status assertions (`wire_error_status.rs:428-446`) are unaffected by how the sentence is worded.

## Steps to Reproduce

1. Create a rescore execution plan and wait past the CREATING transition so it is ACTIVE (see tests/all/harness.rs `create_active_plan`).
2. Call Rescore with Documents = [{"Id":"d3","OriginalScore":0.5,"Body":"a body","TokenizedBody":["a","body"],"Title":"t"}].
3. Observe the 400 ValidationException body: `message` = "Document d3 provides both Body and TokenizedBody; provide one." — it renders the caller-supplied `Document$Id`.
4. Compare with the sibling refusal in the same loop: omit both body forms and the body is the constant "Document must have a Body or TokenizedBody." (src/state.rs:48), and send 101 documents and the body is the value-omitting clause built by `crate::state::constraint_report` (src/ops/rescore.rs:200-206), both of which obey the rule quoted at src/state.rs:76-77.
