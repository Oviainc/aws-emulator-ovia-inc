Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 57d818e145087aac91ee9b85e99c9525e7c9e7c0

# CreateCluster accepts device types that cannot form a cluster (Snowcone / original Snowball) instead of refusing the combination

_Filed: 2026-10-24T12:17:10Z_
_Operation: CreateCluster | Class: validation_

> _Note: this issue was identified against the code base at commit `57d818e145087aac91ee9b85e99c9525e7c9e7c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a CreateCluster request whose `SnowballType` names a device type that does not support cluster jobs is refused with the modeled `InvalidInputCombinationException` (HTTP 400). The model's own note on `CreateClusterRequest$SnowballType` states: "For cluster jobs, Amazon Web Services Snow Family currently supports only the EDGE device type", and `InvalidInputCombinationException` is documented as exactly this failure: "Job or cluster creation failed. One or more inputs were invalid. Confirm that the CreateClusterRequest$SnowballType value supports your CreateJobRequest$JobType, and try again." In particular the Snowcone types (`SNC1_HDD`, `SNC1_SSD`) and the original Snowball (`STANDARD`) are not cluster-capable devices.

Actual: the handler never looks at `SnowballType` at all. It validates only that `AddressId`/`ForwardingAddressId` name stored addresses and that the emulator's own store ceilings are not exceeded, then persists whatever device type was supplied and answers 200 with a minted `ClusterId`. `DescribeCluster` then reports e.g. a `SNC1_SSD` cluster in `AwaitingQuorum`, and `CreateJob` happily attaches node jobs to it (the node inherits the uncluster-able device type), so a whole cluster workflow succeeds for a device family that cannot be clustered.

As a secondary symptom of the same missing check, no `JobType` / `SnowballType` compatibility check happens on this operation either: any pairing of the two modeled enums (e.g. `JobType=EXPORT` with `SnowballType=SNC1_HDD`) is accepted, although the declared combination error is documented in terms of precisely that pairing.

## Evidence

Spec: `smithy_query(query="doc", shape="CreateClusterRequest", member="SnowballType")` — "For cluster jobs, Amazon Web Services Snow Family currently supports only the EDGE device type." `CreateCluster` declares `InvalidInputCombinationException` (HTTP 400) whose documentation names `CreateClusterRequest$SnowballType` (OPERATIONS.md 'CreateCluster'). Well-established Snow Family behavior: clusters are a Snowball Edge feature; Snowcone (SNC1_*) and the original Snowball (STANDARD) cannot be ordered as a cluster.

Handler: services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/create_cluster.rs, `handle` (lines 219-358) — the only validation is the address lookup loop (lines 235-254) and the store-ceiling checks (lines 283-294); `input.snowball_type` is moved straight into `StoredCluster.snowball_type` (line 309) and `input.job_type` into `job_type` (line 308) with no check. `grep -n 'EDGE' src/ops/create_cluster.rs` matches only the generated doc comment, never code.

Tests: the opposite is asserted today — tests/all/cluster_cancellation.rs `cancellable_rows` (lines 125-136) builds one cluster per entry of `SNOWBALL_TYPE_VALUES` (including `SNC1_SSD`, `SNC1_HDD`, `STANDARD`) and the test expects every one of those creates to succeed; tests/all/cluster_updates.rs line 777 uses a `SNC1_SSD` value only as a mismatching node device type. No test asserts a refusal for a non-cluster-capable device type.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSIESnowballJobManagementService.CreateAddress` and a deliverable address; keep the minted AddressId
2. POST / with `X-Amz-Target: ...CreateCluster` and body {"JobType": "IMPORT", "SnowballType": "SNC1_SSD", "ShippingOption": "SECOND_DAY", "AddressId": "<id>"} — observe HTTP 200 with a minted ClusterId instead of a 400 `InvalidInputCombinationException`
3. DescribeCluster on that ClusterId — observe `ClusterMetadata.SnowballType = "SNC1_SSD"` in `AwaitingQuorum`
4. Repeat with "SnowballType": "STANDARD" and with {"JobType": "EXPORT", "SnowballType": "SNC1_HDD"} — both are accepted as well
5. POST / ...CreateJob with {"ClusterId": "<the SNC1_SSD cluster>"} — observe the node job is created and DescribeJob reports a SNC1_SSD cluster node
