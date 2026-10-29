Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e

# An UpdateSite that asks for no change still writes UPDATING onto the site, against the no-op rule the crate's two attachment updaters state and implement

_Filed: 2026-10-29T02:32:51Z_
_Operation: UpdateSite | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateSite` guards each member write with `is_some()` but applies the settle span unconditionally afterwards. A request that supplies neither `Description` nor `Location` (body `{}`) — or one that supplies exactly the values the site already holds — therefore moves the site out of `AVAILABLE` into `UPDATING` for a settle span, and `GetSites` reports that transition for a call that changed nothing.

The crate states and implements the opposite rule for the same class of request in its two attachment updaters: a request asking for the state the resource is already in must not move it to `UPDATING`, must not advance the updated instant, and must write nothing. `UpdateSite` has no such comparison against the held values.

## Evidence

The rule, stated and implemented in this crate:

- `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_vpc_attachment.rs:322-324`:
  ```
  // A request that asks for the state the attachment is already in changes nothing:
  // it does not move the attachment to `UPDATING`, does not advance `UpdatedAt` and
  // writes nothing, because there is no change for the service to apply.
  ```
- `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_direct_connect_gateway_attachment.rs:210-213`:
  ```
  // `EdgeLocations` is the only member this operation can change, and it is OPTIONAL:
  // a request that carries none asks for no change, so the attachment is reported as
  // it stands and nothing is written.
  ```

The side that breaks it — `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_site.rs:179-191`:
```
if input.description.is_some() {
    record.description = input.description;
}
if input.location.is_some() {
    record.location = input.location;
}
// An update is not applied instantly: AWS answers the reconfigured site in the
// non-terminal `UPDATING` state ...
let now = ::std::time::SystemTime::now();
record.updating_until = Some(now + crate::state::SETTLE_DELAY);
```

The crate's own test table names the request a no-op and still pins the moved state — `services/service-com-amazonaws-networkmanager-networkmanager/tests/all/update_site.rs:85-90`:
```
UpdateCase {
    case: "no mutable member at all, so nothing changes",
    body: json!({}),
```
and `tests/all/update_site.rs:266-271` asserts `site["State"] == json!("UPDATING")` for every row, including that one.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateSite in it, and let the create's PENDING -> AVAILABLE settle span elapse so GetSites reports the site AVAILABLE.
2. Send `PATCH /global-networks/{GlobalNetworkId}/sites/{SiteId}` with the empty body `{}`.
3. Observe the answer reports `Site.State = "UPDATING"` and GetSites keeps reporting `UPDATING` for a settle span, although neither Description nor Location changed.
4. Compare with UpdateDirectConnectGatewayAttachment, which answers the attachment as it stands and writes nothing when the request carries no `EdgeLocations` (src/ops/update_direct_connect_gateway_attachment.rs:213).
