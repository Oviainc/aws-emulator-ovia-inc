Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.get-buckets-aggregation-index-not-ready-rejected iot.indexing-configuration-enforced-thing-connectivity
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.get-buckets-aggregation-index-not-ready-rejected|run#9|AssertionError|raise AssertionError(f"unexpected error before index was ready: {code}")
Issue-Key: iot.indexing-configuration-enforced-thing-connectivity|run#14|AssertionError|assert code == "InvalidRequestException", (
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# iot fleet-indexing reads are unimplemented stubs: GetBucketsAggregation and GetThingConnectivityData answer NotImplemented, so index-not-ready and connectivity-indexing refusals are unobservable (2 causes)

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketsAggregation

## Description
Two net-new root causes, both fleet-indexing read operations that are scaffolded but not implemented. Neither operation is named by any already-filed issue in this run's report. Both were found under the `iot` service heading.

Cause 1 of 2: GetBucketsAggregation is an unimplemented scaffold stub.
AWS: Smithy GetBucketsAggregation (POST /indices/buckets) takes queryString, aggregationField and bucketsAggregationType and returns {totalCount: int, buckets: list}. It models IndexNotReadyException (400) among its errors: while the AWS_Things index is rebuilding after UpdateIndexingConfiguration the call is refused IndexNotReadyException, and once DescribeIndex reports indexStatus ACTIVE the identical call succeeds with real aggregation data.
Emulator: every call answers error code `NotImplemented` ("operation GetBucketsAggregation is scaffolded but not implemented in this emulator"), so neither the IndexNotReadyException window nor the ACTIVE-index success path can be observed.
Why: fleet-indexing aggregation is unusable, and a client cannot distinguish "index not ready" (retry) from "operation unavailable".
Fix: implement GetBucketsAggregation over the fleet index, including the IndexNotReadyException gate (services/service-com-amazonaws-iot-awsiotservice/src/ops/get_buckets_aggregation.rs).
Traces: iot.get-buckets-aggregation-index-not-ready-rejected.

Cause 2 of 2: GetThingConnectivityData is an unimplemented scaffold stub.
AWS: Smithy GetThingConnectivityData (POST /things/{thingName}/connectivity-data) returns {thingName, connected, timestamp, …} and models InvalidRequestException (400) and IndexNotReadyException (400). While thingConnectivityIndexingMode is OFF on the ACTIVE AWS_Things index the call is refused InvalidRequestException with the fixed sentence `connectivity indexing has not been enabled on AWS_Things index. Please update your indexing configuration…`; once UpdateIndexingConfiguration sets thingConnectivityIndexingMode=STATUS the identical call succeeds.
Emulator: every call answers error code `NotImplemented`, so the connectivity-indexing precondition is never enforced or reported.
Why: connectivity lookups are unavailable, and the indexing-configuration contract they enforce is unobservable.
Fix: implement GetThingConnectivityData and gate it on thingConnectivityIndexingMode (get_thing_connectivity_data.rs plus indexing-configuration state).
Traces: iot.indexing-configuration-enforced-thing-connectivity.

## Evidence
1. GetBucketsAggregation while AWS_Things is rebuilding — expected error code IndexNotReadyException (then a 200 with buckets/totalCount once ACTIVE); actual code NotImplemented (AssertionError: unexpected error before index was ready: NotImplemented).
2. GetThingConnectivityData with thingConnectivityIndexingMode=OFF — expected InvalidRequestException 'connectivity indexing has not been enabled on AWS_Things index. Please update your indexing configuration…'; actual code NotImplemented (AssertionError: expected a rejection while connectivity indexing is disabled, got NotImplemented).

## Steps to Reproduce
Replay the listed checks (suite simparity-behavioral-aws-v1):
1. UpdateIndexingConfiguration(thingIndexingConfiguration={'thingIndexingMode':'OFF'}) then {'thingIndexingMode':'REGISTRY'}, and while DescribeIndex(indexName='AWS_Things') is still rebuilding call GetBucketsAggregation(queryString='*', aggregationField=…, bucketsAggregationType={'termsAggregation':{'maxBuckets':10}}).
2. CreateThing(t); UpdateIndexingConfiguration(thingIndexingMode='REGISTRY', thingConnectivityIndexingMode='OFF'); wait for DescribeIndex ACTIVE; GetThingConnectivityData(thingName=t).
