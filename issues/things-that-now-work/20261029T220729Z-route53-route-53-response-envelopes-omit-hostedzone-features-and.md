Issue-Source: trace-sweep-divergence
Issue-Trace: route53-create-private-zone-preserves-vpc-association
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-create-private-zone-preserves-vpc-association route53-health-check-lifecycle-and-tagging route53-traffic-policy-lifecycle-with-versions route53-traffic-policy-versioning
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|CreateHostedZone|2xx->2xx|$.CreateHostedZoneResponse.HostedZone.Features
Issue-Key: route53|GetHostedZone|2xx->2xx|$.GetHostedZoneResponse.HostedZone.Features
Issue-Key: route53|ListHealthChecks|2xx->2xx|$.ListHealthChecksResponse.MaxItems
Issue-Key: route53|ListTrafficPolicyVersions|2xx->2xx|$.ListTrafficPolicyVersionsResponse.MaxItems
Issue-Commit: a9b33a473a9762dcd2804e6444a9b0695dc27129

# Route 53 response envelopes omit HostedZone.Features and echo MaxItems as 1000

> _Note: this issue was identified against the code base at commit `a9b33a473a9762dcd2804e6444a9b0695dc27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedZone

## Description
Four success envelopes diverge from AWS in their projection: a modeled HostedZone member is never serialised, and two list operations report their internal page cap in MaxItems instead of the value the caller asked for. 4 causes.

Cause 1 of 4 — CreateHostedZone omits HostedZone.Features.
AWS: Smithy: HostedZone has a `Features` member targeting HostedZoneFeatures, documented as the hosted zone's features configuration; CreateHostedZone's output requires HostedZone (success code 201). AWS serialises the Features container on the created zone, as an empty element when no feature configuration is set.
Emulator: omits $.CreateHostedZoneResponse.HostedZone.Features entirely.
Why: clients deserialising HostedZone into a strict shape lose a modeled member, and code that inspects Features to decide whether a zone has an accelerated-recovery configuration cannot distinguish "no features" from "not reported".
Fix: the HostedZone serializer used by CreateHostedZone — always emit the Features container.
Traces: route53-create-private-zone-preserves-vpc-association#task#2.

Cause 2 of 4 — GetHostedZone omits HostedZone.Features.
AWS: same HostedZone shape on GetHostedZone's output; the read-back carries the same Features container.
Emulator: omits $.GetHostedZoneResponse.HostedZone.Features, showing the gap is in the shared HostedZone projection rather than in the create path.
Why: read-after-write of a hosted zone does not round-trip the member set AWS reports.
Fix: the shared HostedZone serializer / hosted-zone state.
Traces: route53-create-private-zone-preserves-vpc-association#task#4.

Cause 3 of 4 — ListHealthChecks reports MaxItems as its own cap.
AWS: Smithy: ListHealthChecksRequest.MaxItems is the `maxitems` query parameter and ListHealthChecksResponse.MaxItems is a required output member; the documentation states Route 53 returns at most 1000 items and clamps a larger request. AWS echoes the effective page size for the request — the caller's `maxitems` when it is at or below the cap.
Emulator: reports 1000 regardless, i.e. its internal cap rather than the requested page size.
Why: a paging client that derives its loop bound or its "is this a full page" test from the echoed MaxItems computes it against 1000 while only the requested number of items were returned, so it mis-detects truncation.
Fix: ListHealthChecks' handler — echo min(requested maxitems, cap) into the response's MaxItems.
Traces: route53-health-check-lifecycle-and-tagging#task#5.

Cause 4 of 4 — ListTrafficPolicyVersions reports MaxItems as its own cap.
AWS: Smithy: ListTrafficPolicyVersionsRequest.MaxItems is the `maxitems` query parameter and the response's MaxItems is required; AWS echoes the effective page size for the request.
Emulator: reports 1000 for the same reason as cause 3.
Why: same truncation-detection defect on the traffic-policy version listing.
Fix: ListTrafficPolicyVersions' handler — same echo rule (ideally the shared paging helper covering both listings).
Traces: route53-traffic-policy-lifecycle-with-versions#teardown#5 (cluster also covers route53-traffic-policy-versioning#task#3).

## Evidence
Replay of trace set simparity-wire-aws-v1 — statuses matched (201/200) and only body members diverged:
- route53-create-private-zone-preserves-vpc-association#task#2 CreateHostedZone: $.CreateHostedZoneResponse.HostedZone.Features missing (expected empty element, got None).
- route53-create-private-zone-preserves-vpc-association#task#4 GetHostedZone: $.GetHostedZoneResponse.HostedZone.Features missing (expected empty element, got None).
- route53-health-check-lifecycle-and-tagging#task#5 ListHealthChecks: $.ListHealthChecksResponse.MaxItems value_mismatch (expected '100', got '1000').
- route53-traffic-policy-lifecycle-with-versions#teardown#5 ListTrafficPolicyVersions: $.ListTrafficPolicyVersionsResponse.MaxItems value_mismatch (expected '100', got '1000'); cluster also covers route53-traffic-policy-versioning#task#3.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus and inspect the four steps under Traces.
3. Individually: POST /2013-04-01/hostedzone for a private zone and then GET /2013-04-01/hostedzone/{id} — neither response body contains a HostedZone/Features element.
4. GET /2013-04-01/healthcheck?maxitems=100 and GET /2013-04-01/trafficpolicies/{Id}/versions?maxitems=100 — both responses report MaxItems 1000.
