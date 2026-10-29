Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconnect-catalog-list-offerings
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconnect-catalog-list-offerings mediaconnect-flow-error-paths
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect|ListOfferings|2xx->2xx|$.offerings[*]
Issue-Key: mediaconnect|DescribeFlow|4xx->4xx|$.message
Issue-Commit: bcbe4ab72d6cacb413c4017b750730a99b5d228f

# MediaConnect read paths return emulator-authored content: a fabricated ListOfferings catalog and a NotFound message that echoes the request

> _Note: this issue was identified against the code base at commit `bcbe4ab72d6cacb413c4017b750730a99b5d228f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListOfferings

## Description
Two net-new wire divergences on service `mediaconnect` from trace set simparity-wire-aws-v1. Both are read paths where the HTTP status is right but the body is content the emulator authored for itself rather than what AWS returns: a synthesised reservation catalog, and an error message whose text the emulator builds from the caller's own input. Both causes must be closed before this issue is submitted.

=== Cause 1 of 2: ListOfferings returns a synthesised offering catalog whose ARNs, bitrates, prices and descriptions are the emulator's own, not AWS's reservation catalog ===
AWS: Smithy: ListOfferings, GET /v1/offerings, success code 200, output {offerings: __listOfOffering, nextToken}; paginated (items Offerings, pageSize MaxResults, range 1..1000); errors BadRequestException (400), TooManyRequestsException (429), InternalServerErrorException (500), ServiceUnavailableException (503). The model does not fix the catalog's contents, so for the entries themselves this is recorded behavior only: the recorded 200 lists outbound-bandwidth offerings whose `offeringArn` has NO account-id segment — the form is `arn:aws:mediaconnect:<region>:offering:<numeric offering id>` — whose `offeringDescription` follows the template `Reserved outbound bandwidth of <reservedBitrate>Mbps at $<pricePerUnit>/hour`, and whose `pricePerUnit` is the catalog's real per-hour price for that bitrate, with `priceUnits: HOURLY`, `durationUnits: MONTHS`, `currencyCode: USD` and `resourceSpecification.resourceType: Mbps_Outbound_Bandwidth`. The bitrates offered, their numeric offering ids and their prices are catalog data taken from AWS's published MediaConnect reservation catalog; a single recording's ids and prices fill the template rather than defining it.
Emulator: the step answered HTTP 200, but every element of `$.offerings` differs from the recorded catalog, so each index is reported as a value mismatch (12 indices, `$.offerings[0]` .. `$.offerings[11]`). The served entries are machine-generated: `offeringArn` is `arn:aws:mediaconnect:<region>:000000000000:offering:1-000100mbps-12months` — an extra account-id segment AWS's offering ARNs do not carry, plus a synthetic slug instead of a numeric offering id — `offeringDescription` is `100 Mbps outbound bandwidth for 12 months` (not AWS's `Reserved outbound bandwidth of ...Mbps at $.../hour` wording), `pricePerUnit` is a round placeholder (`0.1000` for 100 Mbps, `0.2000` for 200 Mbps, i.e. $0.001 per Mbps by construction), and the bitrate ladder itself (100, 200, ...) is not the catalog's. The shared scalars (currencyCode, duration, durationUnits, priceUnits, resourceType) do match.
Why: ListOfferings is the discovery call a client makes before PurchaseOffering, so against the emulator it picks an offeringArn that does not exist in any account, in a format that fails any client-side ARN parse expecting no account segment, and shows users prices and capacities that are not the service's. Reservation/cost tooling tested this way computes wrong numbers.
Fix: the ListOfferings handler and the offerings catalog it reads from state — seed the catalog from AWS's published MediaConnect reservation offerings (bitrate ladder, numeric offering ids, per-hour prices) and mint `offeringArn` in AWS's account-less form, with `offeringDescription` rendered from AWS's template. Keep the existing pagination contract.
Traces: mediaconnect-catalog-list-offerings#task#1.

=== Cause 2 of 2: DescribeFlow's NotFoundException message interpolates the caller's unparseable flow identifier instead of AWS's fixed text for an unresolvable path ===
AWS: Smithy: DescribeFlow, GET /v1/flows/{FlowArn}, FlowArn is @required @httpLabel with pattern `^arn:.+:mediaconnect.+:flow:.+$`; errors include NotFoundException (httpError 404) and BadRequestException (400). Recorded behavior: when the path segment is not a well-formed flow ARN at all, the request never reaches the flow lookup — the response is HTTP 404 with the message `Not found: 404 Not Found`, i.e. the service's `Not found: ` envelope wrapping the generic upstream 404 reason phrase. That whole string is fixed text for this case: the caller-supplied identifier is NOT echoed into it.
Emulator: the same step answered HTTP 404 (status and error class agree) with the message `Not found: not-a-valid-flow-arn` — the `Not found: ` envelope followed by the raw path segment from the request. So the only variable part of the emulator's message is a request value that AWS's message does not contain.
Why: clients and operators that match MediaConnect's refusal text do not recognise this message, and they cannot tell "the identifier was not a flow ARN" (AWS's generic 404) from "that flow ARN does not exist" (the real NotFoundException, which does name the ARN). It also reflects arbitrary caller-controlled input back into an error body.
Fix: the flow-ARN resolution path shared by DescribeFlow and its siblings (its validation, ahead of the store lookup) — when the path label does not satisfy the modelled FlowArn pattern, emit the fixed `Not found: 404 Not Found` body for the 404 instead of formatting the request value into the message; keep the ARN-naming message only for a well-formed ARN that resolves to nothing.
Traces: mediaconnect-flow-error-paths#task#3.

## Evidence
Replay of simparity-wire-aws-v1: 8 fixtures, 50 steps, 45 passed, 5 mismatched (score 90%), emulator_up=true, bootstrap_signal=false. Both clusters are `verdict: mismatch` with empty `stub_path` and empty `owned_by`.

Cause 1 — fixture mediaconnect-catalog-list-offerings, step task#1, operation ListOfferings:
  expected_status 200, actual_status 200
  divergence_key: service=mediaconnect|ListOfferings|status=200->200|keys=$.offerings[0],$.offerings[1],...,$.offerings[11]  (all 12 list elements diverge)
  detail (first two elements):
    $.offerings[0]: expected {'duration': 12, 'durationUnits': 'MONTHS', 'resourceSpecification': {'reservedBitrate': 50, 'resourceType': 'Mbps_Outbound_Bandwidth'}, 'priceUnits': 'HOURLY', 'pricePerUnit': '1.161', 'currencyCode': 'USD', 'offeringDescription': 'Reserved outbound bandwidth of 50Mbps at $1.161/hour', 'offeringArn': 'arn:aws:mediaconnect:us-west-2:offering:80246697'}
                     got {'duration': 12, 'durationUnits': 'MONTHS', 'resourceSpecification': {'reservedBitrate': 100, 'resourceType': 'Mbps_Outbound_Bandwidth'}, 'priceUnits': 'HOURLY', 'pricePerUnit': '0.1000', 'currencyCode': 'USD', 'offeringDescription': '100 Mbps outbound bandwidth for 12 months', 'offeringArn': 'arn:aws:mediaconnect:us-west-2:000000000000:offering:1-000100mbps-12months'}
    $.offerings[1]: expected reservedBitrate 1500 / pricePerUnit '11.444' / offeringArn 'arn:aws:mediaconnect:us-west-2:offering:80248314'; got reservedBitrate 200 / pricePerUnit '0.2000' / offeringArn 'arn:aws:mediaconnect:us-west-2:000000000000:offering:1-000200mbps-12months'
  The structural requirements this shows: offeringArn carries no account-id segment and ends in a numeric offering id; offeringDescription uses AWS's 'Reserved outbound bandwidth of <N>Mbps at $<price>/hour' template; the bitrate/price pairs come from AWS's catalog, not from a $0.001-per-Mbps formula. The individual ids/prices quoted above are that recording's catalog values.

Cause 2 — fixture mediaconnect-flow-error-paths, step task#3, operation DescribeFlow:
  expected_status 404, actual_status 404
  detail: $.message: value_mismatch (expected 'Not found: 404 Not Found', got 'Not found: not-a-valid-flow-arn')
  divergence_key: service=mediaconnect|DescribeFlow|status=404->404|keys=$.message
  'Not found: 404 Not Found' is the whole fixed text; 'not-a-valid-flow-arn' is the request's own path segment, which AWS's message does not include.

## Steps to Reproduce
Boot the emulator and replay the wire corpus (replay_wire_corpus), or with a botocore `mediaconnect` client pointed at the emulator endpoint:

Cause 1: mc.list_offerings() -> inspect Offerings[]: observe OfferingArn values of the form 'arn:aws:mediaconnect:<region>:000000000000:offering:1-000100mbps-12months' (account-id segment present, synthetic slug id), OfferingDescription '100 Mbps outbound bandwidth for 12 months', and PricePerUnit values generated as bitrate*0.001, instead of AWS's catalog entries whose ARNs are 'arn:aws:mediaconnect:<region>:offering:<numeric id>' and whose descriptions read 'Reserved outbound bandwidth of <N>Mbps at $<price>/hour'. Fixture mediaconnect-catalog-list-offerings step task#1.

Cause 2: issue GET /v1/flows/not-a-valid-flow-arn against the emulator data endpoint (or mc.describe_flow(FlowArn='not-a-valid-flow-arn') with client-side validation disabled) -> observe HTTP 404 with message 'Not found: not-a-valid-flow-arn' instead of 'Not found: 404 Not Found'. Fixture mediaconnect-flow-error-paths step task#3.
