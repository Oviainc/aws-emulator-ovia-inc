Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1

# An empty SiteId is refused by CreateDevice although UpdateDevice (and the crate's other handlers) read it as "no site"

_Filed: 2026-10-28T07:20:04Z_
_Operation: CreateDevice | Class: validation_

> _Note: this issue was identified against the code base at commit `c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`SiteId` is optional on both `CreateDevice` and `UpdateDevice` and both target the same modelled shape (`len 0..50`, `pattern ^[\s\S]*$`), so the empty string reaches the handler. `UpdateDevice` normalises it away as the documented removal of the assignment — `Some("") => Some(None)`, leaving the device unsited — and the crate's tests pin that spelling. `CreateDevice` skips the normalisation and feeds the raw member into the store lookup, so `store.sites.get("")` misses and the create is refused with ValidationException (400) naming `SiteId` as an unresolvable site reference, instead of minting the unsited device that omitting the member produces.

So the same request body shape — `SiteId: ""` — means "no site" on the update and is a 400 on the create of the same resource, and a client that spells an absent optional reference as the empty string cannot create a device at all. `CreateConnection` takes the same raw path for its two optional link references (create_connection.rs:245-263, no empty filter) while `UpdateConnection` filters the empty string out with the service's own "specify an empty string" doc sentence, so the inconsistency is the same one in that pair.

## Evidence

The normalisation the update applies — src/ops/update_device.rs:232-245:
```
        // `SiteId` is the one member of this request that is a REFERENCE rather than a
        // free-form value, so it is resolved before anything is written: an absent member
        // leaves the device where it sits, the empty string removes the assignment ("To
        // remove information for any of the parameters, specify an empty string",
        // `smithy_query query="doc" shape="UpdateDevice"`) and leaves the device unsited —
        let requested_site: Option<Option<String>> = match input.site_id.as_deref() {
            None => None,
            Some("") => Some(None),
```

The path that skips it — src/ops/create_device.rs:242-261:
```
        // `SiteId` is optional, so a device that names none is an unsited device of the
        // global network. When the request does carry one it is a reference like any
        // other, resolved WITHIN the global network the path addresses
        if let Some(site_id) = input.site_id.as_deref() {
            let site_resolves = store
                .sites
                .get(site_id)
                .is_some_and(|site| site.global_network_id == input.global_network_id);
            if !site_resolves {
                return Err(CreateDeviceErrorModeled::ValidationException(
                    crate::state::unknown_reference(
                        crate::state::FIELD_SITE_ID,
```
and the raw member is then stored at create_device.rs:297 (`site_id: input.site_id,`).

The same split in the connection pair — src/ops/update_connection.rs:205-218 ("The empty string is the documented REMOVAL of the information the member held ... so it names no link and resolves nothing" / `let Some(link_id) = link_id.filter(|supplied| !supplied.is_empty()) else { continue };`) versus src/ops/create_connection.rs:252 (`let Some(link_id) = link_id else { continue };`).

The crate's tests pin the empty spelling as supported for the update and never exercise it on the create: tests/all/update_device.rs:260-261 `case: "an empty SiteId leaves the device unsited", body: json!({ "SiteId": "" })`, and tests/all/connection_lifecycle.rs:523-529 `case: "an empty LinkId removes the link it held"`; no test sends `"SiteId": ""` to CreateDevice.

## Steps to Reproduce

1. CreateGlobalNetwork -> global network id
2. POST /global-networks/{id}/devices with {"SiteId": "", "Description": "d"} -> 400 ValidationException with Fields naming SiteId (expected: the unsited device that omitting SiteId creates, since the update of the same member reads "" as "no site")
3. POST /global-networks/{id}/devices with {} -> 200, device created unsited
4. PATCH /global-networks/{id}/devices/{device} with {"SiteId": ""} -> 200, device unsited: the identical member value is accepted by the update and refused by the create
