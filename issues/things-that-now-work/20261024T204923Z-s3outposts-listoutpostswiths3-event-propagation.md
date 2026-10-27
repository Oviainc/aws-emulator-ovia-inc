Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: 1302d977b6b888f4927f8769f8d72a08ded5722c

# S3 on Outposts inventory is not wired to the AWS Outposts emulator: an Outpost created through the outposts API is invisible, so no client can ever reach a usable S3 on Outposts state

_Filed: 2026-10-24T20:49:23Z_
_Operation: ListOutpostsWithS3 | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `1302d977b6b888f4927f8769f8d72a08ded5722c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `ListOutpostsWithS3` "Lists the Outposts with S3 on Outposts capacity for your Amazon Web Services account. Includes S3 on Outposts that you have access to as the Outposts owner, or as a shared user from Resource Access Manager (RAM)." The Outposts it reports are the SAME resources the AWS Outposts control plane owns — the emulator co-hosts that service (`service-com-amazonaws-outposts-outpostsolafservice`, with a fully stateful `CreateSite`/`CreateOutpost`/`CreateOrder`/capacity-task surface). So a client that creates a site and an Outpost through the outposts API and provisions S3 storage capacity on it (an order/capacity task naming S3 storage) must then see that Outpost from `ListOutpostsWithS3` (with `OutpostId`, `OwnerId`, `OutpostArn` = `arn:aws:outposts:<region>:<acct>:outpost/<id>`, `S3OutpostArn`, `CapacityInBytes`), and `CreateEndpoint(OutpostId=<that id>)` must then succeed.

Actual: the two emulated services share nothing. The s3outposts store holds its own `outposts: BTreeMap<String, OutpostRecord>` whose ONLY writer is `provision_outpost`, reached exclusively from the `S3_OUTPOSTS_PROVISIONED_OUTPOSTS` environment variable at store construction (src/state.rs:12-27, :92, :331-350, :361-363). Nothing any AWS client can call ever adds an Outpost. Observable consequences for a client driving the emulator only through AWS APIs:

1. `outposts:CreateOutpost` -> 200 with `op-...`; `s3-outposts:ListOutpostsWithS3` -> `{"Outposts": []}` forever (and no `NextToken`).
2. `s3-outposts:CreateEndpoint(OutpostId=op-...)` -> 404 `ResourceNotFoundException` "The specified outpost does not exist, or does not have S3 capacity provisioned on it.", because the handler resolves the id against that empty map (src/ops/create_endpoint.rs:199-204). `DeleteEndpoint` and `ListSharedEndpoints` 404 for the same reason.
3. The whole endpoint lifecycle of this service is therefore unreachable through the API surface: the only door into it is an operator-set environment variable, which an SDK/CLI client, a CloudFormation deployment or a trace replay cannot set.

Note the correct fix is a GATED projection, not a seeded one: only an Outpost the outposts emulator holds AND that has S3 capacity provisioned on it (an order/capacity task whose catalog item declares S3 storage) belongs in the inventory, with `CapacityInBytes` computed from that capacity — re-inventing capacity for every Outpost would reintroduce the already-fixed "S3 Outposts seeds phantom Outposts reported by ListOutpostsWithS3" defect. Outposts owned by another account and shared via RAM keep being reported with the owner's `OwnerId`.

## Evidence

Spec: smithy_query(query="doc", shape="ListOutpostsWithS3") — "Lists the Outposts with S3 on Outposts capacity for your Amazon Web Services account. Includes S3 on Outposts that you have access to as the Outposts owner, or as a shared user from Resource Access Manager (RAM)."; `Outpost$OutpostArn` is in the AWS Outposts ARN namespace (`arn:aws:outposts:<region>:<acct>:outpost/<id>`) and `Outpost$CapacityInBytes` is "the Amazon S3 capacity of the outpost in bytes" (smithy_query(query="shape", shape="Outpost")) — i.e. this operation reports the other service's resource, so the two emulators must agree on which Outposts exist.
Handler/state: services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:12-27 states the design outright ("The store therefore starts EMPTY… The one thing outside the API that can say an Outpost was provisioned is the operator running this emulator, through PROVISIONED_OUTPOSTS_ENV"), :92 (`S3_OUTPOSTS_PROVISIONED_OUTPOSTS`), :331-350 (`Default` reads only that env var), :361-363 (`provision_outpost` is the only writer); src/ops/list_outposts_with_s3.rs:176-193 pages over that map alone; src/ops/create_endpoint.rs:199-204 answers 404 for any id not in it.
Sibling service that DOES own the resource: services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/create_outpost.rs:275-277 inserts a persisted `OutpostRecord`; that crate's state carries orders and capacity tasks (src/state.rs:641 `OrderRecord`, :522 `CapacityTaskRecord`, :1096 service state) and its catalog items carry `supported_storage` with an `S3` value (src/ops/types/shared_types.rs:330, :514 `SUPPORTED_STORAGE_ENUM_VALUES = ["EBS", "S3"]`) — the S3-capacity fact the gate needs.
Tests: every s3outposts test stands Outposts up in-process through the store seam (tests/all/endpoint_lifecycle.rs:71-91 `store.provision_outpost(…)`), never through any API call; no test drives outposts:CreateOutpost and then s3-outposts:ListOutpostsWithS3, so the cross-service path is entirely uncovered.

## Steps to Reproduce

1. Start the emulator with S3_OUTPOSTS_PROVISIONED_OUTPOSTS unset (the default) and both the `outposts` and `s3-outposts` services mounted
2. Call outposts:CreateSite, then outposts:CreateOutpost(SiteId=<site>, AvailabilityZone=<az>) -> 200 with OutpostId=op-xxxxxxxxxxxxxxxxx
3. Provision S3 capacity on it through the outposts API (outposts:CreateOrder against a catalog item whose SupportedStorage includes S3, and/or a capacity task that completes)
4. Call s3-outposts:ListOutpostsWithS3?maxResults=100
5. Expected: the Outpost just created is reported, with OutpostId=op-xxxxxxxxxxxxxxxxx, OwnerId=<caller account>, OutpostArn=arn:aws:outposts:<region>:<acct>:outpost/op-xxxxxxxxxxxxxxxxx, S3OutpostArn=arn:aws:s3-outposts:<region>:<acct>:outpost/op-xxxxxxxxxxxxxxxxx/s3 and a non-zero CapacityInBytes
6. Observed: 200 with {"Outposts": []}
7. Call s3-outposts:CreateEndpoint(OutpostId=op-xxxxxxxxxxxxxxxxx, SubnetId=subnet-0a1b2c3d4e5f60718, SecurityGroupId=sg-0a1b2c3d4e5f60718)
8. Expected: 200 with an EndpointArn; Observed: 404 ResourceNotFoundException "The specified outpost does not exist, or does not have S3 capacity provisioned on it." — and no sequence of AWS API calls can change that
