Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-cluster-lookup-and-listing-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-cluster-lookup-and-listing-mistakes elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes elasticmapreduce-perf-diagnosis-list-clusters-marker-and-state-mistakes emr-release-label-omitted-rejections
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|DescribeCluster|4xx->4xx|$.__type
Issue-Key: elasticmapreduce|DescribeCluster|4xx->4xx|$.message
Issue-Key: elasticmapreduce|ListInstanceGroups|4xx->4xx|$.__type
Issue-Key: elasticmapreduce|ListInstanceGroups|4xx->4xx|$.message
Issue-Key: elasticmapreduce|ListSteps|4xx->4xx|$.__type
Issue-Key: elasticmapreduce|ListSteps|4xx->4xx|$.message
Issue-Key: elasticmapreduce|ListInstanceFleets|4xx->4xx|$.__type
Issue-Key: elasticmapreduce|ListInstanceFleets|4xx->4xx|$.message
Issue-Key: elasticmapreduce|DescribeStep|4xx->4xx|$.__type
Issue-Key: elasticmapreduce|DescribeStep|4xx->4xx|$.message
Issue-Key: elasticmapreduce|DescribeCluster|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|DescribeCluster|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|ListSteps|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|ListSteps|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|ListClusters|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|ListClusters|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|ListSupportedInstanceTypes|4xx->4xx|$.__type
Issue-Related: 20261029T212242Z-elasticmapreduce-emr-validation-refusals-carry-the-wrong-error-code.md
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EMR cluster-id refusals use the wrong exception shape and omit ErrorCode

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
EMR has TWO distinct client-error envelopes on the wire and the emulator picks the wrong one per operation, so the error type, the error-code member and the message member are all wrong. AWS uses either (a) `__type: ValidationException` with a lowercase `message` member and NO ErrorCode (the framework-level request validator), or (b) `__type: InvalidRequestException` with `ErrorCode` + `Message` (the EMR service exception modelled in Smithy as InvalidRequestException{ErrorCode, Message}). The emulator serves one envelope where the other is required and drops the code/message members entirely on the InvalidRequestException path.

THIS ISSUE BUNDLES 10 CAUSES. All ten must be closed.

Cause 1 of 10: DescribeCluster with an ARN passed as ClusterId.
  AWS: 400 with `__type` = `ValidationException` and member `message` = "clusterId '<value>' is not valid." — fixed text "clusterId '" + "' is not valid.", variable part is the ClusterId value taken verbatim from the request. Smithy: DescribeCluster.ClusterId -> ClusterId @length(0,256), modelled error InvalidRequestException (httpError 400); the ValidationException envelope is recorded behavior only for this malformed-ARN case.
  Emulator: 400 with `__type` = `InvalidRequestException` and no `message` member at all.
  Why: a client that switches on the exception name sees the wrong error class and a client that reads `message` gets nothing to log or surface.
  Fix: the DescribeCluster handler's ClusterId validation and its error serializer must emit the ValidationException envelope with `message`.
  Traces: elasticmapreduce-cluster-lookup-and-listing-mistakes#task#4.

Cause 2 of 10: ListInstanceGroups with an ARN passed as ClusterId.
  AWS: 400, `__type` = `ValidationException`, `message` = "clusterId '<value>' is not valid." (same template as cause 1). Smithy: ListInstanceGroups.ClusterId required, @length(0,256).
  Emulator: 400 with `__type` = `InvalidRequestException` and no `message`.
  Why: same mis-typed error class; ARN-vs-id mistakes become unidentifiable.
  Fix: ListInstanceGroups handler validation + error serializer.
  Traces: elasticmapreduce-cluster-lookup-and-listing-mistakes#task#6.

Cause 3 of 10: ListSteps with an ARN passed as ClusterId.
  AWS: 400, `__type` = `ValidationException`, `message` = "clusterId '<value>' is not valid.". Smithy: ListSteps.ClusterId required, @length(0,256).
  Emulator: 400 with `__type` = `InvalidRequestException` and no `message`.
  Why: as above.
  Fix: ListSteps handler validation + error serializer.
  Traces: elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes#task#2.

Cause 4 of 10: ListInstanceFleets with an ARN passed as ClusterId.
  AWS: 400, `__type` = `ValidationException`, `message` = "clusterId '<value>' is not valid.". Smithy: ListInstanceFleets.ClusterId required, @length(0,256).
  Emulator: 400 with `__type` = `InvalidRequestException` and no `message`.
  Why: as above.
  Fix: ListInstanceFleets handler validation + error serializer.
  Traces: elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes#task#4.

Cause 5 of 10: DescribeStep with an ARN passed as ClusterId.
  AWS: 400, `__type` = `ValidationException`, `message` = "clusterId '<value>' is not valid.". Smithy: DescribeStep.ClusterId required, @length(0,256).
  Emulator: 400 with `__type` = `InvalidRequestException` and no `message`.
  Why: as above.
  Fix: DescribeStep handler validation + error serializer.
  Traces: elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes#task#6.

Cause 6 of 10: DescribeCluster with a null/absent ClusterId — the OPPOSITE envelope.
  AWS: 400 with `__type` = `InvalidRequestException`, `ErrorCode` = `MalformedClusterId`, `Message` = "Cluster id '<value>' is not valid." — fixed text "Cluster id '" + "' is not valid.", the variable part being the supplied id rendered as `null` when absent. Smithy: InvalidRequestException{ErrorCode @length(1,256), Message}, httpError 400.
  Emulator: 400 with `__type` = `ValidationException` and neither `ErrorCode` nor `Message` present.
  Why: the emulator inverts the envelope exactly where AWS uses the service exception, so a client cannot distinguish a malformed id (retry with a corrected id) from a framework validation reject.
  Fix: DescribeCluster handler's missing/malformed-id branch must raise InvalidRequestException with ErrorCode `MalformedClusterId`.
  Traces: elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes#task#8.

