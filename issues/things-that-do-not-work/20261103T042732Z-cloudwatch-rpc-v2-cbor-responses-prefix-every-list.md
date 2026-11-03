Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.rpcv2cbor-and-awsjson-are-one-metric-store
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.rpcv2cbor-and-awsjson-are-one-metric-store|run#52|AssertionError|assert series["c"]["Values"] == [1.5], (
Issue-Commit: f6e0cb845b411895ca6ad8a253200948fbac9f34

# cloudwatch: rpc-v2-cbor responses prefix every list-of-scalars member with the member name (GetMetricData Values/Timestamps)

> _Note: this issue was identified against the code base at commit `f6e0cb845b411895ca6ad8a253200948fbac9f34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
One root cause.

Cause 1 of 1: on the rpc-v2-cbor protocol, every structure member whose target is a list of SCALARS is serialized as a CBOR array whose FIRST element is a text string equal to the member's own name, ahead of the real items. Observed on GetMetricData's MetricDataResult.Values and MetricDataResult.Timestamps. Failing check id: cloudwatch.rpcv2cbor-and-awsjson-are-one-metric-store.

AWS: CloudWatch (sdkId CloudWatch, arnNamespace/sigv4Name monitoring, apiVersion 2010-08-01) binds three protocols — Smithy: service GraniteServiceVersion20100801 protocols [awsJson1_0, awsQuery, rpcv2Cbor]. Smithy: GetMetricData output member MetricDataResults -> list of MetricDataResult; MetricDataResult.Values -> com.amazonaws.cloudwatch#DatapointValues = list<double>; MetricDataResult.Timestamps -> com.amazonaws.cloudwatch#Timestamps = list<timestamp>; MetricDataResult.Messages -> MetricDataResultMessages = list<MetricDataResultMessage> (a list of STRUCTURES). Under rpc-v2-cbor a list member is a plain CBOR array (definite or indefinite) holding exactly the modelled element values and nothing else: a list<double> holds only CBOR float/int items, a list<timestamp> holds only tag-1 (epoch) items. Smithy also documents the pairing: "The number of values always matches the number of timestamps and the timestamp for Values[x] is Timestamps[x]." The member name appears once, as the MAP key, never as an array element.

Emulator: for a response to POST /service/GraniteServiceVersion20100801/operation/GetMetricData with smithy-protocol: rpc-v2-cbor, the decoded MetricDataResult is
  {'Id': 'c', 'Label': 'M', 'Timestamps': ['Timestamps', <tag-1 datetime>], 'Values': ['Values', 1.5], 'StatusCode': 'Complete', 'Messages': []}
so the Values array holds the text string "Values" then the one double, and the Timestamps array holds the text string "Timestamps" then the one tag-1 instant. Status is 200 and content-type is application/cbor, so the framing and the discriminator are right — only the array contents are wrong. Messages, a list of structures, is emitted correctly (empty array, no name element), and the scalar members Id/Label/StatusCode are correct. Nothing about the values 1.5 / 'c' / 'M' / the instant is part of the requirement: they are whatever the request wrote; the defect is the extra leading text element, which is always the member's own name.

Why: a client relying on AWS's behavior gets corrupt data on every rpc-v2-cbor read of a list of scalars. A strict decoder fails the response outright (a text string is not a valid double for DatapointValue, and not a tag-1 timestamp for Timestamp), so GetMetricData is unusable over CBOR; a lenient decoder silently reads Values[0] as the member name instead of the first datapoint and shifts every index by one, breaking the modelled Values[x]/Timestamps[x] pairing. Because the defect is in the shared list-of-scalars serialize path, it corrupts EVERY rpc-v2-cbor response member of that kind, not just GetMetricData (GetMetricStatistics' Statistics-derived lists, ListMetrics and any other list<string>/list<double>/list<timestamp> member on this protocol).

Fix: the rpc-v2-cbor serializer for list members in the CloudWatch service's response serialization path — the generated schema-driven CBOR write for a list whose element target is a scalar must write the LIST's element schema (or the element's own schema) to each item writer, not the enclosing collection member's schema, so no member name is written inside the array. Open issue 20261022T141000Z-vendor-cbor-serializer-reemits-member-key-in-a-list-of-scalars.md describes the same codegen bug (vendor/smithy-macros/src/codegen.rs emit_write) but scopes it to the SERIALIZE side only and asserts "no service's response is wrong today — only a body this project serializes itself"; this check disproves that scope: a real CloudWatch rpc-v2-cbor RESPONSE on the wire is wrong, so the fix must cover the response path and be verified by re-replaying the check below. Also related, and NOT a substitute: 20261025T020208Z-cloudwatch-cfn-deployed-alarms-are-never-created-the.md and 20261102T120041Z-cloudwatch-putmetricstream-filter-statistics-validation-is-absent-the.md both mention rpc-v2-cbor not being served; here it IS served and answers 200 in CBOR, and the divergence is purely the list encoding.

Traces: behavioral check cloudwatch.rpcv2cbor-and-awsjson-are-one-metric-store (mismatch), assertion at check_cloudwatch_rpcv2cbor_and_awsjson_are_one_metric_store.py line 289: "CBOR read of the CBOR-written datum gave {... 'Timestamps': ['Timestamps', datetime(...)], 'Values': ['Values', 1.5] ...}, expected Values [1.5]".

## Evidence
Expected (real AWS, rpc-v2-cbor GetMetricData, one datapoint in the queried bucket):
  MetricDataResults[0] = {Id: "c", Label: "M", Timestamps: [<tag-1 epoch instant>], Values: [<double>], StatusCode: "Complete", Messages: []}
  i.e. Values is a CBOR array of exactly the modelled doubles, Timestamps a CBOR array of exactly the modelled tag-1 instants, len(Values) == len(Timestamps).

Actual (emulator, HTTP 200, content-type application/cbor, smithy-protocol echoed):
  MetricDataResults[0] = {'Id': 'c', 'Label': 'M', 'Timestamps': ['Timestamps', datetime(..., tzinfo=utc)], 'Values': ['Values', 1.5], 'StatusCode': 'Complete', 'Messages': []}
  — the member name is emitted as element 0 of each list-of-scalars array. Values has length 2 where the modelled content is length 1; Timestamps[0] decodes as the text string 'Timestamps' instead of a datetime.

Assertion raised by the replay:
  AssertionError: CBOR read of the CBOR-written datum gave {'Id': 'c', 'Label': 'M', 'Timestamps': ['Timestamps', datetime.datetime(...)], 'Values': ['Values', 1.5], 'StatusCode': 'Complete', 'Messages': []}, expected Values [1.5]

Contrast inside the same response object, which localises the defect to lists of scalars:
  - Messages (list<MetricDataResultMessage>, a list of structures) -> [] , correct.
  - Id / Label / StatusCode (scalars) -> correct.
  - Values (list<double>) and Timestamps (list<timestamp>) -> name-prefixed, wrong.

Smithy citations: service GraniteServiceVersion20100801 protocols [awsJson1_0, awsQuery, rpcv2Cbor], sigv4Name monitoring; MetricDataResult.Values -> DatapointValues = list<double>; MetricDataResult.Timestamps -> Timestamps = list<timestamp>; MetricDataResult.Messages -> MetricDataResultMessages = list<MetricDataResultMessage>; MetricDataResult.Values doc: "The number of values always matches the number of timestamps and the timestamp for Values[x] is Timestamps[x]."

No HTTP status, error code or error message diverges here: the response is a 200 success whose body is mis-encoded.

## Steps to Reproduce
1. Pick a fresh namespace, e.g. "sim2real/h1/cbor-<random>", and an instant truncated to the second.
2. Write one datum over rpc-v2-cbor: POST to /service/GraniteServiceVersion20100801/operation/PutMetricData with headers smithy-protocol: rpc-v2-cbor, content-type: application/cbor, accept: application/cbor, SigV4-signed for signing name "monitoring", and a CBOR body {"Namespace": <ns>, "MetricData": [{"MetricName": "M", "Dimensions": [{"Name":"writer","Value":"cbor"},{"Name":"run","Value":"a"}], "Timestamp": <tag-1 float64 epoch>, "Value": 1.5, "Unit": "Count"}]}. It answers 200.
3. Read it back over rpc-v2-cbor: POST to /service/GraniteServiceVersion20100801/operation/GetMetricData with the same CBOR headers and body {"MetricDataQueries": [{"Id": "c", "ReturnData": true, "MetricStat": {"Metric": {"Namespace": <ns>, "MetricName": "M", "Dimensions": <the same two dimensions>}, "Period": 60, "Stat": "Sum"}}], "StartTime": <instant-5min>, "EndTime": <instant+5min>}.
4. CBOR-decode the 200 response body and inspect MetricDataResults[0]. Observed: Values == ['Values', 1.5] and Timestamps == ['Timestamps', <datetime>]. Expected: Values == [1.5] and Timestamps == [<datetime>], with Timestamps[0] decoding as a tag-1 instant.
5. Note that Messages (a list of structures) in the same object is emitted correctly as [], which isolates the defect to list-of-scalars members.
6. Re-replay behavioral check cloudwatch.rpcv2cbor-and-awsjson-are-one-metric-store to confirm the fix; it also asserts the awsJson1.0 and rpc-v2-cbor readers share one metric store and agree on the instant, which already holds.
