Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 07e288609021e6571eb22a2173d293968d6da2ab

# The reserved-tag-key refusal echoes a tag key out of the @sensitive TagsMap into the ValidationException message

_Filed: 2026-10-29T13:32:19Z_
_Operation: CreateConfigurationManager | Class: error-response_

> _Note: this issue was identified against the code base at commit `07e288609021e6571eb22a2173d293968d6da2ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConfigurationManager$Tags` and `TagResource$Tags` both target `TagsMap`, which the model marks `@sensitive` — the trait applies to the aggregate shape, so both the keys and the values stored in that map are sensitive data that must not be exposed in exception messages. The shared refusal built by `crate::state::reserved_tag_key_rejection` interpolates the offending key verbatim into the `ValidationException` `Message`, so a caller-supplied key out of a sensitive map is rendered back on the wire (and into anything that logs the response body).

The same helper is also used for `UntagResource$TagKeys`, which is a bare `list<String>` with no `@sensitive` anywhere in its path — there the echo is unobjectionable. The defect is only that the one helper treats the sensitive `Tags` members exactly like the non-sensitive `TagKeys` list: the refusal for a `Tags` map must state the member and the reserved prefix (both request-independent) without rendering the key itself.

This does not touch the rule the fixed issue 20261024T170559Z introduced: a reserved key is still refused with 400 `ValidationException` and nothing is stored; only the sensitive value in the message body changes. `tests/all/reserved_tag_keys.rs` currently asserts the key appears in the message for all three members, so that assertion has to be narrowed to the `TagKeys` path.

## Evidence

The model marks the map sensitive — `smithy_query(query="shape", shape="TagsMap")`:
```
"TagsMap": {"shape_id": "com.amazonaws.ssmquicksetup#TagsMap", "type": "map", "sensitive": true,
  "key": {"target": "String", "length": {"min": 1, "max": 128}, "pattern": "^[A-Za-z0-9 _=@:.+-/]+$"}, ...}
```
and both write paths take their tags through it — `smithy_query(query="operation", operation="CreateConfigurationManager")`: `"Tags": {"target": "TagsMap", "sensitive": true, "type": "map"}`; `operation="TagResource"`: `"Tags": {"target": "TagsMap", "required": true, "sensitive": true, "type": "map"}`.

The refusal renders the key — `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:1518-1532`:
```
pub fn reserved_tag_key_rejection<'k>(
    member: &str,
    keys: impl IntoIterator<Item = &'k str>,
) -> Option<String> {
    keys.into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .min()
        .map(|key| {
            format!(
                "{member} must not name a tag key under the '{RESERVED_TAG_PREFIX}' prefix, \
                 which is reserved for Amazon Web Services use and cannot be edited or \
                 deleted, got '{}'",
                echoed_value(key)
            )
        })
```
reached with the sensitive map's keys from `src/ops/create_configuration_manager.rs:213-220`:
```
        crate::state::reserved_tag_key_rejection(
            "Tags",
            input
                .tags
                .iter()
                .flat_map(std::collections::HashMap::keys)
                .map(String::as_str),
        )
```
and from `src/ops/tag_resource.rs:157`:
```
    crate::state::reserved_tag_key_rejection("Tags", tags.keys().map(String::as_str))
```
while the non-sensitive member that may keep the echo is `src/ops/untag_resource.rs:158`:
```
    crate::state::reserved_tag_key_rejection("TagKeys", tag_keys.iter().map(String::as_str))
```
(`smithy_query(query="shape", shape="TagKeys")` -> `{"type": "list", "member": "String"}`, no sensitive trait.)

Test coverage: `tests/all/reserved_tag_keys.rs:396-420` pins the echo for every member, on the stated reason "Every refusal states the member, the reserved prefix and the offending key" — a wording/consistency reason that says nothing about sensitivity:
```
    assert!(
        message.contains(key),
        "case {case:?}: the refusal must name the key {key:?}, got {message:?}"
    );
```

## Steps to Reproduce

1. Boot the emulator with the ssm-quicksetup service mounted and sign requests for us-west-2.
2. POST /configurationManager with {"ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}],"Tags":{"aws:secretOwner":"v"}}.
3. Observed: 400 ValidationException whose Message reads "Tags must not name a tag key under the 'aws:' prefix, ... got 'aws:secretOwner'" — the key of a @sensitive TagsMap is rendered back on the wire.
4. Repeat against an existing manager with PUT /tags/{ManagerArn} and body {"Tags":{"aws:secretOwner":"v"}}: the same sensitive key is echoed.
5. Expected: the refusal still answers 400 ValidationException naming the member ("Tags") and the reserved prefix ("aws:"), but renders no value out of the sensitive map; DELETE /tags/{ManagerArn}?tagKeys=aws:secretOwner may keep naming the key, since UntagResource$TagKeys is not sensitive.
