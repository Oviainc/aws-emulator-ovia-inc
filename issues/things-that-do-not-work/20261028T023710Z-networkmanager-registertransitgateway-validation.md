Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# RegisterTransitGateway accepts a transit gateway owned by another account, which its own documentation forbids

_Filed: 2026-10-28T02:37:10Z_
_Operation: RegisterTransitGateway | Class: validation_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's documentation states two constraints on the gateway's ARN: the Region may be any supported one, but the gateway "must be owned by the same Amazon Web Services account that owns the global network". The handler validates only that the ARN names a transit gateway and carries a Region (`ec2_resource_region`); it never compares the ARN's account segment with the account that owns the global network (`::engine::emulator_account_id()`, the only account this emulator presents), so a registration for a gateway of an arbitrary foreign account is stored and reported as a legitimate registration.

The crate already has the piece needed and uses it for the neighbouring rule: `AssociateTransitGatewayConnectPeer` reads `arn_account_id` from the peer ARN and matches it against the registrations' accounts, so a foreign-account registration also silently widens which Connect peers that global network will accept.

## Evidence

Smithy (smithy_query query="doc" shape="RegisterTransitGateway"): "Registers a transit gateway in your global network. ... The transit gateway can be in any of the supported Amazon Web Services Regions, but it must be owned by the same Amazon Web Services account that owns the global network. You cannot register a transit gateway in more than one global network."

The handler enforces the second sentence and not the first — src/ops/register_transit_gateway.rs:159-173 checks only the ARN form/Region:
```
if crate::state::ec2_resource_region(&input.transit_gateway_arn, crate::state::TRANSIT_GATEWAY_TYPE).is_none() {
    ... "must name a transit gateway and the Region it is in" ...
}
```
and src/ops/register_transit_gateway.rs:214-238 implements "You cannot register a transit gateway in more than one global network". No account comparison appears anywhere in the handler.

The helper the ARN account is read with elsewhere — src/ops/associate_transit_gateway_connect_peer.rs:172-173: `let peer_account = crate::state::arn_account_id(&input.transit_gateway_connect_peer_arn)...` fed into `carries_transit_gateway_in(..., peer_account.as_deref())` (src/state.rs:4073-4086, which compares `arn_account_id(&registration.transit_gateway_arn) == account`).

Disclosure: the shared ARN parser deliberately skips the account for the PEERING case (src/state.rs:7958-7960: "The account segment is not judged: a peered transit gateway may belong to another account of the organization."), and tests/all/transit_gateway_registration_lifecycle.rs:144-147 currently tables a row named "a gateway of another account" (account 210987654321) among the rows that must answer 200 — that row states no reason for the allowance and is what the quoted doc sentence contradicts.

## Steps to Reproduce

1. CreateGlobalNetwork (owned by the emulator account)
2. RegisterTransitGateway with TransitGatewayArn = arn:aws:ec2:ap-southeast-2:210987654321:transit-gateway/tgw-0123456789abcdef0 → 200 and a stored registration
3. Expected per the operation doc: a ValidationException naming TransitGatewayArn, because the gateway is not owned by the account that owns the global network
4. Follow-on: AssociateTransitGatewayConnectPeer with a Connect peer ARN of that foreign account now passes the carries_transit_gateway_in gate on the strength of the registration that should not exist
