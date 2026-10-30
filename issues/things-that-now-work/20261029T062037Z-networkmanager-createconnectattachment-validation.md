Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 68c868d69b495651e8ae949e9225c232ca965615

# A Connect attachment is built on a transport attachment that was REJECTED (or is still waiting for acceptance), although the crate says such an attachment "is never created"

_Filed: 2026-10-29T06:20:37Z_
_Operation: CreateConnectAttachment | Class: validation_

> _Note: this issue was identified against the code base at commit `68c868d69b495651e8ae949e9225c232ca965615`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConnectAttachment` resolves `TransportAttachmentId` and checks exactly three things about the record it finds: that it belongs to the same core network, that its `attachment_type` is `VPC`, and that its `edge_location` equals the requested one. It never looks at the transport's LIFECYCLE STATE.

So a transport attachment that the core network policy held for acceptance and that `RejectAttachment` then turned down — a record the crate itself documents as terminal and as one that "is never created" — is accepted as the "existing VPC attachment" this operation's own quoted model sentence requires. The same hole admits a transport still sitting in `PENDING_ATTACHMENT_ACCEPTANCE`, which the crate documents as "nothing is being created until the request is accepted".

The result is a stored Connect attachment whose `TransportAttachmentId` names an attachment that will never exist as a tunnel: the Connect attachment itself settles to `AVAILABLE` on the normal `SETTLE_DELAY` (its own `available_at`), is reported by `GetConnectAttachment`/`ListAttachments`, can carry Connect peers, and can originate routes in `ListCoreNetworkRoutingInformation` (which checks only the CONNECT attachment's state, ops/list_core_network_routing_information.rs:493). It also traps the rejected record: `DeleteAttachment` counts the Connect attachments carried over a transport as blocking dependents, so the rejected attachment can no longer be deleted until the attachment that should never have been built on it is deleted first.

The liveness notion is not a new concept for this crate — `AttachmentRecord::state_at`, `awaits_acceptance`, `STATE_REJECTED` and `STATE_AVAILABLE` already exist and are used by `UpdateVpcAttachment`, `UpdateDirectConnectGatewayAttachment`, `AcceptAttachment`, `RejectAttachment` and `ListCoreNetworkRoutingInformation`; only this create ignores it for the reference it builds on.

## Evidence

The model sentence the handler itself quotes, and the only three checks it makes of the transport:
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_connect_attachment.rs:191-205
  "// The transport is resolved WITHIN that core network: an attachment that never
   // existed, and one stored under a different core network, both leave
   // `TransportAttachmentId` naming nothing this request can build on.
   let transport = store
       .attachments
       .get(&input.transport_attachment_id)
       .filter(|transport| transport.core_network_id == input.core_network_id)"
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_connect_attachment.rs:207-211
  "// \"A core network Connect attachment uses an existing VPC attachment as the
   // underlying transport mechanism\" (`smithy_query query=\"doc\"
   // shape=\"CreateConnectAttachment\"`), so an attachment of any other kind cannot
   // carry one.
   if transport.attachment_type != crate::state::ATTACHMENT_TYPE_VPC {"
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_connect_attachment.rs:232
  "if input.edge_location != transport.edge_location {"
— and nothing between line 191 and the insert at line 376 reads `transport.state_at(..)`, `transport.rejected_at` or `transport.awaits_acceptance()`.

Smithy (kind A), `smithy_query query="doc" shape="CreateConnectAttachment"`:
  "A core network Connect attachment uses an existing VPC attachment as the underlying transport mechanism."

The crate's own statement that a rejected / still-pending attachment is NOT such an existing attachment (kind B):
services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:2639-2643
  "/// When the attachment request was TURNED DOWN, or `None` for one nothing rejected.
   /// Terminal: an attachment holding an instant here is [`STATE_REJECTED`] from that
   /// instant on, is never created, and has no transition owed on it
   /// ([`Self::reject_at`]).
   pub rejected_at: Option<SystemTime>,"
services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:2630-2634
  "/// The instant the `CREATING` -> `AVAILABLE` transition the clock owes becomes due, or
   /// `None` while the attachment is still waiting for the acceptance the core network
   /// policy asked for: nothing is being created until the request is accepted, so no
   /// transition is owed on it until then"

Sibling handlers in this crate that DO gate on the attachment state the clock computes:
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_vpc_attachment.rs:185-186
  "let held_state = record.state_at(now);
   if held_state != crate::state::STATE_AVAILABLE {"
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/update_direct_connect_gateway_attachment.rs:192-193 (identical gate)
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/reject_attachment.rs:168-171
  "there is a request to reject only while the attachment
   // is waiting for one ... An attachment
   // already under way, already available, or already rejected is in the wrong state"

The refusal that then traps the rejected record:
services/service-com-amazonaws-networkmanager-networkmanager/src/ops/delete_attachment.rs:155-174
  "// A Connect attachment is built on the attachment named as its transport, so
   // removing that transport would leave it pointing at nothing. ...
   let carried = store.connect_attachment_count_over_transport(&input.attachment_id);
   ... return Err(DeleteAttachmentErrorModeled::ValidationException(refusal).into());"

Test coverage: tests/all/attachment_lifecycle.rs:547-603 tables the transport refusals this create DOES make ("a transport id that names nothing", "a transport of another core network", "a Connect attachment as the transport", "an edge location that is not the transport's"); no row covers a rejected or still-pending transport, and tests/all/attachment_rejection.rs does not build a Connect attachment on a rejected one, so no existing test pins the current behaviour.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a policy document that configures an edge in us-west-2 and carries an `attachment-policies` rule claiming VPC attachments at that edge with require-acceptance set.
2. CreateVpcAttachment for a us-west-2 VPC on that core network; GetVpcAttachment reports State = PENDING_ATTACHMENT_ACCEPTANCE.
3. RejectAttachment on that attachment id; GetVpcAttachment now reports State = REJECTED, which state.rs calls terminal and "never created".
4. CreateConnectAttachment with CoreNetworkId = the core network, EdgeLocation = us-west-2, TransportAttachmentId = the REJECTED attachment id, Options.Protocol = GRE.
5. Observed: HTTP 200 — a Connect attachment is stored whose TransportAttachmentId names the rejected attachment, and after the settle delay GetConnectAttachment reports it AVAILABLE.
6. Call DeleteAttachment on the rejected transport: HTTP 400 ValidationException (the dependency refusal of delete_attachment.rs:166-174), so the rejected attachment is now undeletable until the Connect attachment built on it is deleted.
7. Same outcome without step 3: calling CreateConnectAttachment while the transport is still PENDING_ATTACHMENT_ACCEPTANCE is also answered 200.