Cause 7 of 10: ListSteps with a malformed (non-ARN) cluster id.
  AWS: 400 with `__type` = `InvalidRequestException`, `ErrorCode` = `MalformedClusterId`, `Message` = "Cluster id '<clusterId>' is not valid." (variable part from the request).
  Emulator: 400 carrying neither `ErrorCode` nor `Message`.
  Why: the response body is empty of any diagnosis; SDK error-code matchers never fire.
  Fix: ListSteps handler validation must populate InvalidRequestException.ErrorCode/Message.
  Traces: elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes#task#10.

Cause 8 of 10: ListClusters with a malformed Marker.
  AWS: 400 with `ErrorCode` = `MalformedMarker` on the InvalidRequestException envelope. Smithy: ListClusters.Marker -> Marker, paginated(inputToken=Marker); modelled error InvalidRequestException.
  Emulator: 400 with the `ErrorCode` member absent.
  Why: a paginating client cannot tell a bad continuation token from any other 400 and will retry the same token.
  Fix: ListClusters marker validation must set ErrorCode `MalformedMarker`.
  Traces: elasticmapreduce-perf-diagnosis-list-clusters-marker-and-state-mistakes#task#1.

Cause 9 of 10: ListClusters with a lowercase/invalid ClusterStates entry.
  AWS: 400 with `ErrorCode` = `InvalidClusterStates` and `Message` = "Cluster states [<states>] are not valid." — fixed text "Cluster states [" + "] are not valid.", the variable part being the rejected state values from the request, comma-separated.
  Emulator: 400 with both `ErrorCode` and `Message` absent.
  Why: the client is told only "400" and cannot learn which state value was rejected.
  Fix: ListClusters ClusterStates validation must raise InvalidRequestException with that code and message.
  Traces: elasticmapreduce-cluster-lookup-and-listing-mistakes#task#2 (also elasticmapreduce-perf-diagnosis-list-clusters-marker-and-state-mistakes#task#2).

Cause 10 of 10: ListSupportedInstanceTypes with an omitted/null ReleaseLabel.
  AWS: 400 with `__type` = `InvalidRequestException`, `ErrorCode` = `INVALID_RELEASE_LABEL`, `Message` = "The supplied release label is invalid: <releaseLabel>." — fixed text "The supplied release label is invalid: " + ".", the variable part being the supplied release label, rendered `null` when absent. Smithy: ListSupportedInstanceTypes.ReleaseLabel required; modelled error InvalidRequestException.
  Emulator: 400 with `__type` = `ValidationException` and neither `ErrorCode` nor `Message`.
  Why: wrong exception class and no error code, so release-label typos are not programmatically detectable.
  Fix: ListSupportedInstanceTypes handler validation + error serializer.
  Traces: emr-release-label-omitted-rejections#task#2.

Related but NOT covering: open issue 20261029T212242Z-elasticmapreduce-emr-validation-refusals-carry-the-wrong-error-code.md owns a different set of replay keys; none of the ten clusters above is owned by it.

## Evidence
Replay of trace set simparity-wire-aws-v1, verdict=mismatch for each cluster:

1. DescribeCluster elasticmapreduce-cluster-lookup-and-listing-mistakes#task#4 — expected 400 / actual 400; $.__type expected 'ValidationException', got 'InvalidRequestException'; $.message expected "clusterId 'arn:aws:elasticmapreduce:us-west-2:000000000000:cluster/j-3SD91U2P1PANK' is not valid.", got None.
2. ListInstanceGroups ...#task#6 — $.__type expected 'ValidationException', got 'InvalidRequestException'; $.message missing.
3. ListSteps elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes#task#2 — $.__type expected 'ValidationException', got 'InvalidRequestException'; $.message missing.
4. ListInstanceFleets ...#task#4 — same, message template with cluster id j-2K8QXM4RT9VWY.
5. DescribeStep ...#task#6 — same, message template with cluster id j-9FG6TMAEBY4RS.
6. DescribeCluster ...#task#8 — $.__type expected 'InvalidRequestException', got 'ValidationException'; $.ErrorCode expected 'MalformedClusterId', got None; $.Message expected "Cluster id 'null' is not valid.", got None.
7. ListSteps ...#task#10 — $.ErrorCode expected 'MalformedClusterId', got None; $.Message expected "Cluster id 'j-3SD91U2P1PANK' is not valid.", got None.
8. ListClusters elasticmapreduce-perf-diagnosis-list-clusters-marker-and-state-mistakes#task#1 — $.ErrorCode expected 'MalformedMarker', got None.
9. ListClusters elasticmapreduce-cluster-lookup-and-listing-mistakes#task#2 — $.ErrorCode expected 'InvalidClusterStates', got None; $.Message expected 'Cluster states [waiting] are not valid.', got None.
10. ListSupportedInstanceTypes emr-release-label-omitted-rejections#task#2 — $.__type expected 'InvalidRequestException', got 'ValidationException'; $.ErrorCode expected 'INVALID_RELEASE_LABEL', got None; $.Message expected 'The supplied release label is invalid: null.', got None.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Representative: fixture elasticmapreduce-cluster-lookup-and-listing-mistakes, step task#4 (DescribeCluster with ClusterId set to a cluster ARN) — compare $.__type and $.message.
4. Repeat for each fixture#step in trace_ids: the two cluster-lookup-and-listing steps, the five perf-diagnosis-cluster-id-form steps, the list-clusters-marker step and emr-release-label-omitted-rejections#task#2.
