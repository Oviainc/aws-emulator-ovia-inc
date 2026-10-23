Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker-featurestore-runtime.put-record-nonexistent-feature-group-validation-rejected sagemaker-featurestore-runtime.get-record-nonexistent-feature-group-rejected sagemaker-featurestore-runtime.update-record-conflict-stale-event-time-rejected sagemaker-featurestore-runtime.update-record-missing-record-resource-not-found sagemaker-featurestore-runtime.update-record-ttl-without-event-time-rejected sagemaker-featurestore-runtime.batch-get-record-too-many-identifiers-rejected sagemaker-featurestore-runtime.delete-record-harddelete-stale-event-time-rejected
Issue-Shape-Id: com.amazonaws.sagemakerfeaturestoreruntime#AmazonSageMakerFeatureStoreRuntime
Issue-Key: sagemaker-featurestore-runtime.put-record-nonexistent-feature-group-validation-rejected|_error#5|AssertionError|raise AssertionError(f"expected PutRecord against a nonexistent FeatureGroupName to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker-featurestore-runtime.get-record-nonexistent-feature-group-rejected|run#10|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code!r}"
Issue-Key: sagemaker-featurestore-runtime.update-record-conflict-stale-event-time-rejected|run#9|AssertionError|assert code == "ConflictException", f"expected ConflictException for a stale EventTime, got {code!r}"
Issue-Key: sagemaker-featurestore-runtime.update-record-missing-record-resource-not-found|run#4|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound for a never-ingested record, got {code!r}"
Issue-Key: sagemaker-featurestore-runtime.update-record-ttl-without-event-time-rejected|run#8|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code!r}"
Issue-Key: sagemaker-featurestore-runtime.batch-get-record-too-many-identifiers-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker-featurestore-runtime.delete-record-harddelete-stale-event-time-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# SageMaker FeatureStore Runtime rejection semantics diverge: PutRecord accepts an unknown feature group, GetRecord mis-codes it, UpdateRecord has no error semantics, and two rejection messages diverge (4 causes)

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Four distinct root causes in the sagemaker-featurestore-runtime data plane, all grounded in failing behavioral checks of this service.

