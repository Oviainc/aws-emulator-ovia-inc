Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-account-attributes-and-reserved-node-offerings
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-account-attributes-and-reserved-node-offerings redshift-purchase-reserved-node-offering-not-found redshift-describe-reserved-node-not-found redshift-empty-inventory-reads redshift-static-catalog-metadata-reads redshift-describe-event-categories-unknown-source-type
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeReservedNodeOfferings|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|PurchaseReservedNodeOffering|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeReservedNodes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeReservedNodeExchangeStatus|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeClusterTracks|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeEventCategories|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DescribeEventCategories|4xx->2xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift reserved-node and catalog refusals: wrong wording and a missing source-type validation

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReservedNodeOfferings

## Description
6 root causes on the reserved-node and catalog-metadata surfaces: five refusal-message divergences and one validation gate that is missing altogether.

Cause 1 of 6: DescribeReservedNodeOfferings for an unknown ReservedNodeOfferingId. Smithy: ReservedNodeOfferingNotFoundFault @error("client"), httpError 404. AWS: 404, Message "ReservedNodeOfferingId <ReservedNodeOfferingId> not found." — fixed text is the input MEMBER name "ReservedNodeOfferingId " followed by the request's value and " not found." Emulator: "Reserved node offering not found: '<id>'." (human phrasing, colon form, value single-quoted). Why: clients matching AWS's refusal text do not recognize the refusal. Fix: the DescribeReservedNodeOfferings handler's offering-lookup refusal path. Traces: redshift-account-attributes-and-reserved-node-offerings#task#5.

Cause 2 of 6: PurchaseReservedNodeOffering for an unknown ReservedNodeOfferingId. AWS: 404 ReservedNodeOfferingNotFound, "ReservedNodeOfferingId <ReservedNodeOfferingId> not found." Emulator: "Reserved node offering not found: '<id>'." Fix: PurchaseReservedNodeOffering handler refusal path. Traces: redshift-purchase-reserved-node-offering-not-found#task#1.

Cause 3 of 6: DescribeReservedNodes for an unknown ReservedNodeId. Smithy: ReservedNodeNotFoundFault httpError 404. AWS: 404, "ReservedNodeId <ReservedNodeId> not found." — again the member name as the fixed prefix. Emulator: "Reserved Node <ReservedNodeId> not found." (space-separated, title-cased). Fix: DescribeReservedNodes handler refusal path. Traces: redshift-describe-reserved-node-not-found#task#1.

Cause 4 of 6: DescribeReservedNodeExchangeStatus called with neither ReservedNodeId nor ReservedNodeExchangeRequestId. AWS: 400 InvalidParameterValue with the fixed message "You must supply one or both of the optional ReservedNodeId and ReservedNodeExchangeRequestId parameters." (no variable part). Emulator: "You must specify either the ReservedNodeId parameter or the ReservedNodeExchangeRequestId parameter." — emulator-authored prose that also states the wrong rule: AWS permits one OR BOTH, the emulator's wording says exactly one. Why: the message is the only place this input rule is communicated, and it tells the caller something AWS does not require. Fix: the DescribeReservedNodeExchangeStatus input validation message. Traces: redshift-empty-inventory-reads#task#14.

Cause 5 of 6: DescribeClusterTracks filtered by an unknown MaintenanceTrackName. Smithy: InvalidClusterTrackFault @error("client"), httpError 400, awsQueryError InvalidClusterTrack. AWS: 400, "Invalid cluster track: <MaintenanceTrackName>" — fixed prefix "Invalid cluster track: " and the request's track name, with NO trailing period. The emulator appends a period. Why: exact-match message assertions fail on a one-character difference. Fix: the DescribeClusterTracks track-validation refusal message. Traces: redshift-static-catalog-metadata-reads#task#4.

Cause 6 of 6: DescribeEventCategories with an unrecognised SourceType is ACCEPTED. AWS validates SourceType against its closed vocabulary and refuses 400 InvalidParameterValue with the message "Invalid event source type. Valid types are 'cluster', 'cluster-security-group', 'cluster-snapshot', 'cluster-parameter-group', 'scheduled-action', 'reserved-node'" — fixed sentence plus the enumerated valid-type list in that order; the offending value is not echoed. The emulator performs no validation and answers 200 with a (synthetic) catalog body. Why: a client probing which source types the service supports is told every value is valid, and a typo'd SourceType silently yields an unrelated result set instead of an input error. Fix: add SourceType validation to DescribeEventCategories' input validation, ahead of the catalog read. Traces: redshift-describe-event-categories-unknown-source-type#task#1.

## Evidence
Replay of simparity-wire-aws-v1; all six clusters verdict=mismatch.

DescribeReservedNodeOfferings (redshift-account-attributes-and-reserved-node-offerings#task#5), 404 vs 404:
  expected 'ReservedNodeOfferingId 11111111-1111-1111-1111-111111111111 not found.', got "Reserved node offering not found: '11111111-1111-1111-1111-111111111111'."
PurchaseReservedNodeOffering (redshift-purchase-reserved-node-offering-not-found#task#1), 404 vs 404:
  expected 'ReservedNodeOfferingId ebca31f8-f8d9-453b-83b7-1af927c25c65 not found.', got "Reserved node offering not found: 'ebca31f8-f8d9-453b-83b7-1af927c25c65'."
DescribeReservedNodes (redshift-describe-reserved-node-not-found#task#1), 404 vs 404:
  expected 'ReservedNodeId 6a4bcc87-2002-463c-8d81-0f6c9e4f6014 not found.', got 'Reserved Node 6a4bcc87-2002-463c-8d81-0f6c9e4f6014 not found.'
DescribeReservedNodeExchangeStatus (redshift-empty-inventory-reads#task#14), 400 vs 400:
  expected 'You must supply one or both of the optional ReservedNodeId and ReservedNodeExchangeRequestId parameters.', got 'You must specify either the ReservedNodeId parameter or the ReservedNodeExchangeRequestId parameter.'
DescribeClusterTracks (redshift-static-catalog-metadata-reads#task#4), 400 vs 400:
  expected 'Invalid cluster track: sim2real-probe-track-missing', got 'Invalid cluster track: sim2real-probe-track-missing.'
DescribeEventCategories (redshift-describe-event-categories-unknown-source-type#task#1):
  status: expected 400, got 200
  $.ErrorResponse: missing — expected Error.Type 'Sender', Error.Code 'InvalidParameterValue', Error.Message "Invalid event source type. Valid types are 'cluster', 'cluster-security-group', 'cluster-snapshot', 'cluster-parameter-group', 'scheduled-action', 'reserved-node'"

Offering/node ids and the track name shown are request values; the fixed wording around them, and for cause 6 the 400 InvalidParameterValue refusal with its valid-type list, are the requirements.

## Steps to Reproduce
1. Replay redshift-account-attributes-and-reserved-node-offerings step task#5 and redshift-purchase-reserved-node-offering-not-found step task#1 with an unknown ReservedNodeOfferingId; compare Error.Message.
2. Replay redshift-describe-reserved-node-not-found step task#1 with an unknown ReservedNodeId.
3. Replay redshift-empty-inventory-reads step task#14: DescribeReservedNodeExchangeStatus with neither optional id supplied.
4. Replay redshift-static-catalog-metadata-reads step task#4: DescribeClusterTracks with an unknown MaintenanceTrackName.
5. Replay redshift-describe-event-categories-unknown-source-type step task#1: DescribeEventCategories with a SourceType outside the documented set. Observe 200 instead of 400 InvalidParameterValue.
