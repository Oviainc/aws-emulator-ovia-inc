Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7959e6ca7cb81d5c32ad32f8e1e29786f97ad065

# UpdateConnection stores the empty LinkId/ConnectedLinkId verbatim, against the normalisation its own comment claims and CreateConnection applies

_Filed: 2026-10-28T10:05:06Z_
_Operation: UpdateConnection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7959e6ca7cb81d5c32ad32f8e1e29786f97ad065`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConnection` resolves its two optional link references through `crate::state::optional_reference`, whose documented crate-wide rule is that an empty string IS the absence of the reference and is "held as none, exactly as the omitted member is". The handler uses that helper only for the VALIDATION pass (it skips the "must name a link of this global network" lookup for an empty value) and then writes the RAW input into the record: `record.link_id = input.link_id;`. So after `UpdateConnection(LinkId="")` the stored `ConnectionRecord.link_id` is `Some("")`, and `ConnectionRecord::to_shape` reports `Connection$LinkId` as the empty string instead of omitting the member.

That makes the same "no link" input produce two different stored states and two different payloads depending on which call set it: `CreateConnection` normalises first and stores `None` (member absent), `UpdateConnection` stores `Some("")` (member present and empty). The sibling update of the other reference member in this crate, `UpdateDevice`, normalises the same spelling to `None` before writing (`Some("") => Some(None)`), so `UpdateConnection` is the only writer in the crate that leaves a stored reference naming an id nothing can ever be stored under — the exact state `optional_reference`'s doc says the rule exists to prevent.

Note for the fixer: tests/all/connection_lifecycle.rs:523-529 and :537-543 currently PIN the empty-string output (`link: json!("")`) with no stated reason, while the same file's own legend says `Value::Null` is the spelling for "the member is absent" (connection_lifecycle.rs:459-460); those two rows encode the defect and have to move to `Value::Null` with the fix. tests/all/empty_optional_reference.rs:289-360 asserts exactly the absent-member outcome for the CREATE path of the same two members.

## Evidence

The crate-wide rule, src/state.rs:459-477:
```
/// What an OPTIONAL reference member of a request names: the id it carries, or `None`
/// when it names no resource at all.
...
/// On a member that is OPTIONAL the empty string
/// is the ABSENCE of the reference ... So it resolves
/// nothing, is never refused as unresolvable, and is held as none, exactly as the omitted
/// member is: that keeps every stored record's references either naming a resource of the
/// same global network or naming nothing at all.
pub fn optional_reference<Id: AsRef<str>>(supplied: Option<Id>) -> Option<Id> {
    supplied.filter(|id| !id.as_ref().is_empty())
}
```

The comment in the handler that states the same rule — src/ops/update_connection.rs:202-219:
```
        // A link member the request supplies is a reference, resolved WITHIN the global
        // network that owns the connection exactly as `CreateConnection` resolves the one
        // its input carries ... The empty string is the documented REMOVAL of
        // the information the member held ("To remove information for any of the
        // parameters, specify an empty string", `smithy_query query="doc"
        // shape="UpdateConnection"`), so it names no link and resolves nothing
        // ([`crate::state::optional_reference`]).
        for (member, link_id) in [ ... ] {
            let Some(link_id) = crate::state::optional_reference(link_id) else {
                continue;
            };
```
and the write that breaks it — src/ops/update_connection.rs:247-259:
```
        // Only the members the request supplied are changed; an absent member leaves the
        // stored value alone, and a supplied one replaces it — an empty string is how the
        // caller removes the information it holds.
        if input.description.is_some() { record.description = input.description; }
        if input.link_id.is_some() { record.link_id = input.link_id; }
        if input.connected_link_id.is_some() { record.connected_link_id = input.connected_link_id; }
```
(`input.link_id` is the raw `Option<String>`, i.e. `Some("")`.)

The create path, which normalises before storing — src/ops/create_connection.rs:241-249:
```
        // Each link member is OPTIONAL ... and the empty string is one of the two spellings of naming none
        // ([`crate::state::optional_reference`], the same reading `UpdateConnection` and the
        // associates give these members): it resolves nothing and is held as none
        let link_id = crate::state::optional_reference(input.link_id);
        let connected_link_id = crate::state::optional_reference(input.connected_link_id);
```

The sibling update that does normalise before writing — src/ops/update_device.rs:242-244 and :334-336:
```
        let requested_site: Option<Option<String>> = match input.site_id.as_deref() {
            None => None,
            Some("") => Some(None),
...
        if let Some(site_id) = requested_site {
            record.site_id = site_id;
        }
```

What the stored value is reported as — src/state.rs:1850-1851 (`ConnectionRecord::to_shape_with_state`): `link_id: self.link_id.clone(),` / `connected_link_id: self.connected_link_id.clone(),`.

## Steps to Reproduce

1. CreateGlobalNetwork; CreateSite; CreateDevice twice; CreateLink twice at that site
2. CreateConnection(DeviceId=d1, ConnectedDeviceId=d2, LinkId=linkA, ConnectedLinkId=linkB) -> 200
3. PATCH /global-networks/{gn}/connections/{id} with body {"LinkId": ""} -> 200, and the payload reports "LinkId": ""
4. GET /global-networks/{gn}/connections?connectionIds={id} -> the stored connection still carries "LinkId": "" (expected: the member absent, as the crate's optional_reference rule states and as CreateConnection with LinkId="" answers)
5. CreateConnection(DeviceId=d1, ConnectedDeviceId=d2, LinkId="") -> 200 with NO LinkId member: the same input spelling produces two different stored states depending on which operation wrote it
