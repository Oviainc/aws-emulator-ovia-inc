Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-cluster-lookup-and-listing-mistakes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-cluster-lookup-and-listing-mistakes elasticmapreduce-run-job-flow-instance-profile-preconditions elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes elasticmapreduce-perf-diagnosis-modify-instance-groups-refusals
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|ListClusters|4xx->2xx|$.ErrorCode
Issue-Key: elasticmapreduce|ListClusters|4xx->2xx|$.Message
Issue-Key: elasticmapreduce|ListClusters|4xx->2xx|$.__type
Issue-Key: elasticmapreduce|ListClusters|4xx->2xx|status
Issue-Key: elasticmapreduce|RunJobFlow|4xx->2xx|$.Message
Issue-Key: elasticmapreduce|RunJobFlow|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|RunJobFlow|4xx->4xx|$.__type
Issue-Key: elasticmapreduce|GetManagedScalingPolicy|4xx->4xx|$.message
Issue-Key: elasticmapreduce|ModifyInstanceGroups|4xx->4xx|$.message
Issue-Related: 20261029T212242Z-elasticmapreduce-emr-studio-notebook-and-catalog-reads-accept-refused.md
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EMR accepts requests AWS refuses and rewords cluster-id refusals on scaling paths

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClusters

## Description
Two related request-side gaps: the emulator accepts (200) requests AWS rejects with a 400, and where it does reject a malformed cluster id on the scaling paths it writes its own message instead of AWS's.

THIS ISSUE BUNDLES 5 CAUSES. All five must be closed.

Cause 1 of 5: ListClusters does not validate CreatedAfter <= CreatedBefore.
  AWS: 400 with `__type` = `InvalidRequestException`, `ErrorCode` = `INVALID_DATES_CREATED_AFTER_AFTER_CREATED_BEFORE` and `Message` = "createdAfter date cannot be after createdBefore date." — the message is entirely fixed text, with no value from the request interpolated. Smithy: ListClusters.CreatedAfter / CreatedBefore both target the timestamp shape Date; modelled error InvalidRequestException (httpError 400).
  Emulator: 200 with a normal (empty/filtered) cluster list — the inverted window is silently accepted.
  Why: a client with swapped date bounds gets an empty-looking success instead of the refusal, so an operator reads "no clusters in this window" when the real answer is "your window is invalid".
  Fix: ListClusters' input validation, before the filter is applied.
  Traces: elasticmapreduce-cluster-lookup-and-listing-mistakes#task#3.

Cause 2 of 5: RunJobFlow does not check the EC2 instance profile named by JobFlowRole.
  AWS: 400 with `__type` = `AccessDeniedException` and `Message` = "Failed to authorize instance profile <instanceProfileArn>" — fixed text "Failed to authorize instance profile " followed by the ARN of the instance profile derived from the request's JobFlowRole. Smithy: RunJobFlow.JobFlowRole -> XmlString; the modelled error list carries only InternalServerError, so the AccessDeniedException envelope is recorded behavior only.
  Emulator: 200 — the cluster is created and a JobFlowId returned even though the instance profile cannot be authorized.
  Why: a client testing its IAM wiring is told the launch succeeded, so a missing/unauthorized instance profile surfaces only much later (or never) instead of at RunJobFlow.
  Fix: RunJobFlow's precondition validation against IAM instance-profile state.
  Traces: elasticmapreduce-run-job-flow-instance-profile-preconditions#task#6.

