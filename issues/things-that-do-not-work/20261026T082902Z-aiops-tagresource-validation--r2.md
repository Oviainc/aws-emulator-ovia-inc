Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# Modelled TagKey length/pattern is never applied to map KEYS, so an arbitrary tag key is accepted and persisted

_Filed: 2026-10-26T08:29:02Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both this crate's store-bounds doctrine and the `TagResource` handler's own comment assert that the modelled constraints on a tag KEY (`Tags` map key -> `TagKey`, @length 1..128, @pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`) are refused by the centralized constraint walk before the handler runs, and the handler therefore performs no key check of its own: it only screens the reserved `aws:` prefix and the resulting tag count, then inserts the caller's key verbatim into the stored group.

That premise does not hold. The centralized walk captures a map entry's key as a PATH SEGMENT and returns before any constraint check — the framework's own comment says so in as many words — so no length or pattern on a map key shape is ever applied, by the walk or by this crate. A `TagResource` call whose `tags` map carries a key of 500 characters (or a key containing characters outside the modelled class) answers 200, the key is stored in `InvestigationGroup::tags`, and `ListTagsForResource` reports it back. The crate's stated invariant that "nothing caller-sized reaches the store unchecked" is broken for every map key: the same unchecked class covers `CreateInvestigationGroup`'s `tags` keys and the `chatbotNotificationChannel` SNS-topic keys (`SNSTopicArn`, @length 20..2048, `^arn:.*$`) on create and update, where the comment makes the identical claim.

The crate already owns the pattern for a rule the walk cannot apply — `state::reserved_tag_key_rejection` and `state::collection_bounds_rejection` are exactly that — so this is a check that belongs beside them, not a new capability.

## Evidence

The claim, in the handler — `services/service-com-amazonaws-aiops-aiops/src/ops/tag_resource.rs:154-156`:
```
        // Every tag key's and value's length and pattern is a modelled constraint the
        // centralized walk has already refused anything outside of. The `Tags` map
        // carries NO modelled length, so the published per-resource maximum is applied
```
The same claim, single-homed in the state module's store-bounds section — `src/state.rs:76-79` and `:83-88`:
```
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteInvestigationGroup`, so each growth site carries a cap and nothing
// caller-sized reaches the store unchecked.
...
// Every remaining caller-sized member is already bounded by the model, so the
// centralized walk refuses it before a handler runs: `crossAccountConfigurations`
// (0..25), each channel's `ChatConfigurationArns` list (1..5) and its SNS topic key
// (20..2048), each tag key (1..128) and value (1..256), ...
```
What the walk actually does with a map key — `vendor/smithy-schema-server/src/constraints.rs:1089-1104`:
```
        // Map key: the codec writes the entry key as a string against the map
        // member schema before the value. Capture it as the value's segment and
        // toggle the frame; the key itself is not a constrained leaf.
        if let Some(CollectionFrame::Map { expecting_key, pending_key, entries }) = ... {
            if *expecting_key {
                *pending_key = Some(value.to_string());
                *expecting_key = false;
                *entries += 1;
                return;
            }
        }
```
(the `return` precedes every `check_leaf`/`check_length`/`check_pattern` call site in that function).
The model the key is supposed to satisfy — `smithy_query(query="shape",shape="Tags")`: `"key": {"target": "TagKey", "length": {"min": 1, "max": 128}, "pattern": "^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]+)$"}`.
The unguarded write — `src/ops/tag_resource.rs:209-211`:
```
        for (key, value) in input.tags {
            group.tags.insert(key, value);
        }
```
Test coverage: `tests/all/resource_tags.rs` pins the stored set, the reserved-prefix refusal and the per-resource maximum, and `tests/all/investigation_group_singleton.rs:1182-1285` pins the capacity refusal; no row in either file, nor in `tests/all/error_message_rules.rs` (whose covered members are listed at `:18-19` as `name`, `roleArn`, `retentionInDays`, `encryptionConfiguration.type`, `identifier`, `maxResults`, `policy`), exercises a tag KEY outside 1..128 or outside the pattern.

## Steps to Reproduce

1. CreateInvestigationGroup in us-west-2 and note the returned ARN
2. POST /tags/<url-encoded ARN> with body {"tags":{"<a 500-character key>":"v"}} — the emulator answers 200 instead of refusing the key that violates TagKey's modelled @length 1..128
3. GET /tags/<url-encoded ARN> — the 500-character key is reported back, so it was persisted into the stored group record
4. Repeat with {"tags":{"bad key!":"v"}} (outside the modelled TagKey @pattern) — also 200 and also stored
5. Same unchecked key class on create: POST /investigationGroups with {"chatbotNotificationChannel":{"not-an-arn":["arn:aws:chatbot::000000000000:chat-configuration/slack-channel/x"]}} is accepted although the map key targets SNSTopicArn (@length 20..2048, ^arn:.*$)
