Issue-Source: trace-sweep-divergence
Issue-Trace: directconnect-catalog-describe-locations
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: directconnect-catalog-describe-locations
Issue-Shape-Id: com.amazonaws.directconnect#OvertureService
Issue-Key: directconnect|DescribeLocations|2xx->5xx|$.locations
Issue-Key: directconnect|DescribeLocations|2xx->5xx|status
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# DescribeLocations unimplemented: 501 instead of modeled Locations

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLocations

## Description
Cause 1 of 1: Direct Connect DescribeLocations is not served by the emulator — the request is answered with HTTP 501 and no modeled response body, so the operation's entire response shape is absent.

AWS: DescribeLocations takes no input (Smithy: DescribeLocations input_shape=Unit) and returns the Locations structure on HTTP 200, whose single member `locations` targets LocationList (Smithy: Locations.locations -> LocationList). Each Location member set is locationCode, locationName, region, availablePortSpeeds, availableProviders, availableMacSecPortSpeeds, availableBillingModes (Smithy: structure com.amazonaws.directconnect#Location). The only modeled failures are DirectConnectClientException (client, HTTP 400) and DirectConnectServerException (server, HTTP 500); 501 is not a modeled outcome for this operation at all. The recorded response for this call is HTTP 200 with a populated `locations` list (recorded behavior only for the specific catalog contents — the location codes, provider names and port-speed sets are region/account catalog data and are examples, not requirements; what is required is status 200 and a `locations` list of Location structures).

Emulator: the replayed step returned status 501 where 200 was expected, and `$.locations` was missing from the body entirely (replay detail: "status: expected 200, got 501" / "$.locations: missing"). Hypothesis: the operation has no handler wired into the Direct Connect service's dispatch table, so the fallthrough unimplemented-operation path answers 501.

Why: DescribeLocations is the discovery call clients make before creating a connection or a LAG — the AWS CLI/SDK, Terraform's aws_dx_* resources and any provisioning flow call it to pick a locationCode and to validate the requested port speed against availablePortSpeeds. Against the emulator every such client fails with an unmodeled 501 that no SDK error mapping covers (it is neither DirectConnectClientException nor DirectConnectServerException), so the failure surfaces as a transport/unknown error rather than a service error and no retry or fallback logic can handle it. Any workflow that selects a location, or that enumerates MACsec-capable or billing-mode-specific locations, cannot proceed.

Fix: implement the DescribeLocations handler in the Direct Connect service crate and bind it into that service's operation dispatch so the operation stops falling through to the 501 unimplemented path. It must serialize a Locations output on HTTP 200 with a `locations` list of Location structures drawn from a location catalog held in the service's state, emitting locationCode, locationName, region, availablePortSpeeds, availableProviders, availableMacSecPortSpeeds and availableBillingModes (each billing-mode entry carrying billingMode, availablePortSpeeds and, where applicable, includedRegions). The operation takes no input, so there is no request validation to add; an empty catalog must still return 200 with an empty list rather than an error.

Traces: directconnect-catalog-describe-locations#task#1 (DescribeLocations, expected 200, got 501, $.locations missing).

## Evidence
Fixture directconnect-catalog-describe-locations, step task#1, operation DescribeLocations.

Expected (recorded AWS response): HTTP 200, body containing `locations` as a list of Location structures, e.g. an entry with locationCode "EqSe2-EQ", locationName "Westin Building Exchange, Seattle, WA | Equinix, SE2, Seattle, WA", region "us-west-2", availablePortSpeeds ["100G","1G","10G"], availableMacSecPortSpeeds ["100G","10G"], availableProviders [...], availableBillingModes [{billingMode "PayAsYouGo", availablePortSpeeds [...]}, {billingMode "FlatRateTier1", includedRegions [...]}, ...]. The specific catalog values are examples; the requirement is status 200 plus a `locations` list of Location structures.

Actual (emulator): HTTP 501 with no `locations` member in the body.

Replay divergence key: service=directconnect|DescribeLocations|status=200->501|keys=$.locations,status (verdict: mismatch, 1 affected step).

Model grounding: Smithy DescribeLocations input_shape=Unit, output_shape=Locations (locations -> LocationList); modeled errors DirectConnectClientException (400) and DirectConnectServerException (500) only — 501 is not a modeled response for this operation.

## Steps to Reproduce
1. Boot the emulator built from this checkout (Direct Connect is bootstrapped: service-com-amazonaws-directconnect-overtureservice).
2. Replay fixture directconnect-catalog-describe-locations, step task#1 — i.e. issue a Direct Connect DescribeLocations call with an empty request payload against the emulator's data listener.
3. Observe the response status is 501 and the body carries no `locations` member.
4. Expected: HTTP 200 and a body whose `locations` member is a list of Location structures (locationCode, locationName, region, availablePortSpeeds, availableProviders, availableMacSecPortSpeeds, availableBillingModes).
