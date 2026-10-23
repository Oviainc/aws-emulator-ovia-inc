Issue-Source: trace-sweep-divergence
Issue-Trace: logs-bearer-token-auth-and-aggregate-summaries
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-bearer-token-auth-and-aggregate-summaries logs-list-aggregate-log-group-summaries logs-list-aggregate-log-group-summaries-invalid-pattern logs-integration-lifecycle-error-paths logs-integration-setup-and-s3table-source-errors logs-put-integration-invalid-retention-error logs-put-integration-validation-error logs-put-integration-validation-errors logs-lookup-table-error-paths logs-lookup-table-invalid-name-error logs-lookup-table-invalid-name-validation logs-put-storage-tier-policy-invalid-parameter-error logs-put-storage-tier-policy-missing-required-field logs-storage-tier-policy-invalid-value logs-storage-tier-policy-invalid-value-error logs-storage-tier-policy-lifecycle logsval-describe-log-streams-limit-above-maximum logsval-describe-log-streams-name-and-identifier-exclusive logsval-describe-log-streams-name-or-identifier-required logsval-get-log-events-limit-above-maximum logsval-get-log-events-limit-below-minimum logsval-get-log-events-name-and-identifier-exclusive logsval-get-log-events-name-or-identifier-required
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|ListAggregateLogGroupSummaries|4xx->4xx|$.__type
Issue-Key: logs|ListAggregateLogGroupSummaries|4xx->4xx|$.message
Issue-Key: logs|PutIntegration|4xx->4xx|$.__type
Issue-Key: logs|PutIntegration|4xx->4xx|$.message
Issue-Key: logs|CreateLookupTable|4xx->4xx|$.__type
Issue-Key: logs|CreateLookupTable|4xx->4xx|$.message
Issue-Key: logs|PutStorageTierPolicy|4xx->4xx|$.__type
Issue-Key: logs|PutStorageTierPolicy|4xx->4xx|$.message
Issue-Key: logs|DescribeLogStreams|4xx->4xx|$.__type
Issue-Key: logs|DescribeLogStreams|4xx->4xx|$.message
Issue-Key: logs|GetLogEvents|4xx->4xx|$.__type
Issue-Key: logs|GetLogEvents|4xx->4xx|$.message
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# CloudWatch Logs validation refusals answer ValidationException instead of InvalidParameterException

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAggregateLogGroupSummaries

## Description
The emulator's shared constraint-validation layer raises ValidationException for every input-constraint violation in CloudWatch Logs. The recordings show the service raising InvalidParameterException on these operations, and for three of them ValidationException is not even a modeled error of the operation. The HTTP status is 400 in both cases, so the only client-visible difference is the error code -- which is exactly what an SDK dispatches on. This bundle has 6 causes; all 6 must be closed.

