Issue-Source: trace-sweep-divergence
Issue-Trace: route53-hosted-zone-accelerated-recovery-toggle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-hosted-zone-accelerated-recovery-toggle
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|GetHostedZone|2xx->2xx|$.GetHostedZoneResponse.HostedZone.Features
Issue-Commit: 5a89eb73e4cc02f021305be5f0f4930ec9a67d46

# Route 53 GetHostedZone serializes an empty HostedZone.Features element when no features are configured

> _Note: this issue was identified against the code base at commit `5a89eb73e4cc02f021305be5f0f4930ec9a67d46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHostedZone

## Description
Cause 1 of 1: GetHostedZone emits a HostedZone.Features element even when the hosted zone has no features configuration to report, where AWS omits the element entirely.

AWS: Smithy — com.amazonaws.route53#HostedZone member `Features` targets `HostedZoneFeatures` and is OPTIONAL (not in the required set; only Id, Name and CallerReference are required). `HostedZoneFeatures` is a structure of `AcceleratedRecoveryStatus` (enum) and `FailureReasons` (list), documented as "the features configuration for the hosted zone, including accelerated recovery settings and status information". Route 53 is restXml: an unset optional structure member is not serialized at all, exactly as the model documents for the sibling `Config` member ("If you omitted the HostedZoneConfig and Comment elements from the request, the Config and Comment elements don't appear in the response"). The recorded GetHostedZoneResponse for a zone whose accelerated-recovery feature is not in a reportable state therefore carries NO `Features` element under `HostedZone` — status 200, no error. The other GetHostedZone steps of the same fixture (where a features configuration does exist) pass, so the divergence is specifically the unset case.

Emulator: the GetHostedZone response carries `$.GetHostedZoneResponse.HostedZone.Features` present with an empty value (replay: `$.GetHostedZoneResponse.HostedZone.Features: unexpected_present (expected None, got '')`). Status is 200 as expected; only the response shape diverges. The serializer writes the container for the optional `Features` structure unconditionally rather than skipping it when the inner structure has no members set — it emits `<Features></Features>` with neither `AcceleratedRecoveryStatus` nor `FailureReasons`.

Why: an SDK client deserializes the present-but-empty element into a non-null `HostedZoneFeatures` object whose `AcceleratedRecoveryStatus` is null. Code written against AWS distinguishes "this zone reports no features configuration" (`features == null`) from "features exist" by the presence of the member, which is the only signal AWS gives; against the emulator that test is always true, so a caller reads a features object and then dereferences a null `AcceleratedRecoveryStatus`, or reports accelerated recovery as configured-but-unknown on every hosted zone. Strict XML/shape comparators and response-golden assertions also fail on the extra element.

Fix: the GetHostedZone response serializer / hosted-zone projection in the route53 service crate — omit the optional `Features` member (do not open its XML container) when the zone has no features configuration to report, the same way the optional `Config`/`Comment` members are already skipped. The same projection is shared by the hosted-zone read paths, so apply it wherever HostedZone is serialized rather than only in GetHostedZone's handler.

Related but NOT covering this: no open route53 issue mentions HostedZone.Features. The nearest hosted-zone issues (20261103T020342Z-route53-route-53-refusal-payloads-diverge-publiczonevpcassociation-message-is.md, 20261029T220729Z-route53-route-53-accepts-five-requests-aws-refuses-missing.md) are about refusal classes and messages, not response-shape serialization of an optional member.

Traces: route53-hosted-zone-accelerated-recovery-toggle#task#4 (GetHostedZone, 200 vs 200, `$.GetHostedZoneResponse.HostedZone.Features` unexpected_present).

## Evidence
Replay divergence (fixture route53-hosted-zone-accelerated-recovery-toggle, step task#4, operation GetHostedZone, verdict mismatch):
  expected_status 200, actual_status 200
  detail: $.GetHostedZoneResponse.HostedZone.Features: unexpected_present (expected None, got '')

Expected (AWS, recorded): GetHostedZoneResponse/HostedZone contains Id, Name, CallerReference, Config and ResourceRecordSetCount and NO Features element.
Actual (emulator): GetHostedZoneResponse/HostedZone additionally contains an empty Features element (no AcceleratedRecoveryStatus, no FailureReasons).

Model evidence: Smithy com.amazonaws.route53#HostedZone — Features: target HostedZoneFeatures, optional (required members are Id, Name, CallerReference only); HostedZoneFeatures members AcceleratedRecoveryStatus (enum), FailureReasons (list). Model doc for the sibling optional Config member states the element does not appear in the response when it was not supplied — the same omit-when-unset rule the Features element violates here.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire fixture `route53-hosted-zone-accelerated-recovery-toggle` and stop at step `task#4`.
   Equivalent by hand: create a hosted zone, run the fixture's accelerated-recovery toggle steps up to task#4, then call route53 GetHostedZone on that zone id while it has no reportable features configuration.
3. Inspect the restXml body of GetHostedZoneResponse.
4. Observed: `<HostedZone>` contains an empty `<Features></Features>` element.
   Expected: no `Features` element under `<HostedZone>` at all (status 200 either way).
