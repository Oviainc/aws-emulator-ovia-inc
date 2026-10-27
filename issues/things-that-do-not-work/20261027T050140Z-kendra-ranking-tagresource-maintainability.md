Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# The ARN-to-plan resolution walk is written twice in state.rs (read and write copies already differ in the accessor they go through)

_Filed: 2026-10-27T05:01:40Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` resolves a `ResourceARN` to a stored plan with two independent copies of the same three-step walk — take the region's bucket, map the ARN to a plan id through that region's `arn_index`, fetch the plan under that id — one for the reading path (`plan_by_arn`, reached by `ListTagsForResource` through `plan_for_arn`) and one for the writing path (`plan_by_arn_mut`, reached by `TagResource` and `UntagResource` through `plan_for_arn_mut`). The crate's own policy sentence right below them states that the ARN resolution is single-sourced ("the ONE place an ARN-addressed operation turns a `ResourceARN` into either a plan or its refusal, so every such operation resolves the ARN the same way"), but only the REFUSAL is single-sourced: the resolution itself exists in two copies that can drift. The two copies already differ in one step — the read copy goes through the documented `plans_in` reader, the write copy reaches into `self.regions.get_mut` directly — so a change taught to one accessor (ARN normalisation, skipping a plan whose teardown is in flight, an account check) silently applies to only one of the tagging surface's two halves. The same read/write split is repeated one pair up for the id-addressed lookup (`plan` goes through `plans_in`, `plan_mut` through `self.regions.get_mut`). The fix is to let the write copy obtain the plan id through the one existing read step (e.g. an `id_for_arn(&self, region, arn) -> Option<String>` that `plan_by_arn` and `plan_by_arn_mut` both call) so the ARN→id mapping is written once; every response and every existing test stays byte-for-byte unchanged.

## Evidence

Copy 1 — the reading walk, `services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/src/state.rs:823-827`:

```rust
pub fn plan_by_arn(&self, region: &str, resource_arn: &str) -> Option<&RescoreExecutionPlan> {
    let regional = self.plans_in(region)?;
    let id = regional.arn_index.get(resource_arn)?;
    regional.plans.get(id)
}
```

Copy 2 — the same walk again, `state.rs:830-838`:

```rust
pub fn plan_by_arn_mut(
    &mut self,
    region: &str,
    resource_arn: &str,
) -> Option<&mut RescoreExecutionPlan> {
    let regional = self.regions.get_mut(region)?;
    let id = regional.arn_index.get(resource_arn)?.clone();
    regional.plans.get_mut(&id)
}
```

The crate's own single-source claim sits immediately after them, `state.rs:840-844`: "The plan an ARN-addressed call names in `region`, or the modelled refusal that ARN resolves to nothing — the ONE place an ARN-addressed operation turns a `ResourceARN` into either a plan or its refusal, so every such operation resolves the ARN the same way and refuses it with the same sentence."

Already-drifted step: copy 1 goes through the accessor documented at `state.rs:800-807` ("A READER: it never creates the region's bucket, so no read changes what a later caller sees." — `pub fn plans_in`), copy 2 bypasses it with `self.regions.get_mut(region)?`. The same split is repeated for the id lookup at `state.rs:810-817` (`plan` uses `self.plans_in(region)?`, `plan_mut` uses `self.regions.get_mut(region)?`).

Callers of the two copies: `src/ops/list_tags_for_resource.rs:172-174` (`store.plan_for_arn(&region, &input.resource_arn)`), `src/ops/tag_resource.rs:183-185` (`store.plan_for_arn_mut(&region, &input.resource_arn)`), `src/ops/untag_resource.rs:178-180` (`store.plan_for_arn_mut(&region, &input.resource_arn)`).

Test coverage that pins the behaviour both copies must keep producing (so the de-duplication is verifiable without changing any response):
- `tests/all/arn_refusal_single_source.rs:39` `every_arn_addressed_operation_states_the_same_unavailable_refusal` — drives all three ARN-addressed operations over five ARN forms.
- `tests/all/resource_tags.rs:272` `an_arn_that_names_no_plan_is_refused_with_resource_unavailable` and `tests/all/resource_tags.rs:339` `tags_belong_to_the_resource_the_arn_names` (including the cross-region read at lines 377-392).
- `tests/all/wire_error_status.rs:92` `every_client_error_travels_with_the_protocols_client_error_status` — the `ResourceUnavailableException` rows for TagResource / UntagResource / ListTagsForResource.

## Steps to Reproduce

1. Open services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/src/state.rs and read lines 819-844.
2. Observe that `plan_by_arn` (823-827) and `plan_by_arn_mut` (830-838) each spell out the same region-bucket -> arn_index -> plans walk, and that the two copies already reach the region bucket through different accessors (`plans_in` vs `self.regions.get_mut`).
3. Note the doc comment at 840-844 claiming ARN resolution is done in ONE place, while only the refusal (`plan_for_arn` / `plan_for_arn_mut`) is shared.
4. Confirm the two copies back different halves of the tagging surface: src/ops/list_tags_for_resource.rs:172 uses the read copy, src/ops/tag_resource.rs:183 and src/ops/untag_resource.rs:178 use the write copy — so a resolution rule added to one copy would apply to ListTagsForResource but not to TagResource/UntagResource (or the reverse).
5. Run the suites that pin the resolved/refused answers (tests/all/arn_refusal_single_source.rs, tests/all/resource_tags.rs, tests/all/wire_error_status.rs); they must keep passing unchanged after the ARN->id step is written once.
