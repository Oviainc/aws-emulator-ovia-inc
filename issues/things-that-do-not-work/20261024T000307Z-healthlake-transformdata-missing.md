Issue-Severity: MEDIUM
Issue-Gap-Class: missing
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# TransformData has no handler: the synchronous transform endpoint is unserved

_Filed: 2026-10-24T00:03:07Z_
_Operation: TransformData | Class: missing_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TransformData` (POST /transform-data, input `ProfileId` + `InputData` + `DriftDetectionEnabled`/`ProvenanceEnabled`, output required `TransformedData` plus optional `DriftReport`) is declared in the shipped model but no code in the crate serves it: there is no `src/ops/transform_data.rs`, and no entry in `EmulatedService::override_operation` for its REST path. A caller that POSTs to the data-transformation endpoint's `/transform-data` gets the framework's unroutable-request answer rather than a transformed document. Expected: a synchronous transform of the submitted document using the named stored profile version, answering `TransformedData` (and `DriftReport` when drift detection is requested), with `ResourceNotFoundException` for an unknown ProfileId. Actual: nothing serves the path at all.

## Evidence

Spec: `smithy_query(query="operation", operation="TransformData")` gives input ProfileId/InputData/DriftDetectionEnabled/ProvenanceEnabled, output TransformedData (required) + DriftReport, errors AccessDenied/InternalServer/ResourceNotFound/Throttling/Validation, http POST /transform-data. Code: `grep -r TransformData services/service-com-amazonaws-healthlake-healthlake/src` matches nothing; the sibling out-of-closure REST operation `ValidateSource` IS hand-wired in `src/service/validate_source.rs` via `override_operation`, showing the seam exists and was simply not used for TransformData. Profiles and their mappings are already persisted (`crate::state::DataTransformationProfile`), so the input the operation needs is available.

## Steps to Reproduce

1. CreateDataTransformationProfile and note the ProfileId
2. POST /transform-data (datatransformation endpoint) with {"ProfileId":"<ProfileId>","InputData":{...}}
3. Observe the request is not routed to any handler (no TransformedData in the answer)
