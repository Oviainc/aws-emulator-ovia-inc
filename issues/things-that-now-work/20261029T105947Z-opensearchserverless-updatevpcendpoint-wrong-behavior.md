Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2d45ab8ecba5c3bbeb0b6029f45083afacab7d74

# Scope UpdateVpcEndpoint's clientToken replay to the endpoint the request addresses

_Filed: 2026-10-29T10:59:47Z_
_Operation: UpdateVpcEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2d45ab8ecba5c3bbeb0b6029f45083afacab7d74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateVpcEndpoint` looks a replayed `clientToken` up in the shared idempotency index and answers with whatever VPC endpoint that token FIRST named, without checking that the endpoint is the one `id` addresses.

The state helper it calls documents the opposite rule and says every caller in `src/ops/` honours it, and every other `@idempotent` handler of this crate does filter on the addressed identity (the create filters on `name`, `UpdateCollectionGroup`/`UpdateSecurityConfig` on `id`, the policy updates on the `(type, name)` pair).

Consequence: a caller that spends one `clientToken` on `UpdateVpcEndpoint` for endpoint A and then (deliberately or by reusing a token) for endpoint B gets HTTP 200 carrying A's `UpdateVpcEndpointDetail` — A's `id`, `name`, `status`, `subnetIds` and `securityGroupIds` — while the subnet/security-group change it asked for on B is silently never applied. The write is dropped and the response describes a resource the request did not address. The handler's own comment above the code says the replay "answers the record that call left" — the record of THIS call, not of a call against another endpoint.

## Evidence

The helper states the rule the handler must follow: "The handler SCOPES what this answers to the resource its request addresses: a token spent again on another resource is a request for that other resource, which is still served on its own terms rather than answered with the record the token first created. Every caller in `src/ops/` therefore filters on the addressed identity." — src/state.rs:2725-2728, directly above `pub fn replayed_collection(...)` (src/state.rs:2729); `replayed_vpc_endpoint` is defined under "The VPC endpoint a replayed `clientToken` on `operation` already created, if any; see [`Self::replayed_collection`] for why a stale token answers `None`." — src/state.rs:2964-2975.

The handler does NOT filter:
"        // A replayed `clientToken` is a retry of the call that already applied this update,\n        // so it answers the record that call left rather than applying the change twice.\n        if let Some(token) = input.client_token.as_deref() {\n            if let Some(record) = store.replayed_vpc_endpoint(Self::NAME, token) {\n                return Ok(UpdateVpcEndpointOutput {\n                    update_vpc_endpoint_detail: Some(record.update_detail()),\n                });\n            }\n        }" — src/ops/update_vpc_endpoint.rs:244-252.

Every sibling filters on the addressed identity:
- `.replayed_vpc_endpoint(Self::NAME, token).filter(|record| record.name == input.name)` under "The replay is scoped to the name THIS request carries" — src/ops/create_vpc_endpoint.rs:185-194.
- `.replayed_collection_group(Self::NAME, token).filter(|record| record.id == input.id)` — src/ops/update_collection_group.rs:180-184.
- `.replayed_security_config(Self::NAME, token).filter(|record| record.id == input.id)` under "The replay is scoped to the id THIS request addresses." — src/ops/update_security_config.rs:214-219.
- `.replayed_security_policy(Self::NAME, token).filter(|record| record.kind == key.0 && record.name == key.1)` — src/ops/update_security_policy.rs:216-220.

Test coverage: nothing pins the cross-endpoint replay. `tests/all/vpc_endpoint_update.rs:711-791` (`a_replayed_client_token_answers_the_record_its_first_call_left`) replays the token only against the SAME `id` (both retry cases carry `"id": id`, lines 743-752), so scoping the replay to `input.id` leaves that test unchanged.

## Steps to Reproduce

1. CreateVpcEndpoint(name="endpoint-a", vpcId="vpc-1a2b3c4d", subnetIds=["subnet-11111111"]) and CreateVpcEndpoint(name="endpoint-b", vpcId="vpc-1a2b3c4d", subnetIds=["subnet-33333333"]); wait for both to settle to ACTIVE; note ids A and B.
2. UpdateVpcEndpoint(id=A, addSubnetIds=["subnet-22222222"], clientToken="T") -> 200, detail names A with subnetIds [subnet-11111111, subnet-22222222].
3. UpdateVpcEndpoint(id=B, addSubnetIds=["subnet-44444444"], clientToken="T") -> 200, but `UpdateVpcEndpointDetail.id`/`name`/`subnetIds` are A's, not B's.
4. BatchGetVpcEndpoint(ids=[B]) -> B still has only subnet-33333333: the update was accepted with 200 and silently dropped.
