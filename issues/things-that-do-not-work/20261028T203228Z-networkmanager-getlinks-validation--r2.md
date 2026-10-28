Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f071a90384ece90fe46e5865745f415e3851c94

# An empty siteId counts as "specified" for the SiteId/Type/Provider mutual exclusion, so GetLinks refuses a request that names no site

_Filed: 2026-10-28T20:32:28Z_
_Operation: GetLinks | Class: validation_

> _Note: this issue was identified against the code base at commit `0f071a90384ece90fe46e5865745f415e3851c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetLinks` enforces the documented rule "If you specify the site ID, you cannot specify the type or provider in the same request" by testing `input.site_id.is_some()`. `SiteId` is an OPTIONAL reference member modelled `length 0..50` over the permissive `^[\s\S]*$`, so the query string `?siteId=` reaches the handler as `Some("")`.

This crate states one rule for exactly that value: the empty string on an optional reference member is the ABSENCE of the reference, read "exactly as the omitted member is", and `crate::state::optional_reference` is the one function that applies it (six handlers of this crate go through it). Under that rule `?siteId=` specifies NO site — yet `is_some()` says it does, so `GET /global-networks/{id}/links?siteId=&type=router` is answered with a 400 ValidationException naming `Type`, while the identical request with `siteId` simply omitted is served with 200.

The same `Some("")` then also reaches the selector below (`record.site_id == want`), where it matches no stored link at all — a link always sits at a site, so an empty `siteId` filter selects nothing instead of selecting everything the way the omitted member does. Both symptoms come from the one missing `optional_reference` call.

## Evidence

The handler's test for "specified" — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_links.rs:149-158:
```
// "If you specify the site ID, you cannot specify the type or provider in the
// same request. You can specify the type and provider in the same request."
// (`smithy_query query="doc" shape="GetLinks"`) — a rule the model cannot
// express as a constraint, so it is refused here.
if input.site_id.is_some() {
    let conflicting: Vec<&str> = [("Type", &input.r#type), ("Provider", &input.provider)]
```
and the selector that uses the same raw value — get_links.rs:225-230:
```
.filter(|(_, record)| {
    input
        .site_id
        .as_deref()
        .is_none_or(|want| record.site_id == want)
})
```

The crate's own rule for this value (kind B, other side) — services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:460-477:
```
/// What an OPTIONAL reference member of a request names: the id it carries, or `None`
/// when it names no resource at all.
...
/// On a member that is OPTIONAL the empty string
/// is the ABSENCE of the reference ... So it resolves
/// nothing, is never refused as unresolvable, and is held as none, exactly as the omitted
/// member is
pub fn optional_reference<Id: AsRef<str>>(supplied: Option<Id>) -> Option<Id> {
    supplied.filter(|id| !id.as_ref().is_empty())
}
```
and the crate's test module that states it as crate-wide — services/service-com-amazonaws-networkmanager-networkmanager/tests/all/empty_optional_reference.rs:3-6:
```
//! The rule every handler of this crate applies to an OPTIONAL reference member, pinned
//! end to end over HTTP: the empty string names no resource, so it resolves nothing and is
//! held as none — exactly as omitting the member is — while any non-empty value stays a
//! reference that must name a resource of the same global network.
```

The model confirms the member admits the empty string: `smithy_query query="operation" operation="GetLinks"` gives `"SiteId": {"target": "SiteId", "httpQuery": "siteId", "length": {"min": 0, "max": 50}, "pattern": "^[\\s\\S]*$"}` — optional, minimum length 0.

Test coverage: tests/all/link_lifecycle.rs and tests/all/link_ids_filter_maximum.rs drive `GetLinks` with `siteId`, `type` and `provider` present or absent, but never with any of them spelled empty; tests/all/empty_optional_reference.rs covers the write paths (CreateDevice, CreateConnection, AssociateCustomerGateway) and no listing, so no test pins today's behaviour.

## Steps to Reproduce

1. CreateGlobalNetwork, CreateSite, CreateLink (type=router) on that site.
2. GET /global-networks/{network}/links?type=router -> 200, the link is listed.
3. GET /global-networks/{network}/links?siteId=&type=router -> 400 ValidationException with Fields[0].Name = "Type" and Message "SiteId cannot be combined with Type in the same request" (expected: served like step 2, because `siteId=` names no site under this crate's own optional-reference rule).
4. GET /global-networks/{network}/links?siteId= -> 200 with an EMPTY Links list (expected: every link of the global network, exactly as omitting siteId answers).