Cause 3 of 5: RunJobFlow reports a type-coercion failure as a validation error instead of a serialization error.
  AWS: 400 with `__type` = `SerializationException` and `Message` = "NUMBER_VALUE can not be converted to a String" — entirely fixed text; this is the protocol deserializer refusing a JSON number where the modelled member is a string, raised before any business validation. Smithy: the RunJobFlow members involved target XmlString / XmlStringMaxLen256 (string shapes); SerializationException is recorded behavior only (a protocol-level error, not in the operation's modelled error list).
  Emulator: 400 with `__type` = `ValidationException` and no `Message` member at all.
  Why: wrong error class and no message — a client cannot tell a wire/serialization bug in its own request encoder from a semantic rejection of a well-formed request, and has nothing to log.
  Fix: the shared JSON request deserializer for this service must raise SerializationException with that message when a number is supplied for a string member, rather than falling through to the operation's validator.
  Traces: elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals#task#7 (also #task#8).

Cause 4 of 5: GetManagedScalingPolicy rewords AWS's malformed-cluster-id message.
  AWS: 400 with `message` = "clusterId '<value>' is not valid." — fixed text "clusterId '" + "' is not valid.", the variable part being the ClusterId taken verbatim from the request (here an ARN supplied where a j- id is required). Smithy: GetManagedScalingPolicy.ClusterId required, @length(0,256).
  Emulator: 400 with `message` = "Cluster id '<value>' is not valid." — it substitutes the other operations' "Cluster id" wording for the "clusterId" member-name wording AWS uses on this path.
  Why: the status matches but the message does not, so log scraping and message-matching assertions written against AWS fail, and the member name the client is being told about is wrong.
  Fix: GetManagedScalingPolicy's ClusterId validation message template.
  Traces: elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes#task#12 (also #task#13).

Cause 5 of 5: ModifyInstanceGroups rewords the same message.
  AWS: 400 with `message` = "clusterId '<value>' is not valid." (same template and same variable part as cause 4). Smithy: ModifyInstanceGroups.ClusterId -> ClusterId @length(0,256).
  Emulator: 400 with `message` = "Cluster id '<value>' is not valid.".
  Why: as cause 4 — a diverging message on a resize path clients routinely assert on.
  Fix: ModifyInstanceGroups' ClusterId validation message template.
  Traces: elasticmapreduce-perf-diagnosis-modify-instance-groups-refusals#task#5.

Related but NOT covering: 20261029T212242Z-elasticmapreduce-emr-unknown-id-refusal-messages-are-reworded-or.md covers other operations' reworded messages and does not own these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1, verdict=mismatch for each cluster:

1. ListClusters elasticmapreduce-cluster-lookup-and-listing-mistakes#task#3 — status expected 400, got 200; $.__type expected 'InvalidRequestException', got None; $.ErrorCode expected 'INVALID_DATES_CREATED_AFTER_AFTER_CREATED_BEFORE', got None; $.Message expected 'createdAfter date cannot be after createdBefore date.', got None.
2. RunJobFlow elasticmapreduce-run-job-flow-instance-profile-preconditions#task#6 — status expected 400, got 200; $.__type expected 'AccessDeniedException', got None; $.Message expected 'Failed to authorize instance profile arn:aws:iam::000000000000:instance-profile/<profile-name>', got None.
3. RunJobFlow elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals#task#7 — 400 vs 400; $.__type expected 'SerializationException', got 'ValidationException'; $.Message expected 'NUMBER_VALUE can not be converted to a String', got None. Same cluster also covers #task#8.
4. GetManagedScalingPolicy elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes#task#12 — 400 vs 400; $.message expected "clusterId 'arn:aws:elasticmapreduce:us-west-2:000000000000:cluster/j-3SD91U2P1PANK' is not valid.", got "Cluster id 'arn:aws:elasticmapreduce:us-west-2:000000000000:cluster/j-3SD91U2P1PANK' is not valid.". Same cluster also covers #task#13.
5. ModifyInstanceGroups elasticmapreduce-perf-diagnosis-modify-instance-groups-refusals#task#5 — 400 vs 400; $.message expected "clusterId '<arn>' is not valid.", got "Cluster id '<arn>' is not valid.".

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Representative: fixture elasticmapreduce-cluster-lookup-and-listing-mistakes, step task#3 — ListClusters with CreatedAfter later than CreatedBefore; expect 400 InvalidRequestException, observe 200.
4. Fixture elasticmapreduce-run-job-flow-instance-profile-preconditions, step task#6 — RunJobFlow with a JobFlowRole whose instance profile cannot be authorized; expect 400 AccessDeniedException.
5. Fixture elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals, step task#7 — RunJobFlow with a JSON number where a string member is modelled; expect 400 SerializationException.
6. Fixture elasticmapreduce-perf-diagnosis-cluster-id-form-mistakes, step task#12 (GetManagedScalingPolicy) and fixture elasticmapreduce-perf-diagnosis-modify-instance-groups-refusals, step task#5 (ModifyInstanceGroups), both with a cluster ARN as ClusterId — compare $.message.
