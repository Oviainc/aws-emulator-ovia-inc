Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: adb8f986b66b684ed38690a8bd355d4f46d0ebf5

# The per-resource tag-count cap is implemented twice (CreateDomain and TagResource) while the rest of the tag rule is single-homed in tag_bounds

_Filed: 2026-10-26T06:20:30Z_
_Operation: CreateDomain | Class: maintainability_

> _Note: this issue was identified against the code base at commit `adb8f986b66b684ed38690a8bd355d4f46d0ebf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate deliberately single-homes the tag-entry rule: `service::tag_bounds::violation` is "the one check every operation that accepts a `TagMap` shares", so `CreateDomain` and `TagResource` "cannot enforce different halves of one bound". The other half of the same rule — a resource holds at most `state::MAX_TAGS_PER_RESOURCE` tags, answered as `ServiceQuotaExceededException` (402) — is NOT single-homed: it is written out separately in each of the two handlers, and `TagResource`'s own module doc states the two are "the same condition and the same channel". The two copies already differ in what they count (`CreateDomain` bounds the REQUEST's entry count, `TagResource` bounds the MERGED set) — equivalent today only because a create's resource starts with no tags — so the copies can drift silently the moment either side changes. Collapsing the comparison into one place (e.g. bounding `tags_after_merge`'s result in both paths) leaves both wire responses and both existing tests untouched.

## Evidence

Copy 1, `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/create_domain.rs:264-276`:
"        if let Some(tags) = input.tags.as_ref() {\n            if tags.len() > crate::state::MAX_TAGS_PER_RESOURCE {\n                return Err(CreateDomainErrorModeled::ServiceQuotaExceededException(\n                    ServiceQuotaExceededException {\n                        message: Some(format!(\n                            \"tags carries {} entries, more than the maximum of {}\", ..."

Copy 2, `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/tag_resource.rs:229-242`:
"        let merged = store.tags_after_merge(&input.resource_arn, input.tags);\n        if merged.len() > crate::state::MAX_TAGS_PER_RESOURCE {\n            return Err(TagResourceErrorModeled::ServiceQuotaExceededException(\n                ServiceQuotaExceededException {\n                    message: Some(format!(\n                        \"resource {} would carry {} tags, more than the maximum of {}\", ..."

The crate itself says it is one rule in two places — `src/ops/tag_resource.rs:66-68`: "//! * `ServiceQuotaExceededException` (HTTP 402) — the merged tag set would exceed\n//!   [`crate::state::MAX_TAGS_PER_RESOURCE`], the same condition and the same channel\n//!   `CreateDomain` uses for a create carrying more tags than a resource may hold." — while the sibling half of the tag rule is explicitly stated once, `src/service/tag_bounds.rs:17-19`: "//! refuse the entry, and every operation of this service that accepts a `TagMap` —\n//! `CreateDomain` and `TagResource` — refuses it through [`violation`] here, stated once\n//! so the two cannot enforce different halves of one bound."

Existing tests that must stay unchanged by the fix: `tests/all/domain_subscription_lifecycle.rs:960` `store_growth_is_bounded_and_refused_with_the_modelled_errors` (rows at `tests/all/domain_subscription_lifecycle.rs:995-1002`: "tags at the maximum count" -> 201, "one tag past the maximum count" -> 402) pins the `CreateDomain` copy, and `tests/all/resource_tagging.rs:366` `the_tagging_bounds_are_enforced_on_the_merged_set` (assertion at `tests/all/resource_tagging.rs:504-508`: `(402, "ServiceQuotaExceededException")`) pins the `TagResource` copy.

## Steps to Reproduce

1. Read src/ops/create_domain.rs:264-276 — the tag-count cap and its 402 refusal.
2. Read src/ops/tag_resource.rs:229-242 — the same cap against `MAX_TAGS_PER_RESOURCE`, with its own 402 refusal.
3. Read src/ops/tag_resource.rs:66-68 and src/service/tag_bounds.rs:17-19 — the crate states this is one condition, and single-homes the other half of the same tag rule in `tag_bounds::violation`.
4. Note the two copies count different things (request entries vs merged set), so changing the rule in one place silently diverges the other.
