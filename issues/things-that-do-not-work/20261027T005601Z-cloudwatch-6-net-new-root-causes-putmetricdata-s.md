Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.deep-cloudwatch-3-cwentity-third-entity-object-rejected cloudwatch.enable-insight-rules-and-malformed-name-reported-as-failure-entry cloudwatch.invalid-parameter-combination-family cloudwatch.metric-stream-lastupdate-timestamp-reflects-start-stop cloudwatch.put-metric-data-dimension-value-control-character-rejected cloudwatch.put-metric-data-unicode-dimension-value-round-trip cloudwatch.e2e-metric-stream-data-flow-teardown
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch|PutMetricData|InvalidParameterValue
Issue-Key: cloudwatch|EnableInsightRules|ValidationException
Issue-Key: cloudwatch.invalid-parameter-combination-family|run#33|AssertionError|assert message == 'IncludeFilters and ExcludeFilters cannot both be present', f"PutMetricStream: unexpected message {message!r}"
Issue-Key: cloudwatch.metric-stream-lastupdate-timestamp-reflects-start-stop|run#20|AssertionError|assert t1 > t0, "LastUpdateDate did not advance after StopMetricStreams"
Issue-Key: cloudwatch.put-metric-data-dimension-value-control-character-rejected|run#8|AssertionError|assert error["Message"] == (
Issue-Key: cloudwatch.e2e-metric-stream-data-flow-teardown|_await_delivered_record#20|Timeout|time.sleep(15.0)
Issue-Commit: 64e3d31da0f219acb3f0523749fc49dddb7bfb57

# CloudWatch: 6 net-new root causes — PutMetricData's EntityMetricData cap and StrictEntityValidation, EnableInsightRules refuses a batch instead of reporting per-item Failures, three invented refusal messages, GetMetricStream LastUpdateDate frozen, and the metric-stream run/deliver lifecycle never converges

