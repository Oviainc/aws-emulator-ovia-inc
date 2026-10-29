Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6cdf7a4bb780a1593da31764ba2b1e15a7af6027

# A Connect peer's protocol never gates the two protocol-specific members, so a GRE peer is stored with a NO_ENCAP-only SubnetArn and a NO_ENCAP peer with a GRE-only CoreNetworkAddress

_Filed: 2026-10-29T12:19:06Z_
_Operation: CreateConnectPeer | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6cdf7a4bb780a1593da31764ba2b1e15a7af6027`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConnectPeer` reads the tunnel protocol of the Connect attachment the peer is carried over (it needs it, and stores it on the record as `ConnectPeerConfiguration$Protocol`), but it never compares that protocol against the two request members the model declares as protocol-specific:

* `CoreNetworkAddress` — "This only applies only when the protocol is GRE."
* `SubnetArn` — "This only applies only when the protocol is NO_ENCAP."

Both are stored verbatim whatever the attachment's protocol is, and every read of the peer then reports them back. The result is a stored Connect peer whose configuration contradicts its own `Protocol`:

* a peer on a `GRE` Connect attachment created with `SubnetArn` is accepted (200) and `GetConnectPeer`/`ListConnectPeers` report `SubnetArn` on a peer whose protocol the model says that member does not apply to;
* a peer on a `NO_ENCAP` Connect attachment created with `CoreNetworkAddress` is accepted (200) and its `Configuration.CoreNetworkAddress` — plus the copy inside `Configuration.BgpConfigurations[0].CoreNetworkAddress` — report a GRE-only tunnel address.

This is not a gap the crate never attempts: the handler already has the protocol in hand at the point it validates the subnet (it only checks the subnet's Region), and `state.rs` quotes BOTH model sentences in the doc comments of the very fields it stores — so the code under those comments breaks the rule they state.

Scope note for a fixer: `BgpOptions` carries the same "only applies only when the protocol is GRE" sentence, but the existing suite deliberately creates NO_ENCAP peers WITH `BgpOptions.PeerAsn` (tests/all/connect_peer_lifecycle.rs:266-275), so that member is outside this finding. No test pairs a GRE peer with a `SubnetArn`, nor a NO_ENCAP peer with a `CoreNetworkAddress`.

## Evidence

The model, `smithy_query(query="shape", shape="CreateConnectPeerRequest")`:
```
"CoreNetworkAddress": { ... "doc": "<p>A Connect peer core network address. This only applies only when the protocol is <code>GRE</code>.</p>" }
"SubnetArn": { ... "doc": "<p>The subnet ARN for the Connect peer. This only applies only when the protocol is NO_ENCAP.</p>" }
```
The crate quotes both sentences on the stored fields — src/state.rs:3119-3123:
```
    /// The addresses the tunnel's two ends carry: the core network side, which only a GRE
    /// peering needs ("This only applies only when the protocol is GRE", `smithy_query
    /// query="doc" shape="CreateConnectPeerRequest" member="CoreNetworkAddress"`), and the
    /// appliance side, which every peering requires.
    pub core_network_address: Option<String>,
```
and src/state.rs:3136-3140:
```
    /// The subnet the peer reaches its appliance through, which only a `NO_ENCAP` peering
    /// needs (`smithy_query query="doc" shape="CreateConnectPeerRequest"
    /// member="SubnetArn"`), or `None` where the request named none — the modelled pattern
    /// admits the empty string, which names no subnet and is held as none.
    pub subnet_arn: Option<String>,
```
The handler HAS the protocol — src/ops/create_connect_peer.rs:205-219:
```
            // The protocol is the attachment's, not the request's: `CreateConnectPeer`
            // carries no protocol member at all, and the peering runs inside that
            // attachment's tunnel.
            let protocol = match &attachment.detail {
                crate::state::AttachmentDetail::Connect { options, .. } => options.protocol.clone(),
```
but the only check it puts on `SubnetArn` is the Region — src/ops/create_connect_peer.rs:232-238:
```
        // The modelled `SubnetArn` pattern admits the empty string, which names no subnet; a
        // subnet that IS named has to sit in the Region the attachment's edge is in, since
        // that is where this peer reaches its appliance — the same rule a VPC attachment's
        // subnets follow.
        let subnet_arn = input.subnet_arn.clone().filter(|arn| !arn.is_empty());
        if let Some(arn) = subnet_arn.as_deref() {
            if crate::state::arn_region(arn) != Some(edge_location.as_str()) {
```
and both members go onto the record unconditionally — src/ops/create_connect_peer.rs:331-337:
```
            protocol,
            core_network_address: input.core_network_address,
            peer_address: input.peer_address,
            inside_cidr_blocks,
            core_network_asn,
            peer_asn,
            subnet_arn,
```
and are reported unconditionally — src/state.rs:3173-3186 (`configuration()`: `core_network_address: self.core_network_address.clone()`, and again inside `bgp_configurations`) and src/state.rs:3207 (`subnet_arn: self.subnet_arn.clone()`).

Test coverage: tests/all/connect_peer_lifecycle.rs:246-285 only ever pairs `subnet: true` with the NO_ENCAP rows (`transport: 2`) and leaves `core_network_address: None` on every NO_ENCAP row, so no existing assertion pins the contradictory combinations.

## Steps to Reproduce

1. CreateGlobalNetwork, then CreateCoreNetwork with a policy document configuring an edge in us-west-2
2. CreateVpcAttachment for a us-west-2 VPC (the transport), and CreateConnectAttachment over it with `"Options": { "Protocol": "GRE" }`
3. POST /connect-peers with `{ "ConnectAttachmentId": <the GRE Connect attachment>, "PeerAddress": "198.51.100.7", "SubnetArn": "arn:aws:ec2:us-west-2:111122223333:subnet/subnet-0123456789abcdef0" }` → 200, and GET /connect-peers/{id} reports `SubnetArn` although `Configuration.Protocol` is `GRE`, which the model says `SubnetArn` "only applies only when the protocol is NO_ENCAP"
4. Repeat with a second Connect attachment carrying `"Options": { "Protocol": "NO_ENCAP" }` and POST /connect-peers with `{ ..., "PeerAddress": "192.0.2.20", "CoreNetworkAddress": "192.0.2.21" }` → 200, and GET /connect-peers/{id} reports `Configuration.CoreNetworkAddress` (and `Configuration.BgpConfigurations[0].CoreNetworkAddress`) although `Configuration.Protocol` is `NO_ENCAP`, which the model says `CoreNetworkAddress` "only applies only when the protocol is GRE"
5. Expected from the model sentences the crate itself quotes at src/state.rs:3119-3123 and src/state.rs:3136-3140: the protocol the handler already read decides whether those members apply; actual: neither is ever compared against it
