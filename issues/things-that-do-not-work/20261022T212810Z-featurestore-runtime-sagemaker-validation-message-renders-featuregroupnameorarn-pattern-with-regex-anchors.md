Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-featurestore-runtime-batch-get-record-malformed-name
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-featurestore-runtime-batch-get-record-malformed-name sagemaker-featurestore-runtime-batch-write-record-malformed-name sagemaker-featurestore-runtime-delete-record-malformed-name sagemaker-featurestore-runtime-get-record-malformed-name sagemaker-featurestore-runtime-list-records-malformed-name-and-limit sagemaker-featurestore-runtime-put-record-malformed-name sagemaker-featurestore-runtime-update-record-malformed-name
Issue-Shape-Id: com.amazonaws.sagemakerfeaturestoreruntime#AmazonSageMakerFeatureStoreRuntime
Issue-Key: featurestore-runtime.sagemaker|BatchGetRecord|4xx->4xx|$.message
Issue-Key: featurestore-runtime.sagemaker|BatchWriteRecord|4xx->4xx|$.message
Issue-Key: featurestore-runtime.sagemaker|DeleteRecord|4xx->4xx|$.message
Issue-Key: featurestore-runtime.sagemaker|GetRecord|4xx->4xx|$.message
Issue-Key: featurestore-runtime.sagemaker|ListRecords|4xx->4xx|$.message
Issue-Key: featurestore-runtime.sagemaker|PutRecord|4xx->4xx|$.message
Issue-Key: featurestore-runtime.sagemaker|UpdateRecord|4xx->4xx|$.message
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# Validation message renders FeatureGroupNameOrArn pattern with regex anchors AWS omits

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetRecord

## Description
This bundle has 7 causes - one per bound operation of SageMaker FeatureStore Runtime. All seven share a single root cause in how the service's input-validation error message renders the modeled @pattern constraint, and every one must be closed before submitting.

SHARED ROOT CAUSE: the validation-error message builder interpolates the Smithy @pattern string VERBATIM, including the leading '^' and trailing '$' anchors that the Smithy export carries. AWS's validation framework renders the same constraint WITHOUT those anchors. Status, content-type and errortype are now all correct on every one of the seven operations; the residual divergence is exactly the two anchor characters inside $.message.

AWS (common to all seven): every operation takes the feature group name as FeatureGroupNameOrArn - Smithy shape com.amazonaws.sagemakerfeaturestoreruntime#FeatureGroupNameOrArn, type string, @length min 1 max 150, @pattern ^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$ (smithy_query query=shape on services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/smithy-model/com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime.json). A value violating that pattern is rejected before any store lookup with HTTP 400, x-amzn-errortype ValidationException (the model also declares the service-common ValidationError, errorKind client, httpError 400). The message is BUILT FROM THE REQUEST, so its rule is known and is the requirement: "1 validation error detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", where <pattern> is the constraint with the anchors stripped - i.e. "(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})". All seven recordings agree on that rendering across three distinct member paths, so it is the framework's rule and not an artifact of one recording. The quoted value and the member path are examples of the rule, not requirements in themselves.

Emulator (common to all seven): returns HTTP 400 as expected, but the message ends "...regular expression pattern: ^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$" - the Smithy pattern echoed with its '^' and '$' anchors intact. Every other compared field (status 400, content-type, x-amzn-errortype, member path, quoted value, the "1 validation error detected" prefix) matches.

Why (common to all seven): a client that parses or asserts on the constraint text - contract tests, SDK error-message fixtures, tooling that re-compiles the reported regex to explain the rejection to a user - sees a pattern string that differs from AWS's for every single constrained member of this service. Because the message rule is derived mechanically from the model, the same formatter defect will mis-render the @pattern constraint of every other operation and service that validates a pattern-constrained member, so the blast radius is much wider than these seven fixtures. Status and error code are correct, so an SDK's error branch still fires; the damage is limited to the message text, hence LOW.

Fix (common to all seven): the shared input-validation error-message formatter (the component that turns a failed @pattern constraint into the "failed to satisfy constraint: Member must satisfy regular expression pattern: ..." clause) - strip a leading '^' and a trailing '$' from the modeled pattern when interpolating it into the message, while continuing to MATCH with the fully anchored pattern. Do not change the matching semantics: the anchors must still be enforced when deciding whether the value is valid, they must only be absent from the rendered text. This is per-operation only in that each of the seven operations routes a different member path into the same formatter.

Cause 1 of 7: BatchGetRecord (POST /BatchGetRecord). Member path 'identifiers.1.member.featureGroupName'. Expected 400 with the unanchored pattern; emulator returns 400 with the anchored pattern. Trace: sagemaker-featurestore-runtime-batch-get-record-malformed-name step task#1.

Cause 2 of 7: BatchWriteRecord (POST /BatchWriteRecord). Member path 'entries.1.member.featureGroupName'. Same anchored-pattern rendering. Trace: sagemaker-featurestore-runtime-batch-write-record-malformed-name step task#1.

Cause 3 of 7: DeleteRecord (DELETE /FeatureGroup/{FeatureGroupName}). Member path 'featureGroupName'. Same anchored-pattern rendering. Trace: sagemaker-featurestore-runtime-delete-record-malformed-name step task#1.

Cause 4 of 7: GetRecord (GET /FeatureGroup/{FeatureGroupName}). Member path 'featureGroupName'. Same anchored-pattern rendering. Trace: sagemaker-featurestore-runtime-get-record-malformed-name step task#1.

