Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8e0ac146ffee686dd3a46765dec581c593b08a30

# The taggable-kind to tag-map dispatch is written out twice in state.rs, once per borrow kind

_Filed: 2026-10-28T03:18:35Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8e0ac146ffee686dd3a46765dec581c593b08a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`resource_tags` and `resource_tags_mut` each spell out the same four-arm dispatch from `TaggableResourceKind` to the store map that holds that kind's `tags` field. The ARN parsing is already single-homed (`parse_taggable_arn`), but the kind→map mapping — the part a new taggable kind would have to be added to — exists in two copies, so the read half and the write half of the tagging surface can drift: a kind resolved in one and not the other makes a resource taggable but not readable (or the reverse). The doc on the read copy states the invariant the second copy must also satisfy, without any mechanism tying them together. Collapsing the two into one dispatch (e.g. one macro or one kind-keyed accessor used by both) leaves every response and every existing test unchanged.

## Evidence

First copy — `src/state.rs:3084-3106`:
```
    /// with the resource. EVERY taggable kind this service persists is resolved here: a
    /// kind left unresolved would make the tags its create stored unreadable.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        let resource = parse_taggable_arn(resource_arn)?;
        match resource.kind {
            TaggableResourceKind::BillingGroup => self
                .billing_groups
                .get(&resource.arn)
                .map(|record| &record.tags),
            TaggableResourceKind::PricingPlan => self
                .pricing_plans
                .get(&resource.arn)
                .map(|record| &record.tags),
            TaggableResourceKind::PricingRule => self
                .pricing_rules
                .get(&resource.arn)
                .map(|record| &record.tags),
            TaggableResourceKind::CustomLineItem => self
                .custom_line_items
                .get(&resource.arn)
                .map(|record| &record.tags),
        }
    }
```
Second copy — `src/state.rs:3108-3132`:
```
    /// [`resource_tags`](Self::resource_tags), for a tagging operation that WRITES.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        let resource = parse_taggable_arn(resource_arn)?;
        match resource.kind {
            TaggableResourceKind::BillingGroup => self
                .billing_groups
                .get_mut(&resource.arn)
                .map(|record| &mut record.tags),
            TaggableResourceKind::PricingPlan => self
                .pricing_plans
                .get_mut(&resource.arn)
                .map(|record| &mut record.tags),
            TaggableResourceKind::PricingRule => self
                .pricing_rules
                .get_mut(&resource.arn)
                .map(|record| &mut record.tags),
            TaggableResourceKind::CustomLineItem => self
                .custom_line_items
```
The two callers that keep them apart: `src/ops/list_tags_for_resource.rs:171` (`let Some(stored) = state.resource_tags(&input.resource_arn)`) and `src/ops/tag_resource.rs:195` / `src/ops/untag_resource.rs:183` (`state.resource_tags_mut(&input.resource_arn)`).

Tests that pin the behaviour a dedup must preserve: `tests/all/tagging_surface.rs:21-24` — "The rows differ along what those rules generalise over: the resource KIND (all four the modelled tagging `Arn` enumerates — `pricingplan`, `billinggroup`, `pricingrule` and `customlineitem`)" — driving TagResource, ListTagsForResource and UntagResource over all four kinds; plus `tests/all/custom_line_item_tag_lifecycle.rs`, `tests/all/untag_created_tags.rs` and `tests/all/reserved_tag_keys.rs`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-billingconductor-awsbillingconductor/src/state.rs and read `resource_tags` (line 3086) and `resource_tags_mut` (line 3109).
2. Observe the identical four-arm `match resource.kind` dispatch, differing only in `get` vs `get_mut` and `&` vs `&mut`.
3. Note that adding a fifth taggable kind, or correcting the map one kind resolves to, must be done in both copies; the read copy's doc states the invariant ("EVERY taggable kind this service persists is resolved here") but nothing enforces it on the write copy.
4. Run the tagging suites (tests/all/tagging_surface.rs, custom_line_item_tag_lifecycle.rs, untag_created_tags.rs, reserved_tag_keys.rs) before and after collapsing the dispatch into one place: every response must be byte-identical.
