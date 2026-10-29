Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f08062a3928279f71031748dc453b175f9086c5b

# An empty siteId filter is matched literally, so GetDevices reports an empty page for a request that names no site

_Filed: 2026-10-28T22:49:35Z_
_Operation: GetDevices | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f08062a3928279f71031748dc453b175f9086c5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetDevicesRequest$SiteId` is an OPTIONAL reference member whose shape (`SiteId`, `length 0..50`, `pattern ^[\s\S]*$`) admits the empty string, so `GET /global-networks/{id}/devices?siteId=` reaches the handler as `Some("")`.

The crate has ONE stated reading for that input class, in `crate::state::optional_reference`: the empty string on an optional reference member is the ABSENCE of the reference, held as none exactly as an omitted member is. The two WRITERS of a device's `site_id` go through that helper (`create_device.rs` via `optional_reference`, `update_device.rs` via `requested_reference`), so a stored device's `site_id` is either `Some(<a real site id>)` or `None` — never `Some("")`.

`get_devices.rs` is the one READER of those records and does not go through the helper: it compares `record.site_id.as_deref() == Some(want)` with `want == ""`. Because the writers guarantee no device is ever stored under the empty site id, that comparison can never be true, so `?siteId=` answers 200 with an EMPTY `Devices` list for a global network that holds devices — instead of the unfiltered listing the crate's own rule says an absent reference asks for. The caller gets "this global network holds no devices", which is the opposite of the truth.

## Evidence

The rule, stated once in this crate — `services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:460-478`:
```
/// What an OPTIONAL reference member of a request names: the id it carries, or `None`
/// when it names no resource at all.
///
/// Every id shape of this service is modelled `len 0..50` over `^[\s\S]*$`
/// (`smithy_query query="shape" shape="SiteId"`), so the empty string satisfies the
/// constraint walk and reaches the handler. On a member that is OPTIONAL the empty string
/// is the ABSENCE of the reference ... So it resolves nothing, is never refused as
/// unresolvable, and is held as none, exactly as the omitted member is
pub fn optional_reference<Id: AsRef<str>>(supplied: Option<Id>) -> Option<Id> {
    supplied.filter(|id| !id.as_ref().is_empty())
}
```

The writers of the same member DO call it — `src/ops/create_device.rs:251`:
```
let site_id = crate::state::optional_reference(input.site_id);
```
and `src/ops/update_device.rs:243`:
```
match crate::state::requested_reference(input.site_id.as_deref()) {
```

The reader does NOT — `src/ops/get_devices.rs:201-206`:
```
.filter(|(_, record)| {
    input
        .site_id
        .as_deref()
        .is_none_or(|want| record.site_id.as_deref() == Some(want))
})
```
The surrounding comment only reasons about non-empty filters: "Every filter the request carries narrows the global network's devices; an absent one matches everything ... A device that was created without a site matches no `SiteId` filter." (`src/ops/get_devices.rs:193-195`) — the empty-string spelling of "absent" is never considered.

The model confirms the member admits it (`smithy_query operation=GetDevices`): `"SiteId": {"target": "SiteId", "httpQuery": "siteId", "length": {"min": 0, "max": 50}, "pattern": "^[\\s\\S]*$"}`.

Test coverage: `tests/all/device_lifecycle.rs:413-443` and `tests/all/update_device.rs:670` exercise `siteId=<real site>` only; `tests/all/empty_optional_reference.rs:164` (`create_device_reads_an_empty_site_id_as_no_site`) pins the WRITER side of the rule but no test exercises `?siteId=` on the reader, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateSite in it, then CreateDevice in that global network (with or without a SiteId) — note the device id.
2. GET /global-networks/{GlobalNetworkId}/devices with no query string: the device is listed.
3. GET /global-networks/{GlobalNetworkId}/devices?siteId= (empty value).
4. Observe 200 with an EMPTY `Devices` list, although the request names no site and `crate::state::optional_reference`'s stated rule makes the empty string equivalent to omitting the member (which lists every device).
