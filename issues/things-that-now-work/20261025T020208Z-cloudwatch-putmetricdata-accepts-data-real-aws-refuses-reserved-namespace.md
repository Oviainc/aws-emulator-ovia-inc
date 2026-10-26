Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.put-metric-data-into-aws-reserved-namespace-rejected cloudwatch.put-metric-data-reserved-namespace-rejected cloudwatch.put-metric-data-aws-slash-namespace-rejected-case-insensitively cloudwatch.put-metric-data-timestamp-too-far-future-rejected cloudwatch.put-metric-data-timestamp-too-far-past-rejected cloudwatch.put-metric-data-timestamp-too-old-rejected cloudwatch.put-metric-data-timestamp-window-validity cloudwatch.put-metric-data-dimension-value-control-character-rejected cloudwatch.put-metric-data-unicode-dimension-value-round-trip cloudwatch.cw-put-metric-data-oversized-batch-rejected-and-no-partial-ingestion cloudwatch.deep-cloudwatch-3-cwentity-shared-metric-budget-overflow-rejected cloudwatch.put-metric-data-conflicting-value-and-statistic-values-rejected cloudwatch.put-metric-data-rejects-value-and-statisticvalues-combo cloudwatch.put-metric-data-value-and-statistic-values-combo-rejected cloudwatch.invalid-parameter-combination-family cloudwatch.put-metric-data-invalid-value-rejected cloudwatch.put-metric-data-nan-value-rejected cloudwatch.put-metric-data-storage-resolution-invalid-value-rejected cloudwatch.put-metric-data-storage-resolution-out-of-range-rejected cloudwatch.deep-cloudwatch-3-cwentity-omitted-strict-flag-rejected cloudwatch.deep-cloudwatch-3-cwentity-strict-validation-rejects-malformed-entity cloudwatch.deep-cloudwatch-3-cwentity-third-entity-object-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.put-metric-data-into-aws-reserved-namespace-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudwatch.put-metric-data-reserved-namespace-rejected|_rejects#4|AssertionError|raise AssertionError(
Issue-Key: cloudwatch.put-metric-data-aws-slash-namespace-rejected-case-insensitively|_error#9|AssertionError|raise AssertionError("call was ACCEPTED; real AWS rejects it")
Issue-Key: cloudwatch.put-metric-data-timestamp-too-far-future-rejected|_error#7|AssertionError|raise AssertionError(f"real AWS rejects this timestamp, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.put-metric-data-timestamp-too-far-past-rejected|_error#7|AssertionError|raise AssertionError(f"real AWS rejects this timestamp, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.put-metric-data-timestamp-too-old-rejected|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.put-metric-data-timestamp-window-validity|_err#9|AssertionError|raise AssertionError(f"real AWS rejects this timestamp, but the call succeeded: {result!r}")
Issue-Key: cloudwatch.put-metric-data-dimension-value-control-character-rejected|_error#6|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.put-metric-data-unicode-dimension-value-round-trip|run#6|AssertionError|raise AssertionError("non-ASCII dimension value was accepted instead of rejected")
Issue-Key: cloudwatch.cw-put-metric-data-oversized-batch-rejected-and-no-partial-ingestion|run#14|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: cloudwatch.deep-cloudwatch-3-cwentity-shared-metric-budget-overflow-rejected|run#6|AssertionError|assert message == "The collection MetricData must not have a size greater than 1000.", (
Issue-Key: cloudwatch.put-metric-data-conflicting-value-and-statistic-values-rejected|run#8|AssertionError|assert error["Message"] == (
Issue-Key: cloudwatch.put-metric-data-rejects-value-and-statisticvalues-combo|run#8|AssertionError|assert error["Message"] == (
Issue-Key: cloudwatch.put-metric-data-value-and-statistic-values-combo-rejected|run#9|AssertionError|assert err.get("Message", "") == expected, f"expected {expected!r}, got {err.get('Message', '')!r}"
Issue-Key: cloudwatch.invalid-parameter-combination-family|run#6|AssertionError|assert message == 'The parameters MetricData.member.1.Value and MetricData.member.1.StatisticValues are mutually exclusive and you have specified both.', f"PutM
Issue-Key: cloudwatch.put-metric-data-invalid-value-rejected|run#6|AssertionError|assert error["Message"] == "The value NaN for parameter MetricData.member.1.Value is invalid.", error["Message"]
Issue-Key: cloudwatch.put-metric-data-nan-value-rejected|run#7|AssertionError|assert error["Message"] == "The value NaN for parameter MetricData.member.1.Value is invalid.", error["Message"]
Issue-Key: cloudwatch.put-metric-data-storage-resolution-invalid-value-rejected|run#7|AssertionError|assert error["Message"] == (
Issue-Key: cloudwatch.put-metric-data-storage-resolution-out-of-range-rejected|run#6|AssertionError|assert error["Message"] == (
Issue-Key: cloudwatch.deep-cloudwatch-3-cwentity-omitted-strict-flag-rejected|_error#5|AssertionError|raise AssertionError(f"call was expected to be refused but succeeded: {resp!r}")
Issue-Key: cloudwatch.deep-cloudwatch-3-cwentity-strict-validation-rejects-malformed-entity|_error#5|AssertionError|raise AssertionError(f"call was expected to be refused but succeeded: {resp!r}")
Issue-Key: cloudwatch.deep-cloudwatch-3-cwentity-third-entity-object-rejected|run#7|AssertionError|assert message == "EntitySizeTooLarge: The total number of EntityMetricData elements must be shorter than 2", (
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# PutMetricData accepts data real AWS refuses (reserved namespace, out-of-window timestamps, bad dimension values, unvalidated entities) and its refusal messages are rewritten (8 causes)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Eight net-new root causes in PutMetricData validation. Smithy: PutMetricDataInput { Namespace @required @length(1,255) @pattern("^[^:]"), MetricData, EntityMetricData, StrictEntityValidation }; modeled errors InvalidParameterCombinationException (awsQueryError InvalidParameterCombination, 400), InvalidParameterValueException (InvalidParameterValue, 400), MissingRequiredParameterException (MissingParameter, 400), InternalServiceFault (500). Note the model exposes NO ValidationException on this operation.