Cause 5 of 7: ListRecords (POST /FeatureGroup/{FeatureGroupName}/ListRecords). Member path 'featureGroupName'. Same anchored-pattern rendering; AWS still reports exactly "1 validation error detected" here even though the request also carries an out-of-range limit, so the formatter must not start aggregating extra errors while fixing the rendering. Trace: sagemaker-featurestore-runtime-list-records-malformed-name-and-limit step task#1.

Cause 6 of 7: PutRecord (PUT /FeatureGroup/{FeatureGroupName}). Member path 'featureGroupName'. Same anchored-pattern rendering. Trace: sagemaker-featurestore-runtime-put-record-malformed-name step task#1.

Cause 7 of 7: UpdateRecord (POST /FeatureGroup/{FeatureGroupName}/Record). Member path 'featureGroupName'. Same anchored-pattern rendering. Trace: sagemaker-featurestore-runtime-update-record-malformed-name step task#1.

Related open issue (does NOT cover these steps): '20261021T234924Z-featurestore-runtime-sagemaker-router-refuses-every-featurestore-runtime-sagemaker-request-as' reported the router answering 500 AmbiguousService for these same fixtures. That failure mode is gone from this replay - all seven operations now answer 400 application/json ValidationException, so routing and error serialization are working. Its replay keys are all of the form 'featurestore-runtime.sagemaker|<Op>|4xx->5xx|...'; the keys here are '...|status=400->400|keys=$.message', and the replay reports no owner for them, so closing that issue does not close this message-rendering defect.

## Evidence
Replay of simparity-wire-aws-v1: steps_total=8, steps_passed=1, steps_mismatched=7, score 12.5%, emulator_up=true ("/__admin/health answered 200 and the :33781 data listener accepts"), bootstrap_signal=false, state_reset_failures=0, not_implemented=0, steps_blocked=0. All 7 divergence clusters carry stub_path="" and owned_by=[], so none is pre-filed or owned.

Every cluster reports expected_status 400 and actual_status 400, verdict=mismatch, with $.message as the ONLY diverging key (divergence_key 'service=featurestore-runtime.sagemaker|<Operation>|status=400->400|keys=$.message'). No status, content-type or x-amzn-errortype divergence is reported for any of the seven.

Representative diff (BatchGetRecord, fixture sagemaker-featurestore-runtime-batch-get-record-malformed-name step task#1):
  $.message expected: "1 validation error detected: Value 'not a valid name!!' at 'identifiers.1.member.featureGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})"
  $.message got:      "1 validation error detected: Value 'not a valid name!!' at 'identifiers.1.member.featureGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$"
The sole textual delta is the leading '^' and the trailing '$'.

The other six clusters are character-identical apart from the member path:
  BatchWriteRecord 'entries.1.member.featureGroupName'
  DeleteRecord / GetRecord / ListRecords / PutRecord / UpdateRecord 'featureGroupName'
That the anchors are added for three different member paths and six different operations is what identifies a single shared formatter as the cause rather than seven per-operation message strings.

Smithy evidence (model services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/smithy-model/com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime.json):
- digest: sdk_id "SageMaker FeatureStore Runtime", protocol restJson1, authSchemes sigv4, 7 bound operations (BatchGetRecord, BatchWriteRecord, DeleteRecord, GetRecord, ListRecords, PutRecord, UpdateRecord) - the corpus exercises all seven; common_errors include ValidationError.
- shape FeatureGroupNameOrArn: string, @length {min:1,max:150}, @pattern "^(arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$" - i.e. the model's pattern IS anchored, which is where the stray '^'/'$' in the emulator's message come from; AWS renders it unanchored.
- shape ValidationError: structure, errorKind client, httpError 400, member Message (@length 0..2048).

Failing fixtures/steps: sagemaker-featurestore-runtime-batch-get-record-malformed-name#task#1; sagemaker-featurestore-runtime-batch-write-record-malformed-name#task#1; sagemaker-featurestore-runtime-delete-record-malformed-name#task#1; sagemaker-featurestore-runtime-get-record-malformed-name#task#1; sagemaker-featurestore-runtime-list-records-malformed-name-and-limit#task#1; sagemaker-featurestore-runtime-put-record-malformed-name#task#1; sagemaker-featurestore-runtime-update-record-malformed-name#task#1.

## Steps to Reproduce
1. Clone the repo and boot the emulator (/__admin/health answers 200 and the data listener accepts).
2. Point an SDK or raw HTTP client for SageMaker FeatureStore Runtime (protocol restJson1, sigv4 signing name 'sagemaker', endpointPrefix 'featurestore-runtime.sagemaker') at the emulator's data listener.
3. Call GetRecord with a feature group name that violates FeatureGroupNameOrArn, e.g. GET /FeatureGroup/not%20a%20valid%20name!!?RecordIdentifierValueAsString=1.
4. Observe HTTP 400 with x-amzn-errortype ValidationException (correct) whose message ends "...regular expression pattern: ^(arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$". AWS ends the same message with the pattern WITHOUT the leading '^' and trailing '$'.
5. Repeat with the same malformed name for BatchGetRecord (expect member path 'identifiers.1.member.featureGroupName'), BatchWriteRecord ('entries.1.member.featureGroupName'), DeleteRecord, ListRecords, PutRecord and UpdateRecord ('featureGroupName'): each returns the correct status and errortype but the same anchored pattern, confirming one shared formatter rather than seven separate strings.
6. Confirm the matching semantics are unchanged after the fix: a name that is valid only if the pattern is left unanchored (e.g. a name with trailing junk such as 'valid-name!!extra') must still be REJECTED with 400 - the anchors must be dropped from the rendered text only, not from the match.
7. Replay the wire corpus simparity-wire-aws-v1 and re-run all seven fixtures listed under Traces to confirm all 8 steps pass.
