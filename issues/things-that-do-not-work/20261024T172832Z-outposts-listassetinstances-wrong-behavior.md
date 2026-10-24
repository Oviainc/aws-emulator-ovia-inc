Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: d905081120311aa2c64faab09af576215e2cd502

# No instance is ever recorded on an Outpost, so ListAssetInstances answers an empty list for every Outpost

_Filed: 2026-10-24T17:28:32Z_
_Operation: ListAssetInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d905081120311aa2c64faab09af576215e2cd502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAssetInstances` reports `OutpostRecord::instances`, which no request ever writes: `CreateOutpost` initialises it to `Vec::new()` and no handler adds to it. Over the wire the operation is therefore a canned answer — HTTP 200 with `{"AssetInstances": []}` and no `NextToken` for every Outpost, regardless of the Outpost, its hardware type or anything that happened to it.

Expected (real Outposts): the list reports one `AssetInstance` per Amazon EC2 instance running on the Outpost across all accounts — `InstanceId`, `InstanceType`, `AssetId`, `AccountId` and, for service-owned capacity, `AwsServiceName`.

Actual: the handler's four filters (`AssetIdFilter`, `InstanceTypeFilter`, `AccountIdFilter`, `AwsServiceFilter`) and its paging path are unreachable through the API, and two further answers are hollow for the same reason: `ListBlockingInstancesForCapacityTask` always reports `{"BlockingInstances": []}` (so a capacity task is never held up by a running instance and `TaskActionOnBlockingInstances` / `InstancesToExclude` can never change an answer), and `StartOutpostDecommission` can never report the `EC2_INSTANCE` blocking resource type that AWS requires a caller to clear before an Outpost is decommissioned.

The fix is to give the Outpost real instance state that a caller can create — the natural source is the EC2 emulator in this repo, whose `RunInstances` is implemented (`services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs`), so an instance launched against an Outpost should become visible here — or, failing cross-service wiring, state this service can derive from the capacity a completed capacity task placed.

## Evidence

Spec: `com.amazonaws.outposts#ListAssetInstances` is documented "A list of Amazon EC2 instances, belonging to all accounts, running on the specified Outpost. Does not include Amazon EBS or Amazon S3 instances" and models `AssetInstances` plus four filters and `MaxResults`/`NextToken`. `ListBlockingInstancesForCapacityTask` is documented "A list of all running Amazon EC2 instances on the Outpost. Stopping one or more of these instances can free up the capacity needed to run the capacity task", and `BlockingResourceType` declares `EC2_INSTANCE` — all three depend on instance state this service never holds.

Handler/state: `src/ops/list_asset_instances.rs` lines ~198-234 pages `record.instances` only; `src/state.rs` line ~371 declares `pub instances: Vec<AssetInstance>` and `src/ops/create_outpost.rs` line ~299 (`instances: Vec::new()`) is the only initialisation. `src/state.rs` line ~1059-1065 (`blocking_resource_types`) gates `BLOCKING_EC2_INSTANCE` on `!record.instances.is_empty()`, so it can never fire. `grep -n 'instances' …/src` shows no writer.

Tests: `tests/all/asset_instances.rs` and `tests/all/blocking_instances.rs` seed instances by poking the in-memory store rather than through any request, confirming no HTTP path produces one; `tests/all/outpost_decommission.rs::an_outstanding_order_blocks_the_decommission_and_nothing_else_does` exercises only the quote-based blocking reason.

## Steps to Reproduce

1. CreateSite, then CreateOutpost against it; keep the returned OutpostId.
2. GET /outposts/{OutpostId}/assetInstances — observe HTTP 200 with {"AssetInstances": []}, with or without AssetIdFilter / InstanceTypeFilter / AccountIdFilter / AwsServiceFilter and any MaxResults.
3. POST /outposts/{OutpostId}/capacity with {"InstancePools":[{"InstanceType":"m5.4xlarge","Count":2}],"TaskActionOnBlockingInstances":"WAIT_FOR_EVACUATION"}, then GET /outposts/{OutpostId}/capacity/{CapacityTaskId}/blockingInstances — always {"BlockingInstances": []}, so the task completes on the clock with nothing ever blocking it.
4. POST /outposts/{OutpostId}/decommission with {"ValidateOnly":true} — answers SKIPPED and never reports BlockingResourceTypes: ["EC2_INSTANCE"], even though AWS requires running instances to be cleared first.