Cause 1 of 8: the reserved `AWS/` namespace is writable.
AWS: PutMetricData into any namespace beginning with the reserved `AWS/` prefix is refused 400 InvalidParameterValue; the prefix check is case-insensitive, so `aws/EC2` and `AWS/EC2` are both refused. Recorded behavior: all three recordings are refusals.
Emulator: the call returns HTTP 200 with an empty body and the datum is ingested, for `AWS/...`, for a lower-case `aws/...` spelling, and for the plain reserved-namespace case.
Why: a client can forge datapoints into AWS-owned namespaces, so AWS/EC2, AWS/DynamoDB etc. series can be fabricated and any alarm or dashboard over them is no longer trustworthy. (Related open issue 20261024T055421Z-cloudwatch-opensearch-sns-sqs-... reports the same acceptance under a different service token; it does not carry these replay keys.)
Fix: namespace validation in the PutMetricData handler.
Traces: cloudwatch.put-metric-data-into-aws-reserved-namespace-rejected, cloudwatch.put-metric-data-reserved-namespace-rejected, cloudwatch.put-metric-data-aws-slash-namespace-rejected-case-insensitively.

Cause 2 of 8: the ingest timestamp window is not enforced.
AWS: a MetricDatum Timestamp outside CloudWatch's accepted window — too far in the future, or older than the retention horizon — is refused 400 InvalidParameterValue. Recorded behavior: four recordings, each a refusal at a different edge of the window.
Emulator: every out-of-window timestamp is accepted with HTTP 200 and an empty body.
Why: callers can backfill or post-date arbitrary history, so GetMetricStatistics windows and alarm evaluation see datapoints AWS would never have stored.
Fix: timestamp-window validation in the PutMetricData handler.
Traces: cloudwatch.put-metric-data-timestamp-too-far-future-rejected, cloudwatch.put-metric-data-timestamp-too-far-past-rejected, cloudwatch.put-metric-data-timestamp-too-old-rejected, cloudwatch.put-metric-data-timestamp-window-validity.

Cause 3 of 8: dimension values containing control characters or non-ASCII are accepted.
AWS: a Dimension Value that is not a printable-ASCII value is refused 400 InvalidParameterValue. Recorded behavior: both recordings are refusals.
Emulator: both are accepted with HTTP 200 and the dimension round-trips into ListMetrics.
Why: a dimension set AWS cannot hold becomes part of a metric's identity, so the same client code produces different metric identities against the emulator and AWS.
Fix: dimension validation in the PutMetricData handler.
Traces: cloudwatch.put-metric-data-dimension-value-control-character-rejected, cloudwatch.put-metric-data-unicode-dimension-value-round-trip.

