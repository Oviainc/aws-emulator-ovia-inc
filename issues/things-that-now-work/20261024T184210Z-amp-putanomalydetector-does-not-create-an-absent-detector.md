Issue-Severity: LOW
Issue-Gap-Class: fidelity

# amp PutAnomalyDetector does not create a detector whose id names nothing

_Filed: 2026-10-24T18:42:09Z by the fixer of `20261024T184209Z-amp-putanomalydetector-stub.md`_
_Operation: PutAnomalyDetector | Class: fidelity_

## Operation
PutAnomalyDetector

## Description

`PutAnomalyDetector` is implemented as an UPDATE of an anomaly detector that already
exists inside the addressed workspace: it replaces that detector's whole specification,
reports it `UPDATING` while the retraining is owed, and answers the modelled
`ResourceNotFoundException` (404) when `anomalyDetectorId` names no detector the
workspace holds.

The operation's own Smithy documentation, however, says it "creates a new anomaly
detector if one doesn't exist, or updates an existing one" and calls itself "the single
method for creating, updating, and retraining anomaly detectors". That create-if-absent
path is NOT implemented.

The reason it was left out is a conflict inside the model itself, and resolving it needs
a decision this issue exists to record:

* `PutAnomalyDetectorRequest` carries no `alias` member
  (`smithy_query(query="operation",operation="PutAnomalyDetector")`).
* `alias` (`AnomalyDetectorAlias`, len 1..64, pattern `^[0-9A-Za-z][-.0-9A-Z_a-z]*$`) is
  `@required` on BOTH shapes that report a detector, `AnomalyDetectorDescription` and
  `AnomalyDetectorSummary`.

So a detector minted by this operation would have no value for a member every reader of
it must report: `DescribeAnomalyDetector` and `ListAnomalyDetectors` could not serve the
resource the create had just made. Inventing an alias (from the id, say) would be a
user-facing name the caller never gave.

## Evidence

`services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/put_anomaly_detector.rs`
— the handler's parent/child resolution answers `ResourceNotFoundException` for an
`anomalyDetectorId` the workspace does not hold, and the module header records why.

## Steps to Reproduce

1. Boot the amp emulator and create a workspace.
2. `PUT /workspaces/{workspaceId}/anomalydetectors/ad-11111111-1111-4111-8111-111111111111`
   with a valid `configuration`, for an id no `CreateAnomalyDetector` minted.
3. Observe HTTP 404 `ResourceNotFoundException` instead of a created detector.

## What a fix needs to settle

Where `alias` comes from for a detector created this way — a value the request cannot
carry today — and whether the live service really serves that call for a client-chosen
id, which no recording in this project currently shows. Until that is known, answering
the call with a created detector would mean inventing a required member.
