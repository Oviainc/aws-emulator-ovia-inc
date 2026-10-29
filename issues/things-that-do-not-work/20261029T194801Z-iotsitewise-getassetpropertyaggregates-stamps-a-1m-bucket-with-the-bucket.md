Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.asset-property-aggregates-average
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.asset-property-aggregates-average|run#13|AssertionError|assert got_bucket == bucket_end, f"expected aggregate bucket timestamp {bucket_end}, got {got_bucket}"
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# GetAssetPropertyAggregates stamps a 1m bucket with the bucket's START instead of its END

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAssetPropertyAggregates

## Description
Cause 1 of 1: the aggregate bucket timestamp is off by one full resolution interval.
AWS: Smithy: GetAssetPropertyAggregates returns aggregatedValues (list of AggregatedValue, each with a required timestamp and value), selected by the @required resolution member (@pattern 1m|15m|1h|1d) over the startDate/endDate window. The model does not state which edge of the bucket stamps the value — recorded behavior only: the recording shows AWS stamping a 1m bucket with the timestamp of the bucket's END, i.e. bucketStart + 60 seconds for resolution=1m (and, by the same rule, bucketStart + the resolution's length for 15m/1h/1d). Two GOOD DOUBLE values ingested at bucketStart+5 and bucketStart+35 produce one AVERAGE row whose timestamp is bucketStart+60.
Emulator: returns the right AVERAGE but stamps the row with the bucket's START (bucketStart), one whole resolution interval earlier than AWS.
Why: every aggregate row the emulator returns is attributed to the wrong minute/hour/day. A client that plots, joins or re-windows aggregates against its own clock (or against GetAssetPropertyValueHistory samples, which are stamped with the sample time) mis-aligns every series by one interval; a caller paging with startDate/endDate derived from returned timestamps can re-request or skip a bucket, and an alarm or rollup keyed on the bucket timestamp fires for the wrong period. The returned value itself being correct makes the error silent.
Fix: the GetAssetPropertyAggregates handler's bucketing/serialization step must stamp each AggregatedValue with the END of the bucket it summarises (bucket start + the requested resolution's length) for every resolution, instead of the bucket start. The same bucket-edge convention must hold for the batch sibling that shares this aggregation code.
Traces: iotsitewise.asset-property-aggregates-average.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the emulator built from mainline: 46/60 passed, all 14 failures in iotsitewise.

iotsitewise.asset-property-aggregates-average: create an asset model with a DOUBLE measurement property and an asset, BatchPutAssetPropertyValue two GOOD values (10.0 at minuteStart+5 and 30.0 at minuteStart+35) into one aligned minute safely in the past, then GetAssetPropertyAggregates with aggregateTypes=[AVERAGE], resolution=1m, startDate=minuteStart, endDate=minuteStart+60. The AVERAGE value returned is correct (20.0); the row's timestamp is wrong by exactly 60 seconds:

  AssertionError: expected aggregate bucket timestamp 1793302440, got 1793302380

1793302380 is that run's minuteStart (the bucket's start) and 1793302440 is minuteStart+60 (the bucket's end); those two epoch seconds are this run's values — the requirement is the bucket-END convention, not these numbers. The check's own comment records that the END convention was confirmed against real AWS rather than assumed.

Model grounding: GetAssetPropertyAggregates.resolution is @required with @pattern '^1m|15m|1h|1d$'; the output's aggregatedValues member is @required and each AggregatedValue carries a required timestamp. The model does not fix the bucket edge, so the grounding for the edge is the recording.

## Steps to Reproduce
1. Re-replay behavioral check iotsitewise.asset-property-aggregates-average.
2. Equivalently: create an asset model with a DOUBLE measurement property, create an asset from it, wait ACTIVE.
3. Pick minuteStart = a minute-aligned epoch second a few minutes in the past. BatchPutAssetPropertyValue two GOOD doubleValues at minuteStart+5 and minuteStart+35.
4. GET /properties/aggregates?assetId=...&propertyId=...&aggregateTypes=AVERAGE&resolution=1m&startDate=minuteStart&endDate=minuteStart+60.
5. Observe the single returned aggregatedValues entry carries timestamp == minuteStart (bucket start) where AWS returns minuteStart+60 (bucket end); the average value itself is correct.