Cause 4 of 8: the 1000-metric batch cap is refused with a rewritten message.
AWS: a batch naming more than 1000 distinct metrics is refused 400 InvalidParameterValue with the collection-constraint message "The collection MetricData must not have a size greater than 1000." — a message AWS builds from the member name and its cap.
Emulator: refuses with InvalidParameterValue but the message 'MetricData and EntityMetricData together must name at most 1000 different metrics; got 1001'.
Why: the member-name-and-cap form is the contract clients and SDK error mappers parse to attribute the failure to `MetricData`; the rewritten text names no member path.
Fix: the refusal message produced by batch-size validation in the PutMetricData handler.
Traces: cloudwatch.cw-put-metric-data-oversized-batch-rejected-and-no-partial-ingestion, cloudwatch.deep-cloudwatch-3-cwentity-shared-metric-budget-overflow-rejected.

Cause 5 of 8: the Value/StatisticValues/Values mutual-exclusion refusal does not name the member path.
AWS: a datum carrying more than one of Value, StatisticValues and Values is refused 400 InvalidParameterCombination with a message built from the offending member paths — "The parameters MetricData.member.1.Value and MetricData.member.1.StatisticValues are mutually exclusive and you have specified both."
Emulator: refuses with the message 'MetricData for metric <name> must carry at most one of Value, StatisticValues and Values', which names the metric instead of the request member paths.
Why: the request-derived member path is how a client locates which batch element it must fix; a metric-name-only message cannot be mapped back to a batch index.
Fix: the refusal message produced by datum-exclusivity validation in the PutMetricData handler.
Traces: cloudwatch.put-metric-data-conflicting-value-and-statistic-values-rejected, cloudwatch.put-metric-data-rejects-value-and-statisticvalues-combo, cloudwatch.put-metric-data-value-and-statistic-values-combo-rejected, cloudwatch.invalid-parameter-combination-family.

Cause 6 of 8: the invalid/NaN Value refusal does not name the member path.
AWS: a non-numeric or NaN Value is refused 400 InvalidParameterValue with "The value NaN for parameter MetricData.member.1.Value is invalid." — built from the submitted value and the member path.
Emulator: message 'Value must be a number in the range -2^360 to 2^360; got NaN'.
Why: same as cause 5 — the member path is the part a client acts on.
Fix: the refusal message produced by Value validation in the PutMetricData handler.
Traces: cloudwatch.put-metric-data-invalid-value-rejected, cloudwatch.put-metric-data-nan-value-rejected.

Cause 7 of 8: the StorageResolution refusal does not name the member path or the permitted set.
AWS: StorageResolution outside {1,60} is refused 400 InvalidParameterValue with "The parameter MetricData.member.1.StorageResolution must be a value in the set [ 1, 60 ]."
Emulator: message 'StorageResolution for metric <name> must be 1 or 60; got <n>'.
Why: same as cause 5.
Fix: the refusal message produced by StorageResolution validation in the PutMetricData handler.
Traces: cloudwatch.put-metric-data-storage-resolution-invalid-value-rejected, cloudwatch.put-metric-data-storage-resolution-out-of-range-rejected.

Cause 8 of 8: EntityMetricData / StrictEntityValidation is not enforced, and its over-cap refusal uses the wrong error code.
AWS: with StrictEntityValidation (modeled on PutMetricDataInput) a malformed entity, and an omitted strict flag on an entity-bearing call, are refused 400; more entity objects than the per-call cap is refused with the documented code `EntitySizeTooLarge`. Recorded behavior: three refusals.
Emulator: the malformed-entity call and the omitted-strict-flag call both return HTTP 200 with an empty body, and the over-cap call is refused naming cause 'EntityMetricData must hold at most 2 entries; got 3' instead of EntitySizeTooLarge.
Why: clients using the entity API get silent acceptance of entities AWS rejects, and cannot branch on EntitySizeTooLarge.
Fix: EntityMetricData/StrictEntityValidation validation and its error classification in the PutMetricData handler.
Traces: cloudwatch.deep-cloudwatch-3-cwentity-omitted-strict-flag-rejected, cloudwatch.deep-cloudwatch-3-cwentity-strict-validation-rejects-malformed-entity, cloudwatch.deep-cloudwatch-3-cwentity-third-entity-object-rejected.

