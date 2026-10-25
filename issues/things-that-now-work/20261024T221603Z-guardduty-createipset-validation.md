Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d2641f8668c2a4abe0c9d800c33fcdc9b555a45a

# CreateIPSet stores a Name outside the documented character set, unlike the sibling entity-set creates

_Filed: 2026-10-24T22:16:03Z_
_Operation: CreateIPSet | Class: validation_

> _Note: this issue was identified against the code base at commit `d2641f8668c2a4abe0c9d800c33fcdc9b555a45a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateIPSet` copies `input.name` straight into the stored `IpSetRecord` with no check on its characters. The model documents the allowed alphabet for that member explicitly, and the modelled constraint is a length only (`Name`, len 1..300, no pattern), so the rule is the handler's to enforce — which is exactly how the crate treats the identical class of rule for the entity sets: `CreateThreatEntitySet` and `CreateTrustedEntitySet` both call `crate::state::is_entity_set_name`, a helper whose own doc comment says it exists because this is "a documented constraint the model's `len 1..300` does not express, so it is enforced here". `CreateIPSet` (and `UpdateIPSet`, which takes the same member) skips it, so a name containing slashes, control characters or arbitrary non-ASCII text is accepted and read back by GetIPSet.

## Evidence

Smithy, `smithy_query(query="doc",shape="CreateIPSetRequest",member="Name")`: "The user-friendly name to identify the IPSet. Allowed characters are alphanumeric, whitespace, dash (-), and underscores (_)." `smithy_query(operation="CreateIPSet")` shows the member carries only `"length": {"min": 1, "max": 300}` and no pattern, so nothing refuses it at the boundary.

The handler stores it unchecked — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/create_ip_set.rs:209-228:
```
        let id = state.mint_id();
        let record = crate::state::IpSetRecord::created(
            id.clone(),
            input.name.clone(),
```
The sibling create in the SAME crate enforces its documented alphabet — src/ops/create_trusted_entity_set.rs:177-181:
```
        if !crate::state::is_entity_set_name(&input.name) {
            return Err(bad_request(crate::state::invalid_parameter_message(
                crate::state::ENTITY_SET_NAME_MEMBER,
            )));
```
and the helper, src/state.rs:3297-3309:
```
/// Does `name` hold only the characters an entity-set `Name` may contain?
/// "The name of your list can include lowercase letters, uppercase letters,
/// numbers, dash (-), and underscore (_)" ... a documented constraint the model's
/// `len 1..300` does not express, so it is enforced here.
pub fn is_entity_set_name(name: &str) -> bool {
```
The current behaviour is pinned by a test whose stated reason is the missing pattern, which the model doc contradicts — tests/all/ip_set_lifecycle.rs:124-129 and 147-155:
```
/// ... `Name` and `Location` are `len 1..300` with no pattern
...
        (
            "a non-ASCII name round-trips byte for byte",
            json!({
                "name": "リスト – トラスト",
```

## Steps to Reproduce

1. CreateDetector to get a detectorId
2. POST /detector/{detectorId}/ipset with {"name":"bad/name!","format":"TXT","location":"s3://b/l.txt","activate":true}
3. Observe HTTP 200 and an ipSetId; GET /detector/{detectorId}/ipset/{ipSetId} reports the name verbatim
4. POST /detector/{detectorId}/trustedentityset with {"name":"bad name",...} for contrast: refused HTTP 400 because the crate enforces that family's documented alphabet