Cause 1 of 4: PutRecord against a FeatureGroupName that does not exist SUCCEEDS.
Failing check ids: sagemaker-featurestore-runtime.put-record-nonexistent-feature-group-validation-rejected
AWS: Smithy — PutRecord (restJson1, PUT /FeatureGroup/{FeatureGroupName}) declares NO ResourceNotFound error; the only resolution failure it can report is the service-common ValidationError (com.amazonaws.sagemakerfeaturestoreruntime#ValidationError, errorKind=client, httpError 400). Recorded behavior: PutRecord naming an unresolvable feature group is refused with error code ValidationError and HTTP 400.
Emulator: the call returns HTTP 200 with an empty body (ResponseMetadata HTTPStatusCode 200, content-length 0) — the record is accepted against a feature group that was never created.
Why: a typo'd or not-yet-created feature group becomes a silent data-loss path: the client believes ingestion succeeded, no error is ever surfaced, and the record is unreadable afterwards. Over-permissive acceptance also hides the resource-resolution bug class entirely from clients that rely on the 400 to fail fast.
Fix: feature-group resolution in PutRecord's handler/validation — resolve FeatureGroupName against feature-group state before accepting the record and raise ValidationError (HTTP 400) when it does not resolve.
Traces: sagemaker-featurestore-runtime.put-record-nonexistent-feature-group-validation-rejected (_error() raised "expected PutRecord against a nonexistent FeatureGroupName to be rejected, but it SUCCEEDED").

Cause 2 of 4: GetRecord against a FeatureGroupName that does not exist answers the wrong error class/status.
Failing check ids: sagemaker-featurestore-runtime.get-record-nonexistent-feature-group-rejected
AWS: Smithy — GetRecord declares ResourceNotFound (httpError 404) for a missing RECORD, while recorded behavior for an unresolvable FeatureGroupName on this operation is error code ValidationError with HTTP 400 (ValidationError is a service-common error on every operation of this service). The check documents ValidationError as the empirical wire code for an unresolvable FeatureGroupName.
Emulator: returns error code ResourceNotFound (HTTP 404), conflating "this feature group does not exist" with "this record does not exist".
Why: clients distinguish the two: a 400/ValidationException-class failure is a non-retryable request defect (bad name), whereas ResourceNotFound/404 on GetRecord is the normal, expected signal for an absent record and is routinely swallowed as a cache miss. Mis-coding it makes a configuration error look like an empty lookup.
Fix: the not-found classification in GetRecord's handler — reserve ResourceNotFound for an absent record within an existing feature group, and raise ValidationError (400) when the feature group itself does not resolve.
Traces: sagemaker-featurestore-runtime.get-record-nonexistent-feature-group-rejected (AssertionError: expected ValidationError, got 'ResourceNotFound').

Cause 3 of 4: UpdateRecord has no error semantics at all — it answers NotImplemented on every rejection path.
Failing check ids: sagemaker-featurestore-runtime.update-record-conflict-stale-event-time-rejected, sagemaker-featurestore-runtime.update-record-missing-record-resource-not-found, sagemaker-featurestore-runtime.update-record-ttl-without-event-time-rejected
AWS: Smithy — UpdateRecord (POST /FeatureGroup/{FeatureGroupName}/Record) declares ConflictException (httpError 409) and ResourceNotFound (httpError 404), plus the service-common ValidationError (400). ConflictException's modeled doc: "The service rejected the update because the provided EventTime is older than the record's current EventTime ... resubmit the request with an EventTime that is equal to or newer than the current value." So: a stale EventTime -> ConflictException/409 with the record left unchanged; an update to a never-ingested record -> ResourceNotFound/404; a TtlDuration supplied without an accompanying EventTime -> ValidationError/400.
Emulator: all three paths return error code 'NotImplemented' — the operation is an unimplemented scaffold stub (src/ops/update_record.rs). An issue filed from the checkout already names that operation and file as unimplemented (it is credited with blocking the UpdateRecord round-trip checks); this cause is recorded here so the three REJECTION contracts above are not lost when the stub is implemented, and it must be closed together with that stub issue.
Why: a client cannot implement optimistic, event-time-ordered updates at all: it cannot tell a conflict (retry with a fresher EventTime) from a missing record (fall back to PutRecord) from a malformed request, and botocore's generic NotImplemented is not in any modeled error set, so error handlers never match.
Fix: implement the rejection paths of UpdateRecord's handler/validation on top of the feature-store record state — EventTime comparison against the stored record (ConflictException), record existence (ResourceNotFound), and the TtlDuration-requires-EventTime input rule (ValidationError).
Traces: sagemaker-featurestore-runtime.update-record-conflict-stale-event-time-rejected (expected ConflictException, got 'NotImplemented'), sagemaker-featurestore-runtime.update-record-missing-record-resource-not-found (expected ResourceNotFound, got 'NotImplemented'), sagemaker-featurestore-runtime.update-record-ttl-without-event-time-rejected (expected ValidationError, got 'NotImplemented').

Cause 4 of 4: the two rejections whose error CODE already matches carry diverging messages, and one of them reveals a stricter acceptance boundary.
Failing check ids: sagemaker-featurestore-runtime.batch-get-record-too-many-identifiers-rejected, sagemaker-featurestore-runtime.delete-record-harddelete-stale-event-time-rejected
AWS: (a) Smithy — BatchGetRecordRequest.Identifiers targets BatchGetRecordIdentifiers with @length min 1, max 100 and is required; exceeding it is refused with ValidationException and the standard constraint-violation message BUILT FROM THE REQUEST, naming the member and the constraint: "1 validation error detected: Value '[<the submitted identifiers>]' at 'identifiers' failed to satisfy constraint: Member must have length less than or equal to 100". That construction rule (count of violations, offending value, member path 'identifiers', constraint text from the @length max) is the requirement, not the recorded characters. (b) DeleteRecord with DeletionMode=HardDelete and a stale EventTime: recorded behavior only — error code ValidationError (400) with the message "Validation Error: Delete event time must be same or after latest record event time.", i.e. AWS's stated rule admits an EventTime EQUAL to the record's.
Emulator: (a) the limit is enforced with the right code (ValidationException) but the message does not follow the constraint-violation construction above. (b) the HardDelete rejection message is "EventTime: the EventTime of a HardDelete must be later than the EventTime of the record it deletes, but '<submitted>' is not later than '<stored>'" — a differently constructed message that also states a STRICTLY-later rule. Hypothesis: the emulator's comparison is `>` where AWS's is `>=`, so a HardDelete whose EventTime exactly equals the stored record's is refused by the emulator and accepted by AWS; the failing checks only exercise a strictly-earlier EventTime, so only the message divergence is directly observed.
Why: clients and SDK-level validators parse the standard constraint-violation message to report which member and limit a request violated; a free-form message breaks that and makes a 100-item limit breach indistinguishable from any other validation failure. For the HardDelete boundary, a deletion submitted at exactly the record's EventTime — the natural value a client reads back from GetRecord — fails against the emulator while succeeding on AWS.
Fix: the validation/error-serialization layer for BatchGetRecord's Identifiers @length (emit the modeled constraint-violation message form), and the EventTime comparison in DeleteRecord's HardDelete path (accept equal, reject only strictly-older) with its message.
Traces: sagemaker-featurestore-runtime.batch-get-record-too-many-identifiers-rejected (message assertion at run():40, code ValidationException already matched), sagemaker-featurestore-runtime.delete-record-harddelete-stale-event-time-rejected (message assertion at run():42, code already matched).

## Evidence
Cause 1 — expected (AWS): ClientError, code ValidationError, HTTP 400. Actual (emulator): success — {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '0', ...}}}.

