Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e

# An UpdateDevice that asks for no change still writes UPDATING onto the device, against the no-op rule the crate's two attachment updaters state and implement

_Filed: 2026-10-29T02:32:51Z_
_Operation: UpdateDevice | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d4e6bf007f35abe24a64d7f0825cf70aa7f81d5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateDevice` applies the settle span unconditionally: after the reference checks it writes `record.updating_until = Some(now + SETTLE_DELAY)` whatever the request asked for. A request that supplies NO mutable member at all (`PATCH /global-networks/{id}/devices/{id}` with body `{}`), or one that supplies exactly the values the device already holds, therefore moves the device out of `AVAILABLE` into `UPDATING` for a settle span and makes the next `GetDevices` report a transition that corresponds to no change.

This crate states the opposite rule for the same kind of request and implements it, twice, in its two attachment updaters: an update that asks for the state the resource is already in must not move it to `UPDATING`, must not advance `UpdatedAt`, and must write nothing. `UpdateDevice` skips that branch entirely — it has no comparison against the held values at all.

The crate's own test table even labels the case as a no-op while asserting the state moved, so the two sides of the crate disagree in writing.

## Evidence

The rule, stated and implemented in this crate:

- `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_vpc_attachment.rs:322-331`:
  ```
  // A request that asks for the state the attachment is already in changes nothing:
  // it does not move the attachment to `UPDATING`, does not advance `UpdatedAt` and
  // writes nothing, because there is no change for the service to apply.
  if subnet_arns == held_subnets
      && crate::state::same_vpc_options(options.as_ref(), held_options.as_ref())
  {
      return Ok(UpdateVpcAttachmentOutput {
          vpc_attachment: record.to_vpc_attachment(now),
      });
  }
  ```
- `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_direct_connect_gateway_attachment.rs:210-218`:
  ```
  // `EdgeLocations` is the only member this operation can change, and it is OPTIONAL:
  // a request that carries none asks for no change, so the attachment is reported as
  // it stands and nothing is written.
  let Some(requested) = input.edge_locations else {
      return Ok(UpdateDirectConnectGatewayAttachmentOutput { ... });
  };
  ```

The side that breaks it — `services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_device.rs:314-343`, every write guarded by `is_some()` but the settle span not:
```
if input.description.is_some() { record.description = input.description; }
... (type, vendor, model, serial_number, aws_location, location, site_id)
record.updating_until = Some(now + crate::state::SETTLE_DELAY);
```

The crate's own test table names the request a no-op and still pins the moved state — `services/service-com-amazonaws-networkmanager-networkmanager/tests/all/update_device.rs:158-159`:
```
case: "no mutable member at all, so nothing changes",
body: json!({}),
```
while the same test asserts `json!("UPDATING")` for every row (`tests/all/update_device.rs:396`).

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateDevice in it, and wait for the create's PENDING -> AVAILABLE settle span to pass so GetDevices reports the device AVAILABLE.
2. Send `PATCH /global-networks/{GlobalNetworkId}/devices/{DeviceId}` with the empty body `{}` (no mutable member supplied).
3. Observe the answer reports `Device.State = "UPDATING"` and GetDevices keeps reporting `UPDATING` for a settle span, although nothing about the device changed.
4. Compare with UpdateVpcAttachment: re-submit the subnets and options an attachment already holds and observe it stays AVAILABLE and nothing is written (src/ops/update_vpc_attachment.rs:322).
