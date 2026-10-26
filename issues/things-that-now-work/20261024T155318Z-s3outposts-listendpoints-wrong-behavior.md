Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Endpoints and Outposts are not Region-scoped: a resource created in one Region is listed in every Region, with its ARN rewritten to the caller's Region

_Filed: 2026-10-24T15:53:18Z_
_Operation: ListEndpoints | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: S3 on Outposts resources are Region-scoped. An Outpost has a home Region and the endpoints on it live in that Region, so a client that calls CreateEndpoint against us-east-1 and then ListEndpoints against eu-west-1 gets an empty `Endpoints` list, and the endpoint's `EndpointArn` always names the endpoint's own (home) Region no matter which Region's endpoint the client talks to. An ARN is a stable identifier of a resource; it does not change value depending on who reads it.

Actual: the service keeps ONE global store (`endpoints` / `outposts`) with no Region on the record, and every ARN is projected from the Region of the current request's SigV4 credential scope (`request_region(ctx.parts())`). Two observable divergences follow:
1. Cross-Region leakage: an endpoint created while signing for us-east-1 is returned by ListEndpoints (and ListSharedEndpoints / ListOutpostsWithS3) signed for eu-west-1, ap-southeast-2, etc. The real service returns nothing there.
2. Unstable ARNs: the same stored endpoint is reported as `arn:aws:s3-outposts:us-east-1:<acct>:outpost/<op>/endpoint/<id>` to one caller and `arn:aws:s3-outposts:eu-west-1:<acct>:outpost/<op>/endpoint/<id>` to another. A client that stores the ARN (to build an S3 on Outposts access point ARN, an IAM policy, or to compare against a recorded value) gets an identifier that names a Region the resource is not in.

The same projection affects `Outpost$OutpostArn` / `Outpost$S3OutpostArn` from ListOutpostsWithS3 and `Endpoint$EndpointArn` from ListSharedEndpoints, and `CreateEndpointResult$EndpointArn`, because all of them run through the same helpers. (This is distinct from the already-filed partition defect, which is about the `arn:aws:` literal rather than Region scoping of state.)

## Evidence

Spec/semantics: `EndpointArn`, `OutpostArn` and `S3OutpostArn` all embed a Region segment (smithy_query(query="shape", shape="Endpoint,Outpost")); well-established AWS semantics make an Outpost (and the endpoints on it) a resource of one home Region, and make an ARN an immutable identifier of the resource rather than a function of the reader's Region.
State/handler: services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:30-37 states the design outright ("The store holds one set of resources rather than one per Region", ARNs "projected and not stored"); `EndpointRecord` (src/state.rs:194-218) and `OutpostRecord` (src/state.rs:138-145) carry no Region member; `request_region` (src/state.rs:459-463) reads the Region from the request and is interpolated by `endpoint_arn`/`outpost_arn`/`s3_outpost_arn` (src/state.rs:164-178, :223-228). ListEndpoints uses it at src/ops/list_endpoints.rs:175 and filters only on `endpoint.owner_account != caller` (:186) — never on Region.
Tests: tests/all/endpoint_lifecycle.rs::every_arn_names_the_region_the_request_was_signed_for builds a FRESH emulator per Region (line 562) and creates+lists inside the same Region, so no case creates in one Region and reads in another; the cross-Region behaviour is untested, and no test asserts a stable home-Region ARN.

## Steps to Reproduce

1. Start the emulator with an Outpost provisioned (S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248")
2. Sign a CreateEndpoint request for us-east-1: OutpostId="op-0a1b2c3d4e5f60718", SubnetId="subnet-0a1b2c3d4e5f60718", SecurityGroupId="sg-0a1b2c3d4e5f60718" -> 200 with EndpointArn containing :us-east-1:
3. Call ListEndpoints signed for eu-west-1 (SigV4 credential scope .../eu-west-1/s3-outposts/aws4_request) with maxResults=100
4. Expected: an empty Endpoints list (the endpoint lives in us-east-1)
5. Observed: the endpoint is returned, and its EndpointArn now reads arn:aws:s3-outposts:eu-west-1:<account>:outpost/op-0a1b2c3d4e5f60718/endpoint/<id> — a different ARN for the same stored record
6. Call ListOutpostsWithS3 signed for eu-west-1: the same Outpost is reported with OutpostArn/S3OutpostArn naming eu-west-1