> _Note: this issue was identified against the code base at commit `64e3d31da0f219acb3f0523749fc49dddb7bfb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Six net-new root causes in the cloudwatch service, grounded in 7 failing behavioral checks that the replay report marks NEITHER "ALREADY FILED" nor "OWNED by". All six must be closed before this issue is submitted; each cause names its own failing check ids.

==================================================================
Cause 1 of 6: PutMetricData enforces the EntityMetricData element cap one element too early, and StrictEntityValidation=false is not honoured as a non-fatal, header-reported partial failure.
Failing check id: cloudwatch.deep-cloudwatch-3-cwentity-third-entity-object-rejected

AWS: Smithy (com.amazonaws.cloudwatch#GraniteServiceVersion20100801) models PutMetricData with Namespace (required, @length 1..255, @pattern ^[^:]), MetricData, EntityMetricData -> EntityMetricDataList and StrictEntityValidation (boolean). EntityMetricDataList carries NO @length bound at all, so the per-request cap on EntityMetricData objects exists only in the service, not in the model — a model-driven emulator therefore has to add it by hand, and the boundary is the whole content of this cause. Modeled errors: InternalServiceFault (500), InvalidParameterCombinationException (400, awsQueryError InvalidParameterCombination), InvalidParameterValueException (400, awsQueryError InvalidParameterValue), MissingRequiredParameterException (400, MissingParameter). Recorded behavior (the check's own real-AWS replay, us-west-2, recorded 2026-10-01) fixes three points of that boundary:
  * THREE EntityMetricData objects with StrictEntityValidation=true -> HTTP 400, Error.Code 'InvalidParameterValue', Error.Message 'EntitySizeTooLarge: The total number of EntityMetricData elements must be shorter than 2'. That whole sentence is FIXED text — there are no request-derived or state-derived parts in it, and it is a service message AWS words oddly on purpose (it says "shorter than 2" while two objects are in fact accepted). None of the three metrics is ingested.
  * TWO EntityMetricData objects with StrictEntityValidation=true -> HTTP 200, NO 'x-amz-failure-message' response header, and both entities' MetricData ingested (one sample each).
  * THREE EntityMetricData objects with StrictEntityValidation=false -> HTTP 200 carrying response header 'x-amz-failure-message: EntitySizeTooLarge' (that is the header name on the wire; the API reference documents it as 'X-Amzn-Failure-Message'), and ALL THREE entities' MetricData ingested — over-counting drops the ENTITY ASSOCIATION, never the metric data.

Emulator: the strict three-object refusal is produced correctly, and then the SAME refusal is produced for a request that must succeed. After the refusal helper consumes the expected error, the next PutMetricData call in the check — the two-object strict positive control, immediately followed by the three-object lenient call, neither of which is wrapped by that helper — raises 'botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValue) when calling the PutMetricData operation: EntitySizeTooLarge: The total number of EntityMetricData elements must be shorter than 2' and the check aborts at that call site. So the emulator either applies the cap at >= 2 elements instead of > 2, or ignores StrictEntityValidation and refuses the lenient request that AWS answers 200 — the two behaviours are not separable from this replay, and BOTH are requirements: two objects under strict validation must be accepted and ingested, and three objects under lenient validation must answer 200 with 'x-amz-failure-message: EntitySizeTooLarge' and ingest all three.

Why: a client batching entity-scoped metric data loses data it believes it published. Under strict validation a legal two-entity request is refused outright; under lenient validation the documented contract is inverted — the caller asked for best-effort entity association and instead lost the whole datum batch, and it never sees the partial-failure header that tells it the entity association (and only the entity association) was dropped.

Fix: the PutMetricData handler's EntityMetricData validation in the cloudwatch crate (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) — move the cap to "more than two elements", gate it on StrictEntityValidation, and add the lenient partial-failure path that returns 200 with the 'x-amz-failure-message' response header while still ingesting every MetricData entry.

Traces: cloudwatch.deep-cloudwatch-3-cwentity-third-entity-object-rejected.

==================================================================
Cause 2 of 6: EnableInsightRules refuses the whole batch with an unmodeled ValidationException for a syntactically malformed rule name, instead of returning 200 and reporting that name as a per-item Failures[] entry.
Failing check id: cloudwatch.enable-insight-rules-and-malformed-name-reported-as-failure-entry

AWS: Smithy models EnableInsightRules with input RuleNames (required, target InsightRuleNames, a list of InsightRuleName: @length 1..128, @pattern ^[\x20-\x7E]+$) and output Failures -> BatchFailures of PartialFailure{FailureResource, ExceptionType, FailureCode, FailureDescription}. Its modeled errors are InvalidParameterValueException (400), LimitExceededException (400) and MissingRequiredParameterException (400) — ValidationException is NOT a modeled error of this operation. The presence of the PartialFailure output shape is the contract: per-item problems are REPORTED, not raised. Recorded behavior: EnableInsightRules(RuleNames=[<an existing, disabled rule>, <a well-formed name that names no rule>, <a name containing an ASCII control character>]) returns HTTP 200; the existing rule's State flips back to 'ENABLED' (readable via DescribeInsightRules); and Failures[] carries exactly two entries, independently classified — FailureResource=<the missing name> with ExceptionType='ResourceNotFoundException' and FailureCode='RULE_NAME', and FailureResource=<the malformed name> with ExceptionType='InvalidParameterException' and FailureCode='INVALID_RULE_NAME'. The ExceptionType/FailureCode pairs are the fixed vocabulary; FailureResource is the name taken verbatim from the request.

Emulator: the call is refused whole with 'ValidationException: 1 validation error detected: Value \'bad-rule\' at \'ruleNames.3.member\' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\x20-\x7E]+$'. That is an unmodeled error code for this operation, it is a whole-request refusal where AWS answers 200, no Failures[] is produced, the valid rule in the same batch is never enabled, and the two different failure reasons can no longer be distinguished. (The emulator also strips the control character out of the value it echoes — it reports Value 'bad-rule' for a request that sent '\x01bad-rule' — so even the generic message misdescribes the input.)

Why: a client that batches rule names loses the valid work in the batch because one name was bad, and loses the per-item classification it is supposed to branch on — 'this rule does not exist' (RULE_NAME) and 'this name is not a legal rule name' (INVALID_RULE_NAME) collapse into one generic 400. Any batch-oriented caller (console, IaC reconciler) retries forever or gives up on a batch AWS would have partially applied.

Fix: the EnableInsightRules handler in the cloudwatch crate — RuleNames members must NOT be validated by the shared model-constraint validator that raises ValidationException; the length/pattern check belongs in the handler's per-item loop, which emits a PartialFailure entry (ExceptionType='InvalidParameterException', FailureCode='INVALID_RULE_NAME') and continues, while an unknown-but-well-formed name emits (ExceptionType='ResourceNotFoundException', FailureCode='RULE_NAME') and every valid name is enabled. Related open issue 20261025T151319Z-cloudwatch-4-net-new-root-causes-cfn-deployed.md records that PutManagedInsightRules likewise never reports per-entry Failures; it does NOT carry this check id or this operation, so it does not cover this cause — but the two share the missing per-item failure machinery and should be fixed together.

Traces: cloudwatch.enable-insight-rules-and-malformed-name-reported-as-failure-entry.

==================================================================
Cause 3 of 6: PutMetricStream's mutually-exclusive-filter refusal carries an emulator-authored sentence instead of AWS's.
Failing check id: cloudwatch.invalid-parameter-combination-family

AWS: Smithy models PutMetricStream with both IncludeFilters and ExcludeFilters (each target MetricStreamFilters) and InvalidParameterCombinationException (client, httpError 400, awsQueryError code 'InvalidParameterCombination') among its errors. Recorded behavior: a PutMetricStream specifying BOTH IncludeFilters and ExcludeFilters answers HTTP 400, Error.Code 'InvalidParameterCombination', Error.Message 'IncludeFilters and ExcludeFilters cannot both be present'. The entire message is fixed text — it names the two member names of the model and interpolates nothing from the request or from state.

Emulator: the status and the error code are right; the message is 'IncludeFilters and ExcludeFilters cannot be used in the same operation' — a paraphrase with the same two member names but different fixed text.

Why: a client or test harness that matches the refusal text (and the surrounding check family deliberately pins every InvalidParameterCombination message in the service as one contract) treats a correctly refused call as an unknown failure; a message that differs from AWS's wording is itself the defect.

Fix: the PutMetricStream handler's include/exclude mutual-exclusion validation in the cloudwatch crate — emit AWS's fixed sentence verbatim. Related open issue 20261025T020208Z-cloudwatch-putmetricstream-getmetricstream-arn-scope-is-never-validated-empty.md carries other PutMetricStream causes (ARN scope, empty-collection normalization, IncludeLinkedAccountsMetrics default) and the separate check cloudwatch.put-metric-stream-conflicting-include-exclude-filters-rejected; it does NOT carry this check id and this message is not among its causes.

Traces: cloudwatch.invalid-parameter-combination-family (assertion at check line 179).

==================================================================
Cause 4 of 6: GetMetricStream's LastUpdateDate is frozen — StopMetricStreams (and StartMetricStreams) do not advance it.
Failing check id: cloudwatch.metric-stream-lastupdate-timestamp-reflects-start-stop

AWS: Smithy models GetMetricStream's output with BOTH CreationDate and LastUpdateDate (each target Timestamp) plus State (MetricStreamState) — two distinct timestamps, so LastUpdateDate is not a second copy of the creation time. Recorded behavior: every StartMetricStreams / StopMetricStreams call against a stream advances that stream's LastUpdateDate, so a later read is strictly greater than an earlier one and the field is a faithful recency signal for the stream's last state change. The requirement is the ORDERING (t1 > t0 across a state change), never any literal timestamp — the absolute values are per-run.

Emulator: after StopMetricStreams the stream's LastUpdateDate is byte-identical to the value read before the stop — 'AssertionError: LastUpdateDate did not advance after StopMetricStreams' (check line 74). The State field itself transitions, so the divergence is confined to the timestamp bookkeeping.

Why: LastUpdateDate is how an operator or a reconciler tells "this stream was stopped a minute ago" from "this stream has been in this state since it was created". A frozen value makes every metric-stream state change invisible to anything that diffs on recency, and makes an audit of who stopped a stream and when impossible.

Fix: the StartMetricStreams and StopMetricStreams handlers / the metric-stream state in the cloudwatch crate — stamp LastUpdateDate on every accepted state-change call (including a no-op re-stop, which AWS also stamps), leaving CreationDate untouched.

Traces: cloudwatch.metric-stream-lastupdate-timestamp-reflects-start-stop (check line 74).

==================================================================
Cause 5 of 6: PutMetricData's dimension-value rejection replaces AWS's non-ASCII message class with a generic invalid-value sentence that echoes the rejected value.
Failing check ids: cloudwatch.put-metric-data-dimension-value-control-character-rejected, cloudwatch.put-metric-data-unicode-dimension-value-round-trip

AWS: Smithy models Dimension.Value as required, target DimensionValue (@length 1..1024), and documents it: "Dimension values must contain only ASCII characters and must include at least one non-whitespace character. ASCII control characters are not supported..." (the sibling Dimension.Name member carries the same rule). PutMetricData's modeled InvalidParameterValueException is client, httpError 400, awsQueryError code 'InvalidParameterValue'. Recorded behavior: BOTH a non-ASCII dimension value (accented Latin / CJK / emoji) and an ASCII control character inside an otherwise-ASCII value are refused under the SAME message class — HTTP 400, Error.Code 'InvalidParameterValue', Error.Message 'The value for parameter MetricData.member.<i>.Dimensions.member.<j>.Value contains non-ASCII characters.' The fixed text is 'The value for parameter ' + ' contains non-ASCII characters.'; the only variable part is the member path, whose indices come from the request's own MetricData/Dimensions positions. AWS does NOT echo the offending value. Nothing is persisted: GetMetricStatistics and ListMetrics for the refused metric name both come back empty, and a clean ASCII sibling in the same namespace is accepted and round-trips byte-identical through ListMetrics.

Emulator: the status and code are right and nothing is persisted, but the message is a generic invalid-value sentence that interpolates the value: 'The value <value> for parameter MetricData.member.1.Dimensions.member.1.Value is invalid.' Observed as 'The value badvalue for parameter MetricData.member.1.Dimensions.member.1.Value is invalid.' for the control-character case (check line 39 — note the emulator silently dropped the control byte out of the value it echoed, so the message describes an input that would be legal) and 'The value café-🚀-日本語-Server_01 for parameter MetricData.member.1.Dimensions.member.1.Value is invalid.' for the non-ASCII case (check line 50). The emulator's fixed text is 'The value ' + ' for parameter ' + ' is invalid.'; AWS's class has no value slot at all.

Why: the message is the only thing that tells a caller WHICH rule it broke — AWS says "non-ASCII characters", which points at the documented ASCII-only constraint, whereas "is invalid" could be length, the empty-value rule, or the colon rule. A client that classifies refusals by message (and both these checks, plus any operator reading a log line, do) cannot tell a character-set violation from any other bad dimension value. Echoing the rejected value also leaks a caller-supplied byte string back into an error payload AWS keeps out of it.

Fix: the PutMetricData dimension validation in the cloudwatch crate — a dimension Name or Value containing any non-ASCII byte OR any ASCII control character must produce InvalidParameterValue with AWS's fixed 'The value for parameter <member-path> contains non-ASCII characters.' sentence, with the member path built from the datum's and dimension's positions in the request and with no value interpolated. The existing effect-absence behaviour (nothing stored, nothing listed) is already correct and must be preserved.

Traces: cloudwatch.put-metric-data-dimension-value-control-character-rejected (check line 39), cloudwatch.put-metric-data-unicode-dimension-value-round-trip (check line 50).

==================================================================
Cause 6 of 6: the metric-stream RUN-and-DELIVER half of the lifecycle never converges, so the end-to-end metric-stream check cannot complete.
Failing check id: cloudwatch.e2e-metric-stream-data-flow-teardown

AWS: Smithy models StartMetricStreams / StopMetricStreams over Names, and GetMetricStream's output State (MetricStreamState) / ListMetricStreams' Entries[].State. Recorded behavior: after StartMetricStreams, GetMetricStream reports State='running' and ListMetricStreams reports the same state for the same Name; a datapoint published by PutMetricData into a namespace the stream is scoped to is then DELIVERED through the stream's configured Firehose delivery stream into the destination S3 bucket, as a record naming the namespace, the metric name and the published value; StopMetricStreams returns the stream to State='stopped'; DeleteMetricStream then makes GetMetricStream answer 404 ResourceNotFoundException with an empty message.

Emulator: the check does not complete — 'Check timed out after 60.0s'. Everything it does before its first wait is bounded and fast (create the Firehose-to-S3 plumbing, create the metric-stream role, PutMetricData, PutMetricStream, StartMetricStreams); the only unbounded steps are the two waits that follow, polling GetMetricStream for State='running' and then polling the destination bucket for the delivered record. One of those two never converges: either the stream never reports 'running' after StartMetricStreams, or no record is ever written to the destination bucket for a datapoint published into the streamed namespace. This check needs no wall-clock delay of its own — against a service that transitions and delivers promptly every one of its polls is satisfied on its first iteration — so the expiry is a non-convergence, not a budget artifact. Because the wait aborts the check, its whole tail is left ungraded: the ListMetricStreams state agreement, the delivered record's namespace/metric/value shape, the stop transition and the delete-then-404 leg never run.

Why: a metric stream that reports a state but moves no data (or never reports running at all) is the exact failure mode an operator cannot detect from the control plane. Every consumer built on metric streams — a Firehose-to-S3 metric lake, a third-party monitoring sink — gets silence while the API says the pipeline is healthy.

Fix: the metric-stream runtime in the cloudwatch crate — StartMetricStreams must drive the stream to 'running' observably through GetMetricStream and ListMetricStreams, and a running stream must push every PutMetricData datum whose namespace matches its filters into its configured Firehose destination. Two open issues describe the delivery half for OTHER service tokens and do NOT carry this check id: 20261024T005347Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-export-nothing-no-records-reach.md (service cloudwatch__firehose__s3) and 20261025T032458Z-cloudwatch-firehose-s3-a-cloudformation-deployed-metric-stream.md. This cause is registered here so the cloudwatch-token end-to-end lifecycle check is re-replayed when the runtime lands.

Traces: cloudwatch.e2e-metric-stream-data-flow-teardown.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout: 166/412 passed (40.3%); service cloudwatch 156/254 (61%), 98 failed. All 7 check ids below appear in the failing list and NONE is marked "ALREADY FILED" or "OWNED by".

1. cloudwatch.deep-cloudwatch-3-cwentity-third-entity-object-rejected — verdict 'error'.
   expected: PutMetricData with 3 EntityMetricData objects + StrictEntityValidation=true -> 400 InvalidParameterValue 'EntitySizeTooLarge: The total number of EntityMetricData elements must be shorter than 2'; then with 2 objects + StrictEntityValidation=true -> 200 with NO 'x-amz-failure-message' header and both metrics ingested (SampleCount 1, Sum = the sent value); then with 3 objects + StrictEntityValidation=false -> 200 with header 'x-amz-failure-message: EntitySizeTooLarge' and all three metrics ingested; the metric sent only in the refused strict request holds no datapoints.
   actual:   botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValue) when calling the PutMetricData operation: EntitySizeTooLarge: The total number of EntityMetricData elements must be shorter than 2 — raised from a call site the check does NOT wrap in its refusal helper (the two-object strict positive control / the three-object lenient call), so the exception escapes and the check errors out.
   Smithy: EntityMetricDataList has no @length bound; PutMetricData errors = InternalServiceFault(500), InvalidParameterCombinationException(400), InvalidParameterValueException(400), MissingRequiredParameterException(400).

2. cloudwatch.enable-insight-rules-and-malformed-name-reported-as-failure-entry — verdict 'error'.
   expected: HTTP 200; Failures[] keyed by FailureResource == {<missing name>, <malformed name>}; the missing name -> ExceptionType 'ResourceNotFoundException', FailureCode 'RULE_NAME'; the malformed name -> ExceptionType 'InvalidParameterException', FailureCode 'INVALID_RULE_NAME'; the two ExceptionTypes differ; the real rule's State reads 'ENABLED' afterwards.
   actual:   botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the EnableInsightRules operation: 1 validation error detected: Value 'bad-rule' at 'ruleNames.3.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\x20-\x7E]+$
   Smithy: EnableInsightRules output member Failures -> BatchFailures of PartialFailure{FailureResource, ExceptionType, FailureCode, FailureDescription}; InsightRuleName @length 1..128 @pattern ^[\x20-\x7E]+$; modeled errors InvalidParameterValueException / LimitExceededException / MissingRequiredParameterException — ValidationException is not modeled.

3. cloudwatch.invalid-parameter-combination-family — verdict 'mismatch', check line 179.
   expected: PutMetricStream with both IncludeFilters and ExcludeFilters -> Error.Code 'InvalidParameterCombination', Error.Message 'IncludeFilters and ExcludeFilters cannot both be present'.
   actual:   AssertionError: PutMetricStream: unexpected message 'IncludeFilters and ExcludeFilters cannot be used in the same operation' (the code was correct).
   The earlier legs of the same check passed, including PutMetricData's Value+StatisticValues refusal message 'The parameters MetricData.member.1.Value and MetricData.member.1.StatisticValues are mutually exclusive and you have specified both.'

4. cloudwatch.metric-stream-lastupdate-timestamp-reflects-start-stop — verdict 'mismatch', check line 74.
   expected: t1 > t0, where t0 is GetMetricStream.LastUpdateDate after StartMetricStreams and t1 the same field after StopMetricStreams.
   actual:   AssertionError: LastUpdateDate did not advance after StopMetricStreams.
   The check's opening leg — GetMetricStream on a never-created name answering ('ResourceNotFoundException', '') — passed, so the failure is specifically the timestamp.

5. cloudwatch.put-metric-data-dimension-value-control-character-rejected — verdict 'mismatch', check line 39.
   expected: Error.Code 'InvalidParameterValue' and Error.Message 'The value for parameter MetricData.member.1.Dimensions.member.1.Value contains non-ASCII characters.' for Dimensions=[{Name:'Host', Value:'bad\x07value'}].
   actual:   AssertionError: The value badvalue for parameter MetricData.member.1.Dimensions.member.1.Value is invalid.

6. cloudwatch.put-metric-data-unicode-dimension-value-round-trip — verdict 'mismatch', check line 50.
   expected: the same code and the same message for Dimensions=[{Name:'Host', Value:'café-🚀-日本語-Server_01'}].
   actual:   AssertionError: The value café-🚀-日本語-Server_01 for parameter MetricData.member.1.Dimensions.member.1.Value is invalid.
   Message anatomy: emulator fixed text 'The value ' ... ' for parameter ' ... ' is invalid.' with the rejected value and the member path interpolated; AWS fixed text 'The value for parameter ' ... ' contains non-ASCII characters.' with ONLY the member path interpolated. The values quoted above are this run's inputs, not part of the requirement.
   Smithy: Dimension.Value doc — "Dimension values must contain only ASCII characters and must include at least one non-whitespace character. ASCII control characters are not supported..."; DimensionValue @length 1..1024.

7. cloudwatch.e2e-metric-stream-data-flow-teardown — verdict 'mismatch'.
   expected: StartMetricStreams -> GetMetricStream.State 'running' and ListMetricStreams Entries[] agreeing; a PutMetricData datum in the streamed namespace delivered through the configured Firehose into the destination S3 bucket as a record whose namespace / metric_name / value match what was published; StopMetricStreams -> 'stopped'; DeleteMetricStream -> GetMetricStream 404 ResourceNotFoundException with empty message.
   actual:   Check timed out after 60.0s. The check's only unbounded steps are the 'running' poll (interval 3 s) and the destination-bucket poll; every other step is a single bounded call, so nothing in the check requires wall-clock time of its own against a service that transitions and delivers promptly.

## Steps to Reproduce
Boot the emulator from this checkout, then:

Cause 1 — PutMetricData EntityMetricData cap / StrictEntityValidation:
1. cloudwatch:PutMetricData Namespace=<unique>, StrictEntityValidation=true, EntityMetricData=[3 objects, each {Entity:{KeyAttributes:{Type:'Service', Name:<unique-i>, Environment:<env>}}, MetricData:[{MetricName:'t<i>', Value:5.0, Timestamp:<now truncated to the minute>}]}] -> expect 400 InvalidParameterValue 'EntitySizeTooLarge: The total number of EntityMetricData elements must be shorter than 2'.
2. Repeat with only the FIRST TWO objects and StrictEntityValidation=true -> expect HTTP 200 with no 'x-amz-failure-message' header. OBSERVED: the same 400 EntitySizeTooLarge.
3. Repeat with 3 objects (metric names u0..u2) and StrictEntityValidation=false -> expect HTTP 200 with response header 'x-amz-failure-message: EntitySizeTooLarge'.
4. GetMetricStatistics (Period 60, Statistics SampleCount+Sum) for t0, t1, u0, u1, u2 -> each must hold exactly one sample of the value its accepted request carried; for t2 (sent only in the refused request) -> no datapoints.

Cause 2 — EnableInsightRules per-item Failures:
1. cloudwatch:PutInsightRule RuleName=<r>, RuleDefinition={Schema:{Name:'CloudWatchLogRule',Version:1}, AggregateOn:'Count', Contribution:{Filters:[],Keys:['$.ip']}, LogFormat:'JSON', LogGroupNames:[<any>]}.
2. cloudwatch:DisableInsightRules RuleNames=[<r>]; confirm DescribeInsightRules reports State 'DISABLED'.
3. cloudwatch:EnableInsightRules RuleNames=[<r>, <a well-formed name that names no rule>, '\x01bad-rule'] -> expect HTTP 200 with Failures[] = two entries: {FailureResource:<missing>, ExceptionType:'ResourceNotFoundException', FailureCode:'RULE_NAME'} and {FailureResource:'\x01bad-rule', ExceptionType:'InvalidParameterException', FailureCode:'INVALID_RULE_NAME'}. OBSERVED: 400 ValidationException '1 validation error detected: Value ... at \'ruleNames.3.member\' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\x20-\x7E]+$'.
4. DescribeInsightRules -> <r> must read State 'ENABLED'.

Cause 3 — PutMetricStream filter combination message:
1. cloudwatch:PutMetricStream Name=<n>, FirehoseArn=<any>, RoleArn=<any>, OutputFormat='json', IncludeFilters=[{Namespace:'AWS/EC2'}], ExcludeFilters=[{Namespace:'AWS/Lambda'}] -> expect 400 InvalidParameterCombination with Message exactly 'IncludeFilters and ExcludeFilters cannot both be present'. OBSERVED: 'IncludeFilters and ExcludeFilters cannot be used in the same operation'.
2. GetMetricStream Name=<n> must still be a not-found (the refused call created nothing).

Cause 4 — GetMetricStream LastUpdateDate:
1. cloudwatch:PutMetricStream Name=<n> (Firehose + role as above), then StartMetricStreams Names=[<n>]; read GetMetricStream and record LastUpdateDate as t0.
2. cloudwatch:StopMetricStreams Names=[<n>]; poll GetMetricStream until State='stopped' and read LastUpdateDate as t1.
3. Expect t1 > t0. OBSERVED: t1 == t0.

Cause 5 — dimension-value refusal message:
1. cloudwatch:PutMetricData Namespace=<unique>, MetricData=[{MetricName:<m>, Dimensions:[{Name:'Host', Value:'bad\x07value'}], Value:1.0, Timestamp:<now>}] -> expect 400 InvalidParameterValue 'The value for parameter MetricData.member.1.Dimensions.member.1.Value contains non-ASCII characters.'
2. Repeat with Value='café-🚀-日本語-Server_01' -> expect the identical code and message.
3. In both cases GetMetricStatistics and ListMetrics for <m> must be empty, and a clean ASCII dimension value in the same namespace must be accepted and round-trip byte-identical through ListMetrics (these legs already pass).

Cause 6 — metric-stream run/deliver lifecycle:
1. firehose:CreateDeliveryStream to an S3 bucket (with the IAM role it needs); iam:CreateRole for streams.metrics.cloudwatch.amazonaws.com with firehose:PutRecord/PutRecordBatch on that delivery stream.
2. cloudwatch:PutMetricData Namespace=<ns>, MetricData=[{MetricName:'ThroughputBytes', Value:42.0, Unit:'Bytes'}].
3. cloudwatch:PutMetricStream Name=<n> scoped to <ns> with that Firehose and role, then StartMetricStreams Names=[<n>].
4. Poll GetMetricStream Name=<n> for State='running' and ListMetricStreams for the same state -> expect 'running' promptly.
5. PutMetricData another datum into <ns> (e.g. value 4242.0) and poll the destination bucket -> expect an object containing a record whose namespace == <ns>, metric_name == 'ThroughputBytes' and one value == 4242.0. OBSERVED: the check expires at the harness's 60 s cap in step 4 or step 5.
6. StopMetricStreams -> State 'stopped'; DeleteMetricStream -> GetMetricStream answers 404 ResourceNotFoundException with an empty message.

Re-replay every check id in trace_ids to confirm the fix.
