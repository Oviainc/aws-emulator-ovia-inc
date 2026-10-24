Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# Tag keys in the reserved aws: namespace (and other TagKey pattern violations) are accepted and stored

_Filed: 2026-10-24T08:26:44Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` merges whatever key/value pairs the request carries into the campaign's tag map after one single check — the 50-tags-per-resource ceiling. No check of the tag KEY itself happens, in the handler or anywhere else, so `POST /v2/tags/{campaignArn}` with `{"tags":{"aws:reserved":"x"}}` answers HTTP 200 and `ListTagsForResource` then reports the reserved key back.

Expected (AWS + this model): `TagMap`'s key shape `TagKey` carries `@pattern "^(?!aws:)[a-zA-Z+-=._:/]+$"` — a key beginning with the reserved `aws:` prefix (and any key outside that character class, e.g. one containing a space) is refused with HTTP 400 `ValidationException` and NOTHING is written. AWS reserves the `aws:` tag namespace for system tags on every taggable service.

Actual: HTTP 200, the tag is persisted and is visible to `ListTagsForResource` / `DescribeCampaign`.

The reason the central constraint walk does not catch it: the runtime's `@pattern` checker compiles the model regex with the Rust `regex` crate, which does not support the negative lookahead `(?!aws:)`; `vendor/smithy-schema-server/src/constraints.rs::pattern_matches` documents that "a pattern that fails to compile is treated as matching (we never reject a value because of an unparseable model pattern)". So for THIS member the pattern is silently unenforced and the handler has to apply the rule itself.

## Evidence

Spec: `TagMap`/`TagKey` in the Smithy model — key `@length 1..128`, `@pattern ^(?!aws:)[a-zA-Z+-=._:/]+$`; `TagResource` models `ValidationException` (HTTP 400). Handler: src/ops/tag_resource.rs:192-222 — resolves the ARN, clones the stored map, inserts every `(key, value)` from `input.tags` verbatim, and the ONLY refusal is `merged.len() > crate::state::MAX_TAGS_PER_RESOURCE` (src/state.rs:76). Framework: vendor/smithy-schema-server/src/constraints.rs `pattern_matches` + its TODO at the `Regex::new` failure arm — an uncompilable pattern is treated as a match, and `(?!...)` does not compile in the `regex` crate. Test coverage: none — tests/all/instance_onboarding_tags_and_outbound_requests.rs::tagging_merges_into_the_one_map_every_reader_sees only uses ordinary keys; no test anywhere in the crate sends an `aws:`-prefixed or otherwise illegal tag key (grep for `aws:` in tests/ finds only ARNs).

## Steps to Reproduce

1. Boot the emulator with the connectcampaignsv2 crate loaded.
2. PUT /v2/connect-instance/instance-1/onboarding with {"encryptionConfig":{"enabled":false}} and poll GET /v2/connect-instance/instance-1/onboarding until status is SUCCEEDED.
3. PUT /v2/campaigns with {"name":"tagged","connectInstanceId":"instance-1"} and keep the returned arn.
4. POST /v2/tags/<url-encoded arn> with {"tags":{"aws:reserved":"x"}} -> observe HTTP 200; expected HTTP 400 ValidationException because TagKey forbids the aws: prefix.
5. GET /v2/tags/<url-encoded arn> -> observe the reserved key in the tag map; expected the resource to carry no such tag.
6. Repeat step 4 with a key outside the TagKey character class (e.g. "my key") -> observe HTTP 200 instead of HTTP 400.
