Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 715ef9592449e1f33a4d4348cc0507a62879bbde

# decode_cursor accepts a NextToken this service never minted (a well-formed token with an EMPTY cursor value), restarting the walk with a 200 instead of InvalidNextTokenException

_Filed: 2026-10-29T05:18:12Z_
_Operation: DescribeScalingPlans | Class: pagination_

> _Note: this issue was identified against the code base at commit `715ef9592449e1f33a4d4348cc0507a62879bbde`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The shared pagination helper `crate::state::decode_cursor` documents itself as answering `None` "for anything this service did not mint", and both paged reads turn that `None` into the modelled `InvalidNextTokenException`. The implementation only checks that the hex-decoded token starts with `<kind>:` — it accepts an EMPTY remainder, a cursor value the minting side can never produce.

`encode_cursor` is called in exactly two places: `DescribeScalingPlans` mints it from `plan.name` and `DescribeScalingPlanResources` from a `(ServiceNamespace, ResourceId, ScalableDimension)` identity key. `ScalingPlanName` is modelled `len 1..128`, and a resource identity key always carries two `/` separators, so no minted token ever decodes to an empty value. A hand-made token `"706c616e733a"` (hex of `plans:`) therefore is not a token this service minted, yet it decodes to `Some("")`, and the resume filter `plan.name > *last` is true for every stored plan (every name is non-empty). The caller gets HTTP 200 with the FULL first page again instead of the modelled 400 `InvalidNextTokenException` — a bogus/truncated token silently restarts the walk, so a client paging in a loop re-reads the same page rather than being told its token is invalid. The same hole exists on `DescribeScalingPlanResources` through the same helper (hex of `plan-resources:`).

Secondary instance of the same hole: `hex::decode` is case-insensitive, so the upper-cased spelling of a minted token (`706C616E733A…`) also decodes, although it is not a token this service handed out.

## Evidence

The helper's own stated rule, and the code that breaks it — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:1610-1629:
```
/// Encode a pagination cursor as an opaque token.
///
/// The `kind` prefix is part of the encoded value, so a token minted by one
/// operation does not decode for another.
pub fn encode_cursor(kind: &str, value: &str) -> String {
    hex::encode(format!("{kind}:{value}"))
}

/// Decode a cursor minted by [`encode_cursor`] for the same `kind`.
///
/// `None` for anything this service did not mint — the caller answers
/// `InvalidNextTokenException`.
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    decoded
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix(':'))
        .map(ToString::to_string)
}
```
`strip_prefix(':')` accepts an empty `rest`, so `decode_cursor("plans", hex("plans:"))` is `Some("")`.

What the minting side can produce — src/ops/describe_scaling_plans.rs:238-243:
```
        let next_token = if page.len() > page_size {
            page.truncate(page_size);
            page.last()
                .map(|plan| crate::state::encode_cursor(CURSOR_KIND, &plan.name))
```
and the model makes that value non-empty — OPERATIONS.md:47 lists `ScalingPlanNames` over `ScalingPlanName`, whose shape is `(string, len 1..128, pattern ^[\p{Print}&&[^|:/]]+$)` (OPERATIONS.md:29). The sibling read mints from an identity key instead — src/ops/describe_scaling_plan_resources.rs:239: `.map(|(identity, _)| crate::state::encode_cursor(CURSOR_KIND, identity))`.

What the accepted empty cursor then does — src/ops/describe_scaling_plans.rs:191-200 and 231-234:
```
        // The cursor is the name the previous page stopped after, so paging is stable
        // over the ordered map even when plans are created or deleted between pages.
        let after: Option<String> = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                crate::state::decode_cursor(CURSOR_KIND, token).ok_or_else(|| {
                    invalid_next_token("The NextToken provided is not valid")
                })?,
            ),
        };
...
        let mut page: Vec<&crate::state::ScalingPlanRecord> = matching
            .filter(|plan| after.as_ref().is_none_or(|last| plan.name > *last))
            .take(page_size + 1)
```
`after = Some("")` makes `plan.name > ""` true for every plan, i.e. a full first page.

Test coverage states the rule the code breaks but never probes this shape — tests/all/scaling_plan_lifecycle.rs:1115-1125:
```
        // A token neither operation minted does not decode, whatever its shape.
        for token in ["garbage", "", "0", "7a7a7a", "plans:plan-1"] {
            ...
                (400, "InvalidNextTokenException"),
```
Note `""` is an empty TOKEN (hex-decodes to the empty string, whose `strip_prefix("plans")` fails, so it IS refused) — not a well-formed token carrying an empty cursor VALUE, which no row covers. `tests/all/scaling_plan_resource_paging.rs` only walks tokens the service minted.

## Steps to Reproduce

1. CreateScalingPlan three plans (`plan-a`, `plan-b`, `plan-c`), each over its own ApplicationSource, and let the clock settle them to Active.
2. Call DescribeScalingPlans {"MaxResults": 50, "NextToken": "706c616e733a"} — the hex encoding of `plans:`, a token the service never minted (every minted token carries a non-empty plan name after the prefix).
3. Observed: HTTP 200 with all three plans — the walk silently restarts at the beginning. Expected: HTTP 400 `InvalidNextTokenException`, the answer `decode_cursor`'s own doc prescribes for "anything this service did not mint".
4. Same on the sibling read: DescribeScalingPlanResources {"ScalingPlanName":"plan-a","ScalingPlanVersion":1,"NextToken":"706c616e2d7265736f75726365733a"} (hex of `plan-resources:`) answers 200 with the first page instead of the modelled 400.
5. Contrast with the refused shapes already pinned in tests/all/scaling_plan_lifecycle.rs:1116 (`"garbage"`, `""`, `"0"`, `"7a7a7a"`, `"plans:plan-1"`), all of which do answer 400.