Cause 2 — expected (AWS): code ValidationError (HTTP 400). Actual (emulator): code 'ResourceNotFound' (HTTP 404). AssertionError: expected ValidationError, got 'ResourceNotFound'.

Cause 3 — expected (AWS): ConflictException (409) for a stale EventTime with the record unchanged; ResourceNotFound (404) for a never-ingested record; ValidationError (400) for TtlDuration without EventTime. Actual (emulator): code 'NotImplemented' on all three.

Cause 4a — expected (AWS): ValidationException whose message is "1 validation error detected: Value '[BatchGetRecordIdentifier(featureGroupName=<fg>, recordIdentifiersValueAsString=[id-0], featureNames=null), ... ]' at 'identifiers' failed to satisfy constraint: Member must have length less than or equal to 100" (101 identifiers submitted). Actual (emulator): correct code, message not in that form. Cause 4b — expected (AWS): ValidationError with "Validation Error: Delete event time must be same or after latest record event time.". Actual (emulator): ValidationError with "EventTime: the EventTime of a HardDelete must be later than the EventTime of the record it deletes, but '1792779113.57266' is not later than '1792779613.57266'" (values are examples; the rule difference is strictly-later vs same-or-after).

## Steps to Reproduce
Common setup for every cause: CreateFeatureGroup with OnlineStoreConfig.EnableOnlineStore=true, RecordIdentifierFeatureName=record_id, EventTimeFeatureName=event_time (Fractional), feature_a/b/c String; wait for FeatureGroupStatus=Created.

Cause 1: PutRecord with FeatureGroupName set to a name that was never created (Record = record_id + event_time). AWS raises ValidationError/400; the emulator returns 200.

Cause 2: PutRecord one record into the real feature group and read it back (control succeeds), then GetRecord with FeatureGroupName set to a never-created name. AWS raises ValidationError/400; the emulator raises ResourceNotFound/404.

Cause 3: PutRecord record_id=rec-conf at event_time=T with feature_a=original. (a) UpdateRecord the same record with event_time=T-1000 -> expect ConflictException/409 and feature_a still 'original'. (b) UpdateRecord a record identifier that was never ingested -> expect ResourceNotFound/404. (c) UpdateRecord supplying TtlDuration but no EventTime -> expect ValidationError/400. The emulator answers NotImplemented to all three.

Cause 4: (a) BatchGetRecord with 101 Identifiers entries for the same feature group -> expect ValidationException with the constraint-violation message naming 'identifiers' and the max of 100; then the same call with 1 identifier must still succeed. (b) PutRecord record_id=rec-hd-stale at event_time=T, then DeleteRecord with DeletionMode=HardDelete and EventTime=T-500 -> expect ValidationError with AWS's same-or-after message and the record intact; also try EventTime exactly T, which AWS's stated rule accepts and the emulator's strictly-later comparison refuses.
