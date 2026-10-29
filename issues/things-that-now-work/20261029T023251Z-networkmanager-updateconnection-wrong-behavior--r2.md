Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e

# An UpdateConnection that asks for no change still writes UPDATING onto the connection, against the no-op rule the crate's two attachment updaters state and implement

_Filed: 2026-10-29T02:32:51Z_
_Operation: UpdateConnection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConnection` guards each member write with `is_some()` / `if let Some(..)` but applies the settle span unconditionally afterwards. A request supplying none of `Description`, `LinkId` or `ConnectedLinkId` (body `{}`), or one supplying exactly the values the connection already holds, therefore moves the connection out of `AVAILABLE` into `UPDATING` for a settle span, and `GetConnections` reports that transition for a call that changed nothing.

The crate states and implements the opposite rule for the same class of request in its two attachment updaters: a request asking for the state the resource is already in must not move it to `UPDATING` and must write nothing. `UpdateConnection` makes no comparison against the held values.

## Evidence

The rule, stated and implemented in this crate:

- `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_vpc_attachment.rs:322-331`:
  ```
  // A request that asks for the state the attachment is already in changes nothing:
  // it does not move the attachment to `UPDATING`, does not advance `UpdatedAt` and
  // writes nothing, because there is no change for the service to apply.
  if subnet_arns == held_subnets && crate::state::same_vpc_options(..) {
      return Ok(UpdateVpcAttachmentOutput { .. });
  }
  ```
- `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_direct_connect_gateway_attachment.rs:210-218` — a request carrying no `EdgeLocations` "asks for no change, so the attachment is reported as it stands and nothing is written".

The side that breaks it — `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_connection.rs:252-272`:
```
// Only the members the request supplied are changed; an absent member leaves the
// stored value alone ...
if input.description.is_some() { record.description = input.description; }
if let Some(link_id) = requested_link { record.link_id = link_id.map(str::to_string); }
if let Some(connected_link_id) = requested_connected_link { record.connected_link_id = ...; }
...
record.updating_until = Some(now + crate::state::SETTLE_DELAY);
```
The comment above the writes says an absent member "leaves the stored value alone", yet the line below it writes the UPDATING span even when every member was absent.

## Steps to Reproduce

1. CreateGlobalNetwork, CreateDevice twice, then CreateConnection between the two devices, and let the create's settle span elapse so GetConnections reports the connection AVAILABLE.
2. Send `PATCH /global-networks/{GlobalNetworkId}/connections/{ConnectionId}` with the empty body `{}`.
3. Observe the answer reports `Connection.State = "UPDATING"` and GetConnections keeps reporting `UPDATING` for a settle span, although Description, LinkId and ConnectedLinkId are unchanged.
4. Compare with UpdateVpcAttachment / UpdateDirectConnectGatewayAttachment, which both return the resource unchanged and write nothing for a request that asks for no change.
