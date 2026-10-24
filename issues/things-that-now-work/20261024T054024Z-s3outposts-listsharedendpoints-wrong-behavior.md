Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# ListSharedEndpoints hides the caller's own endpoints on a shared Outpost and reports other accounts' endpoints instead

_Filed: 2026-10-24T05:40:24Z_
_Operation: ListSharedEndpoints | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: ListSharedEndpoints "Lists all endpoints associated with an Outpost that has been shared by Amazon Web Services Resource Access Manager (RAM)" — the listing is scoped by the Outpost named in `OutpostId`, which is the one required input member, and it is how a consumer account enumerates the endpoints it has on an Outpost shared with it (CreateEndpoint and DeleteEndpoint are the documented related actions, both of which operate on the caller's own endpoints). The endpoints a caller created on a RAM-shared Outpost must therefore appear here.

Actual: the handler applies an owner filter the spec never states — `if endpoint.outpost_id != input.outpost_id || endpoint.owner_account == caller { continue; }` — so the result set is exactly the complement of what the caller can act on: every endpoint the caller itself created on the shared Outpost is skipped, and only records owned by OTHER accounts are returned, complete with those accounts' `VpcId`, `SubnetId`, `SecurityGroupId` and `CidrBlock` (details the real service does not hand a consumer account). A caller that does CreateEndpoint on a shared Outpost and then ListSharedEndpoints(OutpostId=<that Outpost>) gets an empty `Endpoints` list, and is left with no Outpost-scoped way to enumerate what it created.

## Evidence

Spec: smithy_query(query="doc", shape="ListSharedEndpoints") — "Lists all endpoints associated with an Outpost that has been shared by Amazon Web Services Resource Access Manager (RAM)", related actions CreateEndpoint/DeleteEndpoint; the only required scoping member is `OutpostId` (OPERATIONS.md, ListSharedEndpoints input), and `Endpoint` carries VPC/subnet/security-group members that belong to the endpoint owner's account (smithy_query(query="shape", shape="Endpoint")).
Handler: services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_shared_endpoints.rs:193-199 — the `endpoint.owner_account == caller` skip.
Tests: tests/all/endpoint_lifecycle.rs::list_shared_endpoints_reports_only_what_another_account_owns asserts the current inverted filter (the caller's own endpoints "never appear"), so the behaviour is enshrined rather than covered as a gap.

## Steps to Reproduce

1. Start the emulator with an Outpost owned by another account (shared with the caller), e.g. S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-2c3d4e5f607182930:26388279066624:000000000001"
2. CreateEndpoint(OutpostId="op-2c3d4e5f607182930", SubnetId="subnet-0a1b2c3d4e5f60718", SecurityGroupId="sg-0a1b2c3d4e5f60718") → 200 with an EndpointArn
3. ListSharedEndpoints(OutpostId="op-2c3d4e5f607182930", MaxResults=100)
4. Expected: the endpoint just created on the shared Outpost is listed
5. Observed: Endpoints is empty — the handler skips every endpoint whose owner is the calling account, and reports only records owned by other accounts
