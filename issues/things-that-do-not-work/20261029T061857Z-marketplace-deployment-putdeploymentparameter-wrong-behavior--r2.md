Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 68c868d69b495651e8ae949e9225c232ca965615

# PutDeploymentParameter reports tag entries its own TagsMap response member cannot carry

_Filed: 2026-10-29T06:18:57Z_
_Operation: PutDeploymentParameter | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `68c868d69b495651e8ae949e9225c232ca965615`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutDeploymentParameter` refuses, on the way IN, any tag key outside the character class its `tags` member's target map (`TagsMap`) states — but on the way OUT it copies the record's whole stored tag map, unfiltered, into the response member that targets the SAME `TagsMap`.

The record's tag map is also written by `TagResource`, which merges with `TagConstraints::Unconstrained` and therefore stores keys and values that `TagsMap` cannot represent (a key carrying a space, a key of non-ASCII letters, an EMPTY value — `TagValue` is `length 1..256`). Once such a tag is on the record, the next `PutDeploymentParameter` for the same identity (an update in place, which reports the stored tag map) serializes those entries into `PutDeploymentParameterResponse$tags`, a member whose modelled shape forbids them. So one operation refuses on input exactly what it emits on output through the same shape, and a client that validates or strongly types the response against the model gets a body the model does not admit.

The crate already applies the rule "a stored tag set must be reportable through this response's `TagsMap`" — it is the stated justification for the per-resource tag COUNT bound in `state.rs` — but the same reasoning is not applied to the key/value character class or to the value's minimum length, and nothing in the crate confronts the conflict for the response.

## Evidence

Model (smithy_query query="operation" operation="PutDeploymentParameter"), output member: `"tags": {"target": "TagsMap", "length": {"max": 50}, "type": "map"}`; and (smithy_query query="shape" shape="TagsMap"): `"key": {"target": "TagKey", "length": {"min": 1, "max": 128}, "pattern": "^[a-zA-Z0-9/_+=.:@-]+$"}, "value": {"target": "TagValue", "length": {"min": 1, "max": 256}, "pattern": "^[a-zA-Z0-9/_+=.:@-]+$"}`.

INPUT side of the same operation refuses what the class excludes — `src/ops/put_deployment_parameter.rs:249-255`:
```rust
if let Some(tags) = input.tags.as_ref() {
    if let Err(why) =
        crate::state::validate_tags_member(tags, crate::state::TagConstraints::TagsMap)
```

OUTPUT side emits the stored map unfiltered — `src/ops/put_deployment_parameter.rs:443-449`:
```rust
fn output_of(record: &crate::state::DeploymentParameterRecord) -> PutDeploymentParameterOutput {
    PutDeploymentParameterOutput {
        ...
        tags: Some(record.tags_map()),
```

What can get into that stored map — `src/ops/tag_resource.rs:232-238`:
```rust
let merged = crate::state::merge_tags(
    &record.tags,
    &incoming,
    crate::state::TagConstraints::Unconstrained,
)
```
and `src/state.rs:184-190`: "The member targets a tag map the model leaves unconstrained ... so any UTF-8 text inside them is stored, spaces, letters outside ASCII and an empty value included."

The crate's OWN rule that a stored set must be reportable through this very response — `src/state.rs:135-140`: "AWS's per-resource tag limit, which the model restates for its constrained copy of the tag map: `PutDeploymentParameterRequest$tags` and `PutDeploymentParameterResponse$tags` are a `TagsMap` with `length max 50` ..., so a stored set larger than this could not be reported back through the response at all."

Existing test proving such tags ARE stored and read back — `tests/all/unconstrained_tag_characters.rs:107-112`:
```rust
tags(&[
    ("Cost Center".to_string(), "".to_string()),
    ("Résumé".to_string(), "maître d'hôtel".to_string()),
```
asserted accepted (204) and listed back by `an_unconstrained_tag_set_is_stored_and_listed_back` (line 126). No test asserts what `PutDeploymentParameter`'s response carries after such a write, so this path is unpinned.

## Steps to Reproduce

1. PutDeploymentParameter with catalog=AWSMarketplace, productId=prod-default-1, agreementId=agmt-default-1, deploymentParameter={name:"P", secretString:"s"} — 200; note the minted resourceArn.
2. TagResource on that resourceArn with tags={"Cost Center": ""} — 204 (accepted, per tests/all/unconstrained_tag_characters.rs:107-112, because `TagResource$tags` targets the unconstrained `Tags`).
3. PutDeploymentParameter again for the same (catalog, productId, agreementId, deploymentParameter.name) — 200, update in place, and the response `tags` contains "Cost Center": "".
4. Compare that entry with the response member's own shape: the key carries a space (outside `TagKey`'s `^[a-zA-Z0-9/_+=.:@-]+$`) and the value is empty (below `TagValue`'s `length min 1`), so `PutDeploymentParameterResponse$tags` carries a pair its modelled `TagsMap` cannot represent.
5. Send the same pair through this operation's own `tags` member instead (PutDeploymentParameter with tags={"Cost Center":"x"}) — 400 ValidationException from src/ops/put_deployment_parameter.rs:249, i.e. the operation refuses on input what it emits on output.
