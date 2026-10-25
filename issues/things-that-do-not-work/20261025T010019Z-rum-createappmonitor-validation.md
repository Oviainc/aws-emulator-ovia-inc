Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 11e424e87637980d781b5ebada45988cd752adfb

# CreateAppMonitor stores Tags without the TagKey pattern check TagResource applies, so reserved aws: keys get persisted

_Filed: 2026-10-25T01:00:19Z_
_Operation: CreateAppMonitor | Class: validation_

> _Note: this issue was identified against the code base at commit `11e424e87637980d781b5ebada45988cd752adfb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAppMonitor$Tags` targets the same `TagMap` (key shape `TagKey`, `@pattern ^(?!aws:)[a-zA-Z+-=._:/]+$`) that `TagResource$Tags` targets. `TagResource` applies that pattern IN THE HANDLER, with a comment explaining why it must: the vendored boundary validator compiles `@pattern` with the `regex` crate, which has no look-around, and it treats a pattern it cannot compile as MATCHING — so the constraint is never applied at the boundary for any operation. `CreateAppMonitor` skips that check and inserts `input.tags` verbatim into the record, so a key in AWS's reserved `aws:` namespace (or a key with any character outside the modelled class) is accepted with HTTP 200 and is then reported by `GetAppMonitor` and `ListTagsForResource`, while the identical key is refused with 400 `BadRequestException` by `TagResource`. One service therefore enforces one modelled key constraint on one write path and not on the other.

The same asymmetry applies to the ceiling this crate derives from the tagging documentation: `TagResource` refuses a write that would leave more than `MAX_TAGS_PER_RESOURCE` tags on a resource, `CreateAppMonitor` applies no such check, so a create can seed a monitor past the count `TagResource` then refuses to exceed.

This is NOT the `Domain`-pattern/validation-message gap already reported for CreateAppMonitor in the two open trace issues; it concerns `Tags` only.

## Evidence

Model (smithy_query shape `TagKey`): `"pattern": "^(?!aws:)[a-zA-Z+-=._:/]+$"`, and `TagMap` has `"key": {"target": "TagKey", ... "pattern": "^(?!aws:)[a-zA-Z+-=._:/]+$"}` — the map `CreateAppMonitorRequest$Tags` and `TagResourceRequest$Tags` both target.

Sibling handler, services/service-com-amazonaws-rum-rum/src/ops/tag_resource.rs:196-203:
```
/// This check belongs in the handler rather than at the boundary: `TagKey`'s pattern opens
/// with the negative lookahead `(?!aws:)`, the vendored boundary validator compiles a
/// `@pattern` with the `regex` crate, which has no look-around, and it treats a pattern it
/// cannot compile as matching (`pattern_matches` in
/// `vendor/smithy-schema-server/src/constraints.rs`). So the constraint the model declares
/// would otherwise never be applied to a tag key at all.
```
and tag_resource.rs:388-395:
```
// `TagKey`'s `@pattern` is the one modelled constraint the boundary validator
// cannot apply (it opens with a negative lookahead), so it is applied here,
// before anything is written.
for (key, _) in &requested {
    if let Some(message) = tag_key_violation(&format!("tags.{key}.member"), key) {
        return Err(TagResourceError::BadRequest(ValidationException { message }));
```
The vendor side confirms it, vendor/smithy-schema-server/src/constraints.rs:421-424: "A pattern that fails to compile is treated as matching (we never reject a value because of an unparseable model pattern)."

This handler, services/service-com-amazonaws-rum-rum/src/ops/create_app_monitor.rs:370:
```
tags: input.tags.unwrap_or_default().into_iter().collect(),
```
— no `tag_key_violation`, no key inspection anywhere in the file (the only prose-rule refusals it makes are `domain_choice_refusal` and `app_monitor_configuration_refusal`, create_app_monitor.rs:215-234).

Ceiling side: tag_resource.rs:139-151 `const MAX_TAGS_PER_RESOURCE: usize = 50;` ("this operation's own documentation states \"You can associate as many as 50 tags with a resource\"") and tag_resource.rs:434-450 refuse a write past it; `CreateAppMonitor` references neither.

Test coverage: services/service-com-amazonaws-rum-rum/tests/all/resource_tagging.rs:481-511 (`a_tag_key_outside_the_modelled_pattern_is_refused_and_writes_nothing`) drives REJECTED_TAG_KEYS through `tag_resource(...)` only; the monitor in that test is created with the valid key `{"Env": "dev"}` (line 484-485), so no existing test pins CreateAppMonitor accepting a reserved key.

## Steps to Reproduce

1. POST /appmonitor with body {"Name": "tagged", "Domain": "example.com", "Tags": {"aws:owner": "me"}} — observe HTTP 200 and an Id.
2. GET /appmonitor/tagged (or GET /tags/arn:aws:rum:us-east-1:<account>:appmonitor/tagged) — the reserved key `aws:owner` is reported as a stored tag.
3. POST /tags/arn:aws:rum:us-east-1:<account>:appmonitor/tagged with body {"Tags": {"aws:owner": "me"}} — the same key is refused with HTTP 400 / BadRequestException by the sibling operation.
4. Also create a monitor with 60 valid tags: the create succeeds, while TagResource refuses the 51st key on the same resource.
