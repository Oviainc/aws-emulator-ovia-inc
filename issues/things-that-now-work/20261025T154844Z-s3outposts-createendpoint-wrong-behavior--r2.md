Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9c887f34f86da2c6771f7b9b12ec1c8e6bf552af

# CreateEndpoint's "one endpoint per (Outpost, subnet, security group)" conflict check is silently scoped to the caller's own endpoints

_Filed: 2026-10-25T15:48:44Z_
_Operation: CreateEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9c887f34f86da2c6771f7b9b12ec1c8e6bf552af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler states its own uniqueness rule in a comment — "One endpoint per (Outpost, subnet, security group)" — and then implements a narrower rule: the duplicate search additionally requires `endpoint.owner_account == caller`, so an endpoint record owned by another account is invisible to the check. A create that repeats all three members of such a record is accepted with 200 and a second endpoint is stored committing the same subnet and security group to the same Outpost, which is exactly the situation the comment above the check calls "the conflict this operation models". The crate's own test states the same rule in the same words ("One endpoint per (Outpost, subnet, security group)") but only exercises records this account owns, so the owner-scoped branch is untested and the code and its stated rule disagree. The state module explicitly supports the state that reaches this path: an Outpost shared by another account carrying that account's own endpoint (the crate's test fixture creates exactly that).

## Evidence

Handler comment vs. code — services/service-com-amazonaws-s3outposts-s3outposts/src/ops/create_endpoint.rs:206-214:
```
        // One endpoint per (Outpost, subnet, security group): a repeat of the
        // same request would commit a second range of the same subnet to the
        // same Outpost, which is the conflict this operation models.
        if let Some(existing) = store.endpoints.values().find(|endpoint| {
            endpoint.owner_account == caller
                && endpoint.outpost_id == input.outpost_id
                && endpoint.subnet_id == input.subnet_id
                && endpoint.security_group_id == input.security_group_id
        }) {
```
The same rule, stated by this crate's test without the owner qualifier — tests/all/endpoint_lifecycle.rs:1014-1015:
```
/// One endpoint per (Outpost, subnet, security group): a create that repeats all three is
/// the modelled conflict, and a create that differs in any one of them is accepted.
```
and its cases only vary OutpostId/SubnetId/SecurityGroupId for endpoints this account owns (tests/all/endpoint_lifecycle.rs:1031-1060), so nothing covers a record another account owns.

The state that reaches the skipped branch is one the crate stands up itself — tests/all/endpoint_lifecycle.rs:104-120: `store.endpoints.insert((SHARED_OUTPOST_ID.to_string(), endpoint_id.clone()), EndpointRecord { ... subnet_id /* "subnet-0a1b2c3d4e5f60718" */, security_group_id: "sg-0a1b2c3d4e5f60718".to_string(), ... owner_account: SHARING_ACCOUNT_ID.to_string() })` — and src/state.rs:91-94 describes it as a supported scenario: "An entry whose owner is another account is one shared with the caller through Resource Access Manager ... and what a caller can create endpoints on".

## Steps to Reproduce

1. Stand up the emulator over a store that holds an Outpost shared by another account plus that account's endpoint on it — exactly the crate's own fixture, services/service-com-amazonaws-s3outposts-s3outposts/tests/all/endpoint_lifecycle.rs::emulator_with_engine (Outpost op-2c3d4e5f607182930, endpoint on subnet-0a1b2c3d4e5f60718 / sg-0a1b2c3d4e5f60718, owner 000000000001)
2. POST /S3Outposts/CreateEndpoint with {"OutpostId":"op-2c3d4e5f607182930","SubnetId":"subnet-0a1b2c3d4e5f60718","SecurityGroupId":"sg-0a1b2c3d4e5f60718"}
3. Expected per the handler's own stated rule ("One endpoint per (Outpost, subnet, security group)"): 409 ConflictException, nothing stored
4. Observed: 200 with a new EndpointArn; GET /S3Outposts/ListSharedEndpoints?outpostId=op-2c3d4e5f607182930&maxResults=100 now reports two endpoints with the same SubnetId and SecurityGroupId on one Outpost