Cause 1 of 6 -- ListAggregateLogGroupSummaries, invalid `groupBy` enum value.
AWS: Smithy -- ListAggregateLogGroupSummaries.groupBy targets enum ListAggregateLogGroupSummariesGroupBy (required); the operation models both InvalidParameterException and ValidationException (both client, httpError 400). The recorded response is 400 with __type InvalidParameterException and a request-derived message naming the member and the permitted enum set.
Emulator: 400 with __type ValidationException.
Why: a client catching InvalidParameterException (the code AWS raises, and the one the SDK's error mapper binds for this failure) sees an unmatched error and falls through to a generic service-exception branch, so enum-typo handling and retry classification both break.
Fix: the error classification in the ListAggregateLogGroupSummaries input validation -- the shared validator must select the operation's recorded refusal code rather than always emitting ValidationException.
Traces: logs-bearer-token-auth-and-aggregate-summaries#task#8 (also logs-list-aggregate-log-group-summaries#task#3, logs-list-aggregate-log-group-summaries-invalid-pattern#task#1).

Cause 2 of 6 -- PutIntegration, invalid `integrationType` enum value.
AWS: Smithy -- PutIntegration.integrationType targets enum IntegrationType (required, single value OPENSEARCH); errors InvalidParameterException and ValidationException. Recorded: 400, __type InvalidParameterException.
Emulator: 400, __type ValidationException.
Why: as cause 1 -- the integration-setup error path is unrecognisable to a client.
Fix: PutIntegration input validation / error classification.
Traces: logs-integration-lifecycle-error-paths#task#3 (also logs-integration-setup-and-s3table-source-errors#task#1, logs-put-integration-invalid-retention-error#task#1, logs-put-integration-validation-error#task#1, logs-put-integration-validation-errors#task#1).

Cause 3 of 6 -- CreateLookupTable, `lookupTableName` pattern violation.
AWS: Smithy -- CreateLookupTable.lookupTableName @pattern ^[a-zA-Z0-9_]+$, @length 1..256, required; errors include InvalidParameterException and ValidationException. Recorded: 400, __type InvalidParameterException.
Emulator: 400, __type ValidationException.
Why: as cause 1.
Fix: CreateLookupTable input validation / error classification.
Traces: logs-lookup-table-error-paths#task#3 (also logs-lookup-table-invalid-name-error#task#1, logs-lookup-table-invalid-name-validation#task#1).

Cause 4 of 6 -- PutStorageTierPolicy, invalid `storageTier` enum value.
AWS: Smithy -- PutStorageTierPolicy.storageTier targets enum StorageTier (required); the operation's modeled errors are AccessDeniedException, InvalidParameterException, OperationAbortedException, ResourceNotFoundException, ServiceUnavailableException -- ValidationException is NOT modeled on this operation. Recorded: 400, __type InvalidParameterException.
Emulator: 400, __type ValidationException -- an error shape the operation cannot return.
Why: the emulator returns an error outside the operation's modeled error set, so a code-generated client has no branch for it at all.
Fix: PutStorageTierPolicy input validation / error classification.
Traces: logs-put-storage-tier-policy-invalid-parameter-error#task#2 (also logs-put-storage-tier-policy-missing-required-field#task#3, logs-storage-tier-policy-invalid-value#task#3, logs-storage-tier-policy-invalid-value-error#task#2, logs-storage-tier-policy-lifecycle#task#3).

Cause 5 of 6 -- DescribeLogStreams, `limit` above the modeled maximum.
AWS: Smithy -- DescribeLogStreams.limit targets DescribeLimit @range min 1 max 50; the operation's modeled errors are InvalidParameterException, ResourceNotFoundException, ServiceUnavailableException -- ValidationException is NOT modeled. Recorded: 400, __type InvalidParameterException.
Emulator: 400, __type ValidationException (unmodeled for this operation).
Why: as cause 4.
Fix: DescribeLogStreams input validation / error classification.
Traces: logsval-describe-log-streams-limit-above-maximum#task#4 (also logsval-describe-log-streams-name-and-identifier-exclusive#task#4, logsval-describe-log-streams-name-or-identifier-required#task#4).

Cause 6 of 6 -- GetLogEvents, `limit` above the modeled maximum.
AWS: Smithy -- GetLogEvents.limit targets EventsLimit @range min 1 max 10000; modeled errors are InvalidParameterException, ResourceNotFoundException, ServiceUnavailableException -- ValidationException is NOT modeled. Recorded: 400, __type InvalidParameterException.
Emulator: 400, __type ValidationException (unmodeled for this operation).
Why: as cause 4; GetLogEvents is a hot read path, so every paging client that guards against over-large limits mishandles the refusal.
Fix: GetLogEvents input validation / error classification.
Traces: logsval-get-log-events-limit-above-maximum#task#4 (also logsval-get-log-events-limit-below-minimum#task#4, logsval-get-log-events-name-and-identifier-exclusive#task#4, logsval-get-log-events-name-or-identifier-required#task#4).

## Evidence
Expected vs actual, per cause (status 400 on both sides throughout):
- ListAggregateLogGroupSummaries / groupBy: $.__type expected 'InvalidParameterException', got 'ValidationException'.
- PutIntegration / integrationType: $.__type expected 'InvalidParameterException', got 'ValidationException'.
- CreateLookupTable / lookupTableName: $.__type expected 'InvalidParameterException', got 'ValidationException'.
- PutStorageTierPolicy / storageTier: $.__type expected 'InvalidParameterException', got 'ValidationException' (ValidationException is not in the operation's modeled error set).
- DescribeLogStreams / limit=51: $.__type expected 'InvalidParameterException', got 'ValidationException' (not modeled on the operation).
- GetLogEvents / limit=10001: $.__type expected 'InvalidParameterException', got 'ValidationException' (not modeled on the operation).
Smithy citations: ListAggregateLogGroupSummaries errors [InvalidParameterException, ServiceUnavailableException, ValidationException]; PutStorageTierPolicy errors [AccessDeniedException, InvalidParameterException, OperationAbortedException, ResourceNotFoundException, ServiceUnavailableException]; DescribeLogStreams errors [InvalidParameterException, ResourceNotFoundException, ServiceUnavailableException]; GetLogEvents errors [InvalidParameterException, ResourceNotFoundException, ServiceUnavailableException]; DescribeLimit @range 1..50; EventsLimit @range 1..10000.
The message text also diverges on these steps; that divergence is filed separately as the validation-message-rendering issue.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay logs-bearer-token-auth-and-aggregate-summaries step task#8 (ListAggregateLogGroupSummaries with groupBy='NOT_A_REAL_ENUM') and read $.__type from the 400 body.
3. Replay logs-integration-lifecycle-error-paths#task#3, logs-lookup-table-error-paths#task#3, logs-put-storage-tier-policy-invalid-parameter-error#task#2, logsval-describe-log-streams-limit-above-maximum#task#4 and logsval-get-log-events-limit-above-maximum#task#4 and read $.__type on each.
4. Each returns 400 with __type 'ValidationException'; the recorded responses carry __type 'InvalidParameterException'.
