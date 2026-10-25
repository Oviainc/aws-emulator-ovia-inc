Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# CreateCluster ignores ForceCreateJobs and never refuses an over- or under-provisioned cluster

_Filed: 2026-10-24T21:29:48Z_
_Operation: CreateCluster | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateClusterRequest$ForceCreateJobs` is documented as "Force to create cluster when user attempts to overprovision or underprovision a cluster. A cluster is overprovisioned or underprovisioned if the initial size of the cluster is more (overprovisioned) or less (underprovisioned) than what [is] needed to meet capacity requirement specified with OnDeviceServiceConfiguration."

Expected: CreateCluster compares the requested `InitialClusterSize` against the node capacity the request itself declares in `OnDeviceServiceConfiguration` — in particular `S3OnDeviceService.ServiceSize` ("Applicable when creating a cluster. Specifies how many nodes are needed for Amazon S3 compatible storage on Snow family devices", range 3..16) together with `StorageLimit`/`FaultTolerance` — and refuses a misprovisioned order (the modelled `InvalidInputCombinationException`, "One or more inputs were invalid") unless the caller passed `ForceCreateJobs: true`, in which case the cluster is created anyway.

Actual: the handler never reads `input.force_create_jobs` at all, and never compares `InitialClusterSize` with the declared capacity. Every request is created as asked: a cluster asking for Amazon S3 compatible storage with `ServiceSize: 16` and `InitialClusterSize: 1` answers 200, mints one node job, and `ForceCreateJobs` is a dead input (it is not stored, not reported, and changes nothing). A client that relies on the service to reject a misprovisioned cluster order — and on `ForceCreateJobs` as the explicit override — gets a silently accepted cluster that can never serve the capacity it was configured for, and cannot distinguish "accepted because it is correctly provisioned" from "accepted because nothing checked".

The handler's own comment justifies the omission with "a requirement this emulator stages no data for and so cannot compute", but the capacity requirement is not staged data: it is request input (`OnDeviceServiceConfiguration.S3OnDeviceService.ServiceSize` is literally a node count), so the comparison with `InitialClusterSize` is computable from the request alone.

## Evidence

Spec: `smithy_query(query="doc", shape="CreateClusterRequest", member="ForceCreateJobs")` → "Force to create cluster when user attempts to overprovision or underprovision a cluster. A cluster is overprovisioned or underprovisioned if the initial size of the cluster is more (overprovisioned) or less (underprovisioned) than what needed to meet capacity requirement specified with OnDeviceServiceConfiguration." `smithy_query(query="shape", shape="S3OnDeviceServiceConfiguration")` → `ServiceSize` (integer, range 3..16) "Applicable when creating a cluster. Specifies how many nodes are needed for Amazon S3 compatible storage on Snow family devices." CreateCluster declares `InvalidInputCombinationException` (HTTP 400).

Handler: `src/ops/create_cluster.rs` — the input type carries `pub force_create_jobs: bool` (`src/ops/types/create_cluster_types.rs:31`), but `handle` (lines 235-427) reads only `initial_cluster_size` (line 343) and explicitly documents the omission at lines 324-332 ("`ForceCreateJobs` guards over- and under-provisioning … which this emulator stages no data for and so cannot compute, which is why no request is treated as misprovisioned and the flag changes nothing"). A repo-wide grep for `force_create_jobs`/`ForceCreateJobs` finds no read anywhere outside the generated input struct.

Tests: no coverage asserting a refusal. The opposite is asserted — `tests/all/cluster_creation.rs:219-270` builds rows with `OnDeviceServiceConfiguration.S3OnDeviceService.ServiceSize` of 3 and of 16 while setting `InitialClusterSize` to 1, and expects a 200 create with exactly 1 node job and no `ForceCreateJobs` in the request.

## Steps to Reproduce

1. CreateAddress with a deliverable address in the signed Region; keep the returned `AddressId`.
2. CreateCluster with `JobType: LOCAL_USE`, `SnowballType: EDGE`, a valid `ShippingOption`, the `AddressId`, `InitialClusterSize: 1`, `OnDeviceServiceConfiguration: {"S3OnDeviceService": {"StorageLimit": 100.0, "StorageUnit": "TB", "ServiceSize": 16, "FaultTolerance": 4}}` and NO `ForceCreateJobs` (or `ForceCreateJobs: false`).
3. Observe HTTP 200 with a `ClusterId` and a single-entry `JobListEntries`, instead of a 400 refusing the under-provisioned cluster (the requested configuration needs 16 nodes, the order creates 1).
4. Repeat the same request with `ForceCreateJobs: true` and observe a byte-identical outcome — the flag has no effect on any response or on stored state (`DescribeCluster` reports the same cluster either way).
