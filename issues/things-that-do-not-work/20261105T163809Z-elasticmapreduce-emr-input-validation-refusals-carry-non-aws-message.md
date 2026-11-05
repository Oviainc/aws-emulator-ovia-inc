Issue-Source: trace-sweep-divergence
Issue-Trace: emr-release-label-omitted-rejections
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-release-label-omitted-rejections elasticmapreduce-multi-master-placement-config-mistakes elasticmapreduce-multi-master-topology-release-and-type-mistakes elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|DescribeReleaseLabel|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|DescribeReleaseLabel|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|RunJobFlow|4xx->4xx|$.message
Issue-Commit: 8015bc879e5141c181811b2c40a1894f3f2e5f5b

# EMR input-validation refusals carry non-AWS message text and omit ErrorCode

> _Note: this issue was identified against the code base at commit `8015bc879e5141c181811b2c40a1894f3f2e5f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReleaseLabel

## Description
Two net-new causes, both in the emulator's EMR request-validation path: when EMR refuses a malformed RunJobFlow/DescribeReleaseLabel request the emulator returns the right HTTP status but a body whose message text is not AWS's, and in one case drops a modeled error member entirely. Shared fault: the refusal payloads for input validation are authored locally instead of reproducing AWS's validation templates.

Cause 1 of 2: DescribeReleaseLabel with an empty/omitted ReleaseLabel loses the ErrorCode member and is reworded.
  AWS: DescribeReleaseLabel models exactly one client error, InvalidRequestException (Smithy: DescribeReleaseLabel errors = [InternalServerException, InvalidRequestException]; InvalidRequestException @httpError 400 with members ErrorCode (@length 1..256, "The error code associated with the exception") and Message). The recording shows AWS answering HTTP 400 with ErrorCode "InvalidInputParameter" and Message "The release label can't be empty." — that message is entirely fixed text for this error class, with no request- or state-derived parts.
  Emulator: HTTP 400 is correct, but the body omits ErrorCode (expected 'InvalidInputParameter', got None) and the Message is 'ReleaseLabel is required.' instead of "The release label can't be empty."
  Why: a client that branches on InvalidRequestException.ErrorCode sees a null code and cannot distinguish an invalid-input refusal from any other InvalidRequestException; a client (or an operator reading logs) matching on AWS's message text never matches. The modeled member being absent also makes the error shape structurally thinner than the model promises.
  Fix: DescribeReleaseLabel's input validation / error serializer in the EMR service crate — populate InvalidRequestException.ErrorCode on the empty-ReleaseLabel path and use AWS's fixed message string.
  Traces: emr-release-label-omitted-rejections#task#1.

Cause 2 of 2: RunJobFlow's constraint-violation message enumerates the PlacementGroupStrategy enum value set in the wrong order.
  AWS: recorded behavior only — the Smithy model constrains PlacementGroupConfig.PlacementStrategy to the PlacementGroupStrategy enum but does not specify the wording of the framework's constraint message. The recording shows HTTP 400 ValidationException with message "1 validation error detected: Value '<supplied value>' at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [PARTITION, SPREAD, NONE, CLUSTER]". The fixed text is "<N> validation error(s) detected: Value ... at ... failed to satisfy constraint: Member must satisfy enum value set: [...]"; the variable parts are the rejected value and the member path taken from the request (here a lowercase 'spread' at placementGroupConfigs.1.member.placementStrategy). The bracketed value list is part of the fixed template and AWS emits it in the order PARTITION, SPREAD, NONE, CLUSTER.
  Emulator: same status and same template, but the enum list is rendered in the model's declaration order — got "... Member must satisfy enum value set: [SPREAD, PARTITION, CLUSTER, NONE]" where AWS says "[PARTITION, SPREAD, NONE, CLUSTER]" (Smithy: PlacementGroupStrategy values = [SPREAD, PARTITION, CLUSTER, NONE], i.e. the emulator is printing declaration order).
  Why: clients and tests that assert on AWS's validation message (a very common pattern for enum rejections, since the message is the only machine-readable statement of the accepted set) compare unequal even though the refusal itself is correct; anyone rendering the message to a user sees a different accepted-value listing than AWS produces.
  Fix: the shared constraint/validation message formatter used by RunJobFlow's input validation in the EMR service crate — the enum value list must be emitted in AWS's order for this enum, not the model's declaration order.
  Traces: elasticmapreduce-multi-master-placement-config-mistakes#task#12 (cluster also covers #task#13, elasticmapreduce-multi-master-topology-release-and-type-mistakes#task#14, elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals#task#12 and #task#13).

Neither cause is covered by the three open EMR issues: 20261029T212242Z-elasticmapreduce-emr-unknown-id-refusal-messages-are-reworded-or.md covers unknown-ID refusal wording, 20261029T212242Z-elasticmapreduce-emr-studio-notebook-and-catalog-reads-accept-refused.md covers Studio/notebook/catalog reads and accepted-but-should-refuse requests, and 20261102T083234Z-elasticmapreduce-emr-cluster-and-instance-group-reads-omit-modeled.md covers cluster/instance-group read projections. The replay report attributes no owner to either cluster filed here.

## Evidence
Cause 1 — fixture emr-release-label-omitted-rejections, step task#1, DescribeReleaseLabel, expected 400 / actual 400:
  $.ErrorCode: missing (expected 'InvalidInputParameter', got None)
  $.Message: value_mismatch (expected "The release label can't be empty.", got 'ReleaseLabel is required.')

Cause 2 — fixture elasticmapreduce-multi-master-placement-config-mistakes, step task#12, RunJobFlow, expected 400 / actual 400 (cluster of 5 steps):
  $.message: value_mismatch
    expected: "1 validation error detected: Value 'spread' at 'placementGroupConfigs.1.member.placementStrategy' failed to satisfy constraint: Member must satisfy enum value set: [PARTITION, SPREAD, NONE, CLUSTER]"
    actual:   "1 validation error detected: Value 'spread' at 'placementGroupConfigs.1.member.placementStrategy' failed to satisfy constraint: Member must satisfy enum value set: [SPREAD, PARTITION, CLUSTER, NONE]"
  Affected: elasticmapreduce-multi-master-placement-config-mistakes#task#12, #task#13; elasticmapreduce-multi-master-topology-release-and-type-mistakes#task#14; elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals#task#12, #task#13.

Model evidence: InvalidRequestException (com.amazonaws.emr) declares members ErrorCode (@length 1..256) and Message, @httpError 400; PlacementGroupStrategy declares values [SPREAD, PARTITION, CLUSTER, NONE].

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: replay fixture emr-release-label-omitted-rejections, step task#1 — call DescribeReleaseLabel with ReleaseLabel empty/omitted. Observe HTTP 400 whose body has no ErrorCode and Message 'ReleaseLabel is required.'; AWS returns ErrorCode 'InvalidInputParameter' and Message "The release label can't be empty.".
3. Cause 2: replay fixture elasticmapreduce-multi-master-placement-config-mistakes, step task#12 — call RunJobFlow with PlacementGroupConfigs[1].PlacementStrategy set to a value outside the enum (e.g. lowercase 'spread'). Observe the 400 ValidationException message ends with "enum value set: [SPREAD, PARTITION, CLUSTER, NONE]"; AWS ends it with "enum value set: [PARTITION, SPREAD, NONE, CLUSTER]". The same cluster reproduces via elasticmapreduce-multi-master-topology-release-and-type-mistakes#task#14 and elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals#task#12.