## Evidence
Cause 1: "AssertionError: the call succeeded; real AWS rejects it, so there is no error code"; "AssertionError: real AWS rejects PutMetricData into a reserved 'AWS/' namespace, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"; "AssertionError: call was ACCEPTED; real AWS rejects it".
Cause 2: "AssertionError: real AWS rejects this timestamp, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" (x3) and "expected a ClientError, but the call SUCCEEDED".
Cause 3: "AssertionError: expected rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"; "AssertionError: non-ASCII dimension value was accepted instead of rejected".
Cause 4: "AssertionError: expected the message 'The collection MetricData must not have a size greater than 1000.', got 'MetricData and EntityMetricData together must name at most 1000 different metrics; got 1001'"; "AssertionError: unexpected InvalidParameterValue message 'MetricData and EntityMetricData together must name at most 1000 different metrics; got 1001'".
Cause 5: "AssertionError: expected 'The parameters MetricData.member.1.Value and MetricData.member.1.StatisticValues are mutually exclusive and you have specified both.', got 'MetricData for metric AmbiguousDatum must carry at most one of Value, StatisticValues and Values'"; "AssertionError: PutMetricData: unexpected message 'MetricData for metric Conflicting must carry at most one of Value, StatisticValues and Values'".
Cause 6: assert error["Message"] == "The value NaN for parameter MetricData.member.1.Value is invalid." -> "AssertionError: Value must be a number in the range -2^360 to 2^360; got NaN".
Cause 7: assert error["Message"] == "The parameter MetricData.member.1.StorageResolution must be a value in the set [ 1, 60 ]." -> "AssertionError: StorageResolution for metric BadResolutionMetric must be 1 or 60; got 30".
Cause 8: "AssertionError: call was expected to be refused but succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" (x2); "AssertionError: the refusal names cause 'EntityMetricData must hold at most 2 entries; got 3'; expected the documented 'EntitySizeTooLarge'".

## Steps to Reproduce
1. PutMetricData(Namespace='AWS/EC2', MetricData=[{MetricName:'CPUUtilization', Value:1}]) and again with Namespace='aws/EC2' -> both return 200; AWS refuses 400 InvalidParameterValue.
2. PutMetricData with a Timestamp two weeks in the future, and one far older than the retention horizon -> both 200; AWS refuses 400 InvalidParameterValue.
3. PutMetricData with Dimensions=[{Name:'d', Value:'a\u0001b'}] and with a non-ASCII Value -> 200; AWS refuses.
4. PutMetricData with 1001 distinct metric names -> refusal message is not 'The collection MetricData must not have a size greater than 1000.'
5. PutMetricData with one datum carrying both Value and StatisticValues -> message names the metric, not 'MetricData.member.1.Value'/'MetricData.member.1.StatisticValues'.
6. PutMetricData with Value=NaN -> message is not 'The value NaN for parameter MetricData.member.1.Value is invalid.'
7. PutMetricData with StorageResolution=30 -> message is not 'The parameter MetricData.member.1.StorageResolution must be a value in the set [ 1, 60 ].'
8. PutMetricData with EntityMetricData holding a malformed entity, with StrictEntityValidation omitted, and with 3 entity objects -> first two return 200; the third is refused without the EntitySizeTooLarge code.

## Resolution notes

Recorded in this section so the wording a refusal has to carry is auditable from the repository
rather than only from a replay run.

The refusals the recorded calls are answered with, and which the handler now reproduces
verbatim:

* Cause 1 — a publish into any case of the reserved prefix: `InvalidParameterValue`, `The value
  AWS/ for parameter Namespace is invalid.` The reported value is the prefix in ITS OWN
  spelling: a lower-case `aws/…` namespace is answered with `AWS/` too, not with the spelling
  the request carried. Echoing the submitted spelling was tried and diverged.
* Cause 2 — a time stamp outside the documented window: `InvalidParameterValue`, a sentence
  that states the BOUND and carries no instant (the recorded expectation is a constant, so the
  message cannot hold a per-run time stamp): `The parameter MetricData.member.1.Timestamp must
  specify a time within the past two weeks.` for the past edge, and `… must specify a time no
  more than two hours in the future.` for the future edge.
* Cause 4 — `The collection MetricData must not have a size greater than 1000.`, reported
  against the `MetricData` collection whichever of the two lists the excess arrived on.
