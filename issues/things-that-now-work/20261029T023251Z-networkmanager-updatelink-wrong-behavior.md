Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e

# An UpdateLink that asks for no change still writes UPDATING onto the link, against the no-op rule the crate's two attachment updaters state and implement

_Filed: 2026-10-29T02:32:51Z_
_Operation: UpdateLink | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateLink` guards every member write with `is_some()` / `if let Some(..)` but then applies the settle span unconditionally. A request supplying none of `Description`, `Type`, `Provider` or `Bandwidth` (body `{}`), or one supplying exactly the values the link already holds, therefore moves the link out of `AVAILABLE` into `UPDATING` for a settle span, and `GetLinks` reports that transition for a call that changed nothing.

The crate states and implements the opposite rule for the same class of request in its two attachment updaters: a request that asks for the state the resource is already in must not move it to `UPDATING` and must write nothing. `UpdateLink` makes no comparison against the held values.

## Evidence

The rule, stated and implemented in this crate:

- `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_vpc_attachment.rs:322-331` — "A request that asks for the state the attachment is already in changes nothing: it does not move the attachment to `UPDATING`, does not advance `UpdatedAt` and writes nothing, because there is no change for the service to apply." followed by the early `return Ok(..)`.
- `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_direct_connect_gateway_attachment.rs:236-245` — "A request that asks for the list the attachment already spans changes nothing: it does not move the attachment to `UPDATING`, does not advance `UpdatedAt` and writes nothing" followed by `if !record.set_edge_locations(requested, now) { return Ok(..) }`.

The side that breaks it — `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_link.rs:216-233`:
```
if input.description.is_some() { record.description = input.description; }
if input.r#type.is_some() { record.link_type = input.r#type; }
if input.provider.is_some() { record.provider = input.provider; }
if let Some(bandwidth) = input.bandwidth { record.bandwidth = bandwidth; }
...
record.updating_until = Some(now + crate::state::SETTLE_DELAY);
```

The crate's own test table names the request a no-op and still pins the moved state — `services/service-com-amazonaws-networkmanager-networkmanager/tests/all/update_link.rs:115-122`:
```
UpdateCase {
    case: "no mutable member at all, so nothing changes",
    body: json!({}),
```
while `tests/all/update_link.rs:297` asserts `json!("UPDATING")` for every row.

## Steps to Reproduce

1. CreateGlobalNetwork, CreateSite, then CreateLink on that site, and let the create's settle span elapse so GetLinks reports the link AVAILABLE.
2. Send `PATCH /global-networks/{GlobalNetworkId}/links/{LinkId}` with the empty body `{}`.
3. Observe the answer reports `Link.State = "UPDATING"` and GetLinks keeps reporting `UPDATING` for a settle span, although Description, Type, Provider and Bandwidth are unchanged.
4. Compare with UpdateVpcAttachment, which returns the attachment unchanged and writes nothing when the request asks for the subnets/options it already holds.
