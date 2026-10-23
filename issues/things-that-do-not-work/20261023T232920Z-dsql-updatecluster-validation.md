Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# UpdateCluster accepts a multiRegionProperties.witnessRegion equal to the cluster's own Region

_Filed: 2026-10-23T23:29:20Z_
_Operation: UpdateCluster | Class: validation_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's UpdateCluster documentation states the constraint explicitly: "The witness Region specified in multiRegionProperties.witnessRegion cannot be the same as the cluster's Region."

Expected: UpdateCluster whose `multiRegionProperties.witnessRegion` equals the region of the cluster being updated is refused with HTTP 400 ValidationException (a declared error of this operation) and the cluster is left unchanged.

Actual: the handler assigns `cluster.multi_region_properties = Some(multi_region_properties)` with no inspection of `witnessRegion` (`update_cluster.rs` lines 282-284), answers 200, drives the cluster to UPDATING and then ACTIVE, and GetCluster afterwards echoes the self-referential witness region. A client relying on AWS to reject this configuration persists a multi-Region configuration that cannot exist.

## Evidence

Spec: UpdateCluster operation documentation in the Smithy model (reproduced in `src/ops/update_cluster.rs` lines 70-72); ValidationException (HTTP 400) is a declared UpdateCluster error. Handler: `src/ops/update_cluster.rs` lines 282-292 — the member is stored verbatim; `grep -i witness src/` finds no validation anywhere, only the idempotency fingerprint helper in `state.rs` lines 436-437. Tests: `tests/all/cluster_lifecycle.rs` lines 409-426 only use witness regions different from the cluster's region (eu-west-2 / eu-central-1 against a us-east-1 cluster), so no test asserts the equal-region case.

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false} against an endpoint signed for us-east-1; wait for ACTIVE.
2. POST /cluster/{id} with {"multiRegionProperties": {"witnessRegion": "us-east-1"}}.
3. Emulator answers 200 with status UPDATING. Expected HTTP 400 ValidationException naming multiRegionProperties.witnessRegion.
4. GET /cluster/{id} after the clock settles → multiRegionProperties.witnessRegion == "us-east-1" was persisted.
