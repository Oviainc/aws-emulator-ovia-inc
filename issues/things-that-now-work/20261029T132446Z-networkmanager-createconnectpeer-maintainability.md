Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a1e71609e4dd8609a7a875072f755c7dbeedcd71

# CreateConnectPeer hand-rolls the "empty string names no resource" normalisation instead of calling crate::state::optional_reference

_Filed: 2026-10-29T13:24:45Z_
_Operation: CreateConnectPeer | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a1e71609e4dd8609a7a875072f755c7dbeedcd71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate single-homes the rule that an optional reference member whose modelled pattern admits the empty string reads that empty string as "no resource" in `crate::state::optional_reference`, and its own documentation says so: it is "the way every other reader and writer of an optional reference reads it". `CreateConnectPeer` applies the same rule to `SubnetArn` by open-coding `.filter(|arn| !arn.is_empty())` instead of calling that helper — it is the only place in `src/ops/` that spells the rule by hand rather than through the single home. The two copies can drift: a change to what counts as "naming no resource" (whitespace-only, say) reaches every other member and silently misses this one. `optional_reference` is generic over `Id: AsRef<str>` and is already called with an `Option<String>` elsewhere (`associate_connect_peer.rs:214`, `associate_transit_gateway_connect_peer.rs:220`), so `crate::state::optional_reference(input.subnet_arn.clone())` is a drop-in with byte-identical behaviour: no response and no existing test changes.

## Evidence

The hand-rolled copy — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_connect_peer.rs:232-236:
```
        // The modelled `SubnetArn` pattern admits the empty string, which names no subnet; a
        // subnet that IS named has to sit in the Region the attachment's edge is in, since
        // that is where this peer reaches its appliance — the same rule a VPC attachment's
        // subnets follow.
        let subnet_arn = input.subnet_arn.clone().filter(|arn| !arn.is_empty());
```

The single home it skips — services/service-com-amazonaws-networkmanager-networkmanager/src/state.rs:489-493:
```
/// string at all (an id pattern, a minimum length) never reaches a handler empty, because the
/// constraint walk refuses it first.
pub fn optional_reference<Id: AsRef<str>>(supplied: Option<Id>) -> Option<Id> {
    supplied.filter(|id| !id.as_ref().is_empty())
}
```
and the crate's own statement that this helper is THE way the rule is applied — services/service-com-amazonaws-networkmanager-networkmanager/src/ops/get_network_resources.rs:226-233: "`ResourceArn` and `RegisteredGatewayArn` are OPTIONAL reference members whose modelled shape (`ResourceArn`, `length 0..1500`, `pattern ^[\\s\\S]*$`) admits the empty string, so what each names is read the way every other reader and writer of an optional reference reads it ([`crate::state::optional_reference`])". The same sentence appears in get_network_telemetry.rs:217-224, get_network_resource_relationships.rs:244-251, get_connections.rs:198-201 and get_links.rs:212-215, and the call is made in associate_connect_peer.rs:214 (`crate::state::optional_reference(input.link_id.clone())`), associate_transit_gateway_connect_peer.rs:220, associate_customer_gateway.rs:213 and create_connection.rs:248-249.

A grep of `src/ops/` for `!.*is_empty\\(\\)` returns exactly three hits, and create_connect_peer.rs:236 is the only one that is this normalisation (get_links.rs:159 tests a Vec of conflicting member names; list_organization_service_access_status.rs:156 is the NextToken rule its own comment documents separately).

Tests that cover the member and would stay green: tests/all/create_connect_peer_bounds.rs and tests/all/connect_peer_lifecycle.rs drive CreateConnectPeer with an absent SubnetArn, an empty-string SubnetArn and a well-formed one; tests/all/empty_optional_reference.rs pins the "empty string names no resource" reading for the members that already go through the helper.

## Steps to Reproduce

1. grep -n 'optional_reference' services/service-com-amazonaws-networkmanager-networkmanager/src/ops/ — every optional free-form reference member of the crate goes through crate::state::optional_reference
2. Open services/service-com-amazonaws-networkmanager-networkmanager/src/ops/create_connect_peer.rs:236 — the same rule is spelled inline as `.filter(|arn| !arn.is_empty())`
3. Replace it with `crate::state::optional_reference(input.subnet_arn.clone())` and re-run the suite: CreateConnectPeer with SubnetArn absent, SubnetArn="" and a well-formed subnet ARN all answer exactly what they answer today
