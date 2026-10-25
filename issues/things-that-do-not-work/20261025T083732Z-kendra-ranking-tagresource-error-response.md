Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: a727425709f65cfb01a0ac76d7787005b46d11cb

# TagResource's tag-ceiling refusal hand-builds a sentence that echoes a request-derived count, bypassing the crate's single-source constraint report

_Filed: 2026-10-25T08:37:32Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `a727425709f65cfb01a0ac76d7787005b46d11cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate enforces two unmodelled list ceilings in handler code: `Rescore`'s batch size (the model puts no maximum on `Documents`) and `TagResource`'s accumulated tag count (the model's `@length 0..200` on `TagList` bounds one REQUEST, not the set the plan accumulates). `state.rs` provides exactly one renderer for that situation — `constraint_report`, documented as "The report for one member that failed `kind`, for a handler stating a constraint the model cannot express itself" — and states the rule it implements: a clause "names WHERE the offending value was and WHAT it had to satisfy, and never renders the value itself", a rule the crate declares is "applied to EVERY constraint class and every operation". `state.rs`'s other handler-enforced ceiling, `capacity_units_over_quota`, repeats the rule for its own sentence: it "names the ceiling that was exceeded and never echoes the amount asked for, like every other refusal this service states".

`Rescore` obeys this: its documents-ceiling refusal goes through `crate::state::constraint_report`. `TagResource`'s tag-ceiling refusal, the only other handler-stated list ceiling in the crate, does neither — it `format!`s its own sentence and interpolates `{resulting}`, a count computed from the caller's request (the keys the plan already holds plus the ones this request introduces, via `tag_count_after`). So two refusals of the same class, in the same crate, are worded under two mutually contradictory policies, and one of them reflects a request-derived value back to the caller that the crate's own documentation says never reaches a refusal body. The error code and status are unaffected, so this is wording/shape fidelity only.

## Evidence

The crate's stated rule, `src/state.rs:72-77`:
```
// Two rules, both this service's own rendering of the standard AWS report, and
// both applied to EVERY constraint class and every operation (the boundary
// validator reaches them through `service.rs`'s `override_refusal`):
//
// * a clause names WHERE the offending value was and WHAT it had to satisfy, and
//   never renders the value itself;
```
The helper provided for exactly this case, `src/state.rs:162-172`:
```
/// The report for one member that failed `kind`, for a handler stating a constraint the
/// model cannot express itself.
pub fn constraint_report(member: &str, kind: ViolationKind) -> String {
...
        // A PRESENT value selects the value-omitting clause rather than the `Value null`
        // one an absent member gets. What it holds is irrelevant: the value never
        // reaches the body.
```
and the same rule restated for the crate's other handler-enforced ceiling, `src/state.rs:527-529`:
```
/// The sentence names the ceiling that was exceeded and never echoes the amount asked for,
/// like every other refusal this service states.
pub fn capacity_units_over_quota(
```
The sibling handler obeys it, `src/ops/rescore.rs:199-209`:
```
        if input.documents.len() > crate::state::MAX_DOCUMENTS_PER_RESCORE {
            return Err(RescoreErrorModeled::ValidationException(ValidationException {
                message: Some(crate::state::constraint_report(
                    DOCUMENTS_MEMBER,
                    ::smithy_server_runtime::ViolationKind::MaxLength(
                        crate::state::MAX_DOCUMENTS_PER_RESCORE as i64,
                    ),
                )),
```
This handler does not, `src/ops/tag_resource.rs:195-206`:
```
        let resulting = plan.tag_count_after(&input.tags);
        if resulting > crate::state::MAX_TAGS_PER_PLAN {
            return Err(TagResourceErrorModeled::ValidationException(
                ValidationException {
                    message: Some(format!(
                        "A rescore execution plan can carry at most {} tags; this request would leave it with {resulting}.",
                        crate::state::MAX_TAGS_PER_PLAN
                    )),
```
Test coverage: `tests/all/resource_tags.rs:410-486` (`a_plans_tag_set_is_bounded_and_a_request_past_the_ceiling_writes_nothing`) pins only the status and the error code for this refusal — `assert_eq!(status, expected_status, ...)` and `assert_eq!(error_code(&answer), "ValidationException", ...)` at `resource_tags.rs:468-477` — plus the "refused whole, nothing written" assertion at `resource_tags.rs:478-484`. The message text is not asserted anywhere in the suite (grep for "can carry at most" / "would leave it" in `tests/` matches nothing), and `tests/all/control_plane_fidelity.rs:3-10` scopes its report assertions to "A request outside a modelled constraint", so re-rendering this sentence through the crate's single-source report leaves every existing assertion passing.

## Steps to Reproduce

1. Create a plan and fill its tag set to `state::MAX_TAGS_PER_PLAN` (see tests/all/resource_tags.rs:419-433).
2. Call TagResource on that plan's ARN with one more, new tag key: `{"ResourceARN": arn, "Tags": [{"Key": "one-too-many", "Value": "v"}]}`.
3. Observe the 400 ValidationException body: `message` = "A rescore execution plan can carry at most 200 tags; this request would leave it with 201." — a count computed from the caller's request is rendered into the refusal.
4. Compare the sibling handler-enforced list ceiling: send Rescore with 101 documents and the body is the value-omitting clause built by `crate::state::constraint_report` (src/ops/rescore.rs:199-209), the rendering state.rs:72-77 says applies to every constraint class and every operation.
