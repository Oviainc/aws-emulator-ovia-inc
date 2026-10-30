Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f071a90384ece90fe46e5865745f415e3851c94

# An empty deviceId/linkId satisfies the documented "either the device ID or the link ID must be specified" rule, so the refusal is skipped and an empty page is served

_Filed: 2026-10-28T20:32:28Z_
_Operation: GetLinkAssociations | Class: validation_

> _Note: this issue was identified against the code base at commit `0f071a90384ece90fe46e5865745f415e3851c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetLinkAssociations` enforces the documented rule "Either the device ID or the link ID must be specified" by testing `input.device_id.is_none() && input.link_id.is_none()`. Both members are OPTIONAL and modelled `length 0..50` over the permissive `^[\s\S]*$`, so `?deviceId=` arrives as `Some("")`.

This crate states one rule for that value — the empty string on an optional reference member is the ABSENCE of the reference, read "exactly as the omitted member is", applied through `crate::state::optional_reference` by six sibling handlers. Under that rule `?deviceId=` selects no subject at all, so the request names neither a device nor a link and must get the modelled 400. Instead `is_none()` is false, the refusal is skipped, and the request is answered 200 — and because the raw `Some("")` then reaches the selector (`record.device_id == want`), the page is EMPTY, which reads to a client as "this device/link has no associations" rather than "you named no subject".

The same omission also makes a half-specified request lose its other selector: `?deviceId=&linkId=<real link>` answers an empty page where omitting `deviceId` answers that link's associations.

## Evidence

The handler's test for "specified" — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_link_associations.rs:145-153:
```
// "Gets the link associations for a device or a link. Either the device ID or the
// link ID must be specified." (`smithy_query query="doc"
// shape="GetLinkAssociations"`) — both members are optional in the model, which
// cannot express "at least one of", so the rule is refused here: a request that
// selects neither names no subject to report associations for. ...
if input.device_id.is_none() && input.link_id.is_none() {
    let reason = "either DeviceId or LinkId must be specified";
```
and the selectors that use the same raw values — get_link_associations.rs:209-220:
```
.filter(|(_, record)| {
    input
        .device_id
        .as_deref()
        .is_none_or(|want| record.device_id == want)
})
.filter(|(_, record)| {
    input
        .link_id
        .as_deref()
        .is_none_or(|want| record.link_id == want)
})
```

The crate's own rule for this value (kind B, other side) — services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:460-477:
```
/// What an OPTIONAL reference member of a request names: the id it carries, or `None`
/// when it names no resource at all.
...
/// So it resolves
/// nothing, is never refused as unresolvable, and is held as none, exactly as the omitted
/// member is
pub fn optional_reference<Id: AsRef<str>>(supplied: Option<Id>) -> Option<Id> {
    supplied.filter(|id| !id.as_ref().is_empty())
}
```
stated as crate-wide in services/service-com-amazonaws-networkmanager-networkmanager/tests/all/empty_optional_reference.rs:3-6: "The rule every handler of this crate applies to an OPTIONAL reference member ... the empty string names no resource, so it resolves nothing and is held as none — exactly as omitting the member is".

The model confirms the members admit the empty string: `smithy_query query="operation" operation="GetLinkAssociations"` gives `"DeviceId": {..., "httpQuery": "deviceId", "length": {"min": 0, "max": 50}, "pattern": "^[\\s\\S]*$"}` and the same for `LinkId` — both optional, minimum length 0.

Test coverage: tests/all/link_association_lifecycle.rs drives GetLinkAssociations with deviceId only, linkId only, both, and neither (the 400 case), but never with either spelled empty, so no test pins today's behaviour.

## Steps to Reproduce

1. CreateGlobalNetwork, CreateSite, CreateDevice at that site, CreateLink on that site, AssociateLink(device, link).
2. GET /global-networks/{network}/link-associations -> 400 ValidationException "either DeviceId or LinkId must be specified".
3. GET /global-networks/{network}/link-associations?deviceId= -> 200 with an empty LinkAssociations list (expected: the same 400 as step 2, because `deviceId=` names no device under this crate's own optional-reference rule).
4. GET /global-networks/{network}/link-associations?deviceId=&linkId={link} -> 200 with an empty list, while GET ...?linkId={link} -> 200 listing the association: the empty member silently dropped the result the other selector asked for.
