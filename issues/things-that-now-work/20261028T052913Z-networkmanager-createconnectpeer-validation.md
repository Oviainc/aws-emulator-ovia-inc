Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# CreateConnectPeer accepts a PeerAddress and CoreNetworkAddress of different IP address families, which its own documentation forbids

_Filed: 2026-10-28T05:29:13Z_
_Operation: CreateConnectPeer | Class: validation_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's modelled documentation states a requirement on the pair of addresses the request carries: the Connect peer's two ends must be in the same IP address family. The handler stores both members verbatim and never compares them: `PeerAddress` and `CoreNetworkAddress` are copied straight into the record, so a GRE peer whose core-network end is IPv4 and whose appliance end is IPv6 (or the reverse) is created with 200 and reported by `GetConnectPeer` / `ListConnectPeers` / `ListCoreNetworkRoutingInformation` as an AVAILABLE BGP session that cannot exist.

The handler does validate every OTHER rule of this create that the model leaves to it — the attachment kind, the subnet's Region, the inside CIDR block list — and the sibling create in the same crate quotes its own doc sentence and enforces it (`CreateConnectAttachment` refuses a transport that is not a VPC attachment). This one sentence of the same kind is the only one left unimplemented, and the rule is reproduced verbatim in the handler file's own modeled-contract header.

## Evidence

The model states the rule — smithy_query(query="doc", shape="CreateConnectPeer"): "Creates a core network Connect peer for a specified core network connect attachment between a core network and an appliance. The peer address and transit gateway address must be the same IP address family (IPv4 or IPv6)."

The same sentence is carried in the handler's own header — src/ops/create_connect_peer.rs:39-41:
```
//! Creates a core network Connect peer for a specified core network connect attachment
//! between a core network and an appliance. The peer address and transit gateway address
//! must be the same IP address family (IPv4 or IPv6).
```

The code under it never compares the two — src/ops/create_connect_peer.rs:338-339 is the only place either member is touched:
```
            core_network_address: input.core_network_address,
            peer_address: input.peer_address,
```
and nothing in the crate has a notion of address family at all: `grep -n "ipv4|ipv6|address_family" src/state.rs` matches only `options.ipv6_support` (a VPC-attachment option, state.rs:7933). `check_inside_cidr_blocks` explicitly declines to look inside the strings (state.rs:860-863: "What the blocks CONTAIN is not judged here"), so no helper is reused here either.

The sibling create DOES enforce its own doc sentence — src/ops/create_connect_attachment.rs:207-225:
```
        // "A core network Connect attachment uses an existing VPC attachment as the
        // underlying transport mechanism" (`smithy_query query="doc"
        // shape="CreateConnectAttachment"`), so an attachment of any other kind cannot
        // carry one.
        if transport.attachment_type != crate::state::ATTACHMENT_TYPE_VPC {
            return Err(... ValidationException ...);
```

No test pins the mixed pair as acceptable: tests/all/connect_peer_lifecycle.rs keeps both ends in one family in every row — line 257 "a GRE peer whose two ends are IPv6 addresses" with `peer_address: "2001:db8::1", core_network_address: Some("2001:db8::2")` (lines 259-260) — and tests/all/create_connect_peer_bounds.rs never varies the family of `CoreNetworkAddress`.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a policy document configuring an edge, then CreateVpcAttachment at that edge
2. CreateConnectAttachment over that VPC attachment with Options.Protocol=GRE
3. POST /connect-peers with {"ConnectAttachmentId": "<the connect attachment>", "PeerAddress": "2001:db8::1", "CoreNetworkAddress": "203.0.113.2"}
4. Observe 200 and a stored ConnectPeer whose Configuration reports PeerAddress=2001:db8::1 and CoreNetworkAddress=203.0.113.2 (expected: the modelled ValidationException (400) naming the member that broke the same-family rule, as the handler's own header states it)
5. GetConnectPeer on the returned PeerId keeps reporting the mixed-family session, and after the settle span it reads AVAILABLE
