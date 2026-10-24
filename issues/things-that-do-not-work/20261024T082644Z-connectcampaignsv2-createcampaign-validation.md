Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# Tags supplied at CreateCampaign bypass both the TagKey rules and the per-resource tag cap TagResource enforces

_Filed: 2026-10-24T08:26:44Z_
_Operation: CreateCampaign | Class: validation_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCampaign` takes `input.tags` and stores it verbatim (`tags: input.tags.unwrap_or_default().into_iter().collect()`) with no validation at all, so the create path is strictly more permissive than the tag path on the same resource:

1. Reserved/illegal keys: a create carrying `{"tags":{"aws:reserved":"x"}}` answers HTTP 200 and the key is persisted and echoed back in the `CreateCampaign` response and by `ListTagsForResource`. `TagKey`'s `@pattern ^(?!aws:)[a-zA-Z+-=._:/]+$` forbids it, and the central constraint walk cannot enforce it (the Rust `regex` crate rejects the negative lookahead, and the runtime then treats the pattern as matching — see vendor/smithy-schema-server/src/constraints.rs::pattern_matches). Expected HTTP 400 `ValidationException` with nothing created.
2. Tag cap: `TagResource` refuses a merge that would leave the campaign with more than `MAX_TAGS_PER_RESOURCE` (50) tags, but `CreateCampaign` accepts a map of any size, so a campaign can be born with 60 tags that no `TagResource` call could ever have produced. Expected the same ceiling (and the modelled `ValidationException`) on the create path.

Actual: both requests answer HTTP 200 and the campaign is created with the invalid tag map.

## Evidence

Spec: `CreateCampaign$tags` targets `TagMap`, whose key shape `TagKey` carries `@length 1..128` and `@pattern ^(?!aws:)[a-zA-Z+-=._:/]+$`; `CreateCampaign` models `ValidationException` (HTTP 400). Handler: src/ops/create_campaign.rs:209-229 — the only pre-store checks are the onboarded-instance gate and the store ceiling; `tags` is moved into `StoredCampaign` unchecked. Contrast src/ops/tag_resource.rs:201-222, which enforces `crate::state::MAX_TAGS_PER_RESOURCE` (src/state.rs:76, derived from `TagKeyList`'s 0..50 bound) on the very same stored map. Framework: the lookahead pattern cannot be compiled, so no boundary check rejects an `aws:` key (vendor/smithy-schema-server/src/constraints.rs, `Regex::new` failure arm => treated as matching). Test coverage: none — tests/all/campaign_lifecycle.rs::create_then_describe_returns_every_stored_member creates with ordinary tag keys and asserts they round-trip; no test sends a reserved key or an oversized tag map to CreateCampaign.

## Steps to Reproduce

1. Boot the emulator with the connectcampaignsv2 crate loaded.
2. PUT /v2/connect-instance/instance-1/onboarding with {"encryptionConfig":{"enabled":false}} and poll GET /v2/connect-instance/instance-1/onboarding until SUCCEEDED.
3. PUT /v2/campaigns with {"name":"c1","connectInstanceId":"instance-1","tags":{"aws:reserved":"x"}} -> observe HTTP 200 and the reserved key echoed in the response `tags`; expected HTTP 400 ValidationException and no campaign created.
4. GET /v2/tags/<url-encoded arn of that campaign> -> observe the reserved tag stored.
5. PUT /v2/campaigns with {"name":"c2","connectInstanceId":"instance-1","tags":{...60 distinct legal keys...}} -> observe HTTP 200; expected the same 50-tag refusal TagResource gives (POST /v2/tags/{arn} with 51 tags is refused with ValidationException).