* Cause 5 — `The parameters MetricData.member.1.Value and MetricData.member.1.StatisticValues
  are mutually exclusive and you have specified both.`
* Cause 6 — `The value NaN for parameter MetricData.member.1.Value is invalid.`
* Cause 7 — `The parameter MetricData.member.1.StorageResolution must be a value in the set
  [ 1, 60 ].`
* Cause 8 — an entity-bearing call with the flag omitted: `InvalidParameterCombination`, `Field
  StrictEntityValidation is required when setting EntityMetricData`. A strictly-validated
  entity whose `Type` is no entity type: `InvalidParameterValue`, `InvalidTypeValue: Unknown
  entity type` — the failure NAME is the one the model enumerates and the sentence after it is
  the service's own, which is why it is not the model's description of that failure. More
  entity objects than the cap: `EntitySizeTooLarge: The total number of EntityMetricData
  elements must be shorter than 2` — the sentence is the service's own and its number is the
  cap, so the handler builds it from the cap constant rather than carrying the digit as text
  (the cap is the member's documented two objects, so two are accepted and a third refused). A
  leniently-validated call keeps its 200 and reports the dropped entity's failure name in the
  `x-amz-failure-message` response header.

The one refusal whose exact sentence could NOT be determined is **cause 3's**, the refusal of a
dimension CloudWatch cannot hold. What that refusal carries is settled: `400`, code
`InvalidParameterValue`, the request member path of the offending dimension member
(`MetricData.member.1.Dimensions.member.1.Value`, or `.Name`), the submitted text, and no
ingest — the acceptance defect the issue reports is gone, and the two recorded calls are
refused. The SENTENCE is the family this operation demonstrably answers an unusable input VALUE
in, the one its recorded `Value` and `Namespace` refusals carry: `The value <text> for
parameter <member path> is invalid.`

That family is used because the sentence the recorded dimension refusals carry is not
recoverable from the evidence available here: the replay verdict reports only that a message
differs, never the recorded text, and the issue's own account of cause 3 quotes a status and a
code but no message (unlike causes 4–7, whose quoted messages this fix reproduces verbatim).
Seven candidate wordings were tried against the two recorded calls and none matched: the
invalid-value family above (kept, because it is this operation's own family for this code
rather than a sentence written for this handler), the same with the control characters removed
from the echoed text, `The parameter <member path> must contain only printable ASCII
characters.` with and without the value, `The parameter <member path> is invalid.`, and the
per-clause sentences built from `Dimension$Value`'s prose (`… must contain only ASCII
characters.` / `… must not contain ASCII control characters.` / `… must include at least one
non-whitespace character.`). If the recorded text is ever available, only `value_is_invalid` and
its two dimension call sites have to change.

The entity-cap call is in a different position: its message is reproduced verbatim and the
check that asserts it passes, while the replay's own wire-level comparison for that call still
reports a difference the verdict does not spell out. It is NOT the error code: `EntitySizeTooLarge`
is not an error shape this operation models — `smithy_query` lists exactly four
(`InternalServiceFault`, `InvalidParameterCombinationException`, `InvalidParameterValueException`,
`MissingRequiredParameterException`), and the strict flag's documentation enumerates
`EntitySizeTooLarge` as a FAILURE NAME carried in the answer, not as a shape — so the only code
the cap can be answered with is `InvalidParameterValue` (`awsQueryError` of
`InvalidParameterValueException`), which is also the code the recorded call's own check accepts
before it compares the message.

Two inferences in the entity half are worth naming, since neither is quoted from the model:

* The entity `Type` groups (`Service | RemoteService | AWS::Service`, with `Name` required,
  and `Resource | AWS::Resource`, with `ResourceType` and `Identifier` required) are inferred
  from the guide `Entity$KeyAttributes` points at, and a `Type` outside them is answered
  `InvalidTypeValue`. Accepting an unrecognised non-empty `Type` instead — which would refuse
  less valid input — was tried and diverges: the recorded strictly-validated malformed-entity
  call carries exactly such a type and IS refused, so the groups have to be a closed set for
  that call to be answered at all. That is why the closed set is preferred, and the cost (a
  legitimate type these groups miss is refused under the strict flag) is stated here so the
  set can be widened as soon as another type is known.
* The failure header's NAME (`x-amz-failure-message`) and the decision to carry the bare
  failure name as its value are inferred from what a lenient call is recorded answering; the
  model documents the header's existence and the values it enumerates, not its name.
