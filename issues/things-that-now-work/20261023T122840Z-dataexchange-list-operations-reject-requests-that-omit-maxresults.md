Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dataexchange.dataset-lifecycle-create-get-delete dataexchange.revision-list-and-delete-lifecycle dataexchange.list-data-sets-invalid-origin-filter-rejected dataexchange.list-data-set-revisions-invalid-next-token-rejected dataexchange.send-data-set-notification-error-semantics
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange|ListDataSets|ValidationException
Issue-Key: dataexchange|ListDataSetRevisions|ValidationException
Issue-Key: dataexchange.list-data-sets-invalid-origin-filter-rejected|run#6|AssertionError|assert message == 'The following parameter(s) are not valid: "origin" must be one of [OWNED, ENTITLED].', f"expected the exact refusal message, got {message!r}"
Issue-Key: dataexchange.list-data-set-revisions-invalid-next-token-rejected|run#7|AssertionError|assert message == "Invalid NextToken. Account Id does not match Caller's Account Id.", f"expected the exact refusal message, got {message!r}"
Issue-Key: dataexchange.send-data-set-notification-error-semantics|run#6|AssertionError|assert code_not_configured_message == "Data set not configured for AWS Marketplace Data Product.", (
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# DataExchange list operations reject requests that omit maxResults, and SendDataSetNotification cannot parse an ISO-8601 DataUpdatedAt

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDataSets

## Description
Two distinct root causes in the DataExchange emulator, both in request deserialization/validation. This issue bundles 2 causes; both must be closed.

=== Cause 1 of 2: optional `maxResults` is materialized as its Smithy default (0) and then range-validated, so every list call that omits maxResults is refused ===
AWS: Smithy model (services/service-com-amazonaws-dataexchange-dataexchange/smithy-model/com-amazonaws-dataexchange-dataexchange.json) — ListDataSets.MaxResults and ListDataSetRevisions.MaxResults both target shape `MaxResults` (integer, @range min 1 max 200, @default 0) bound as @httpQuery("maxResults"). The member is NOT @required. Real AWS treats an absent `maxResults` query parameter as "no caller-supplied page size" and serves the default page; it does not apply the modeled default 0 and then fail its own range check. ListDataSets / ListDataSetRevisions with no maxResults return HTTP 200 with the DataSets / Revisions list.
Emulator: any GET /v1/data-sets or GET /v1/data-sets/{DataSetId}/revisions without a `maxResults` query parameter is refused with HTTP 400 ValidationException, message (example) "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1". The same refusal fires on calls whose intended rejection is something else entirely: ListDataSets(Origin="NOT_A_REAL_ORIGIN") and ListDataSetRevisions(NextToken="not-a-real-token!!") both come back with the maxResults message instead of the Origin-enum / NextToken refusals AWS produces, so the emulator's Origin-enum and NextToken validation are masked and unverified.
Why: a client cannot list data sets or revisions at all unless it explicitly passes MaxResults — the plain `list_data_sets()` / `list_data_set_revisions(DataSetId=...)` call every SDK paginator and console flow makes fails with a 400. Rejections for genuinely bad input are also misattributed to maxResults, so error-handling logic keyed on the real cause misbehaves.
Fix: the shared default-application/constraint-validation step for @httpQuery integer members with a @default that falls outside their own @range — an absent optional query parameter must stay absent (None) and not be range-checked — plus the ListDataSets / ListDataSetRevisions handlers' pagination and Origin/NextToken validation, which must be exercised once the maxResults refusal no longer shadows them.
Traces: dataexchange.dataset-lifecycle-create-get-delete (ListDataSets), dataexchange.revision-list-and-delete-lifecycle (ListDataSetRevisions), dataexchange.list-data-sets-invalid-origin-filter-rejected, dataexchange.list-data-set-revisions-invalid-next-token-rejected.

=== Cause 2 of 2: SendDataSetNotification rejects a date-time timestamp in Details.DataUpdate.DataUpdatedAt as an unparsable integer ===
AWS: Smithy model — SendDataSetNotification (POST /v1/data-sets/{DataSetId}/notification, success 202) takes Details: NotificationDetails; NotificationDetails.DataUpdate -> DataUpdateRequestDetails.DataUpdatedAt targets shape `Timestamp` with @timestampFormat("date-time"), i.e. an ISO-8601 string on the wire. With a well-formed body, AWS then applies business validation: a data set that exists and is owned by the caller but is not part of an AWS Marketplace product is refused with ValidationException (HTTP 400), recorded message example "Data set not configured for AWS Marketplace Data Product.", and a well-formed but never-created DataSetId is refused with the distinct ResourceNotFoundException (HTTP 404), recorded message example "The requested resource was not found." (modeled errors for this operation include both).
Emulator: the request never gets past deserialization — it answers HTTP 400 ValidationException with message 'invalid timestamp: failed to parse int' for Details={"DataUpdate": {"DataUpdatedAt": "<ISO-8601 instant>"}}, i.e. the timestamp member is being read as epoch-seconds integer instead of the modeled date-time format. Because the body is rejected at parse time, neither the marketplace-not-configured ValidationException nor the not-found ResourceNotFoundException path is reachable.
Why: every SDK serializes this member as an ISO-8601 string per @timestampFormat("date-time"), so SendDataSetNotification is unusable: callers get a parse-shaped ValidationException instead of either success (202) or the modeled, distinguishable refusals, and cannot tell "not configured for Marketplace" (400) from "data set does not exist" (404).
Fix: the restJson1 timestamp deserializer for JSON-body members carrying @timestampFormat("date-time") (it must accept the modeled date-time form, not only epoch integers), and then SendDataSetNotification's handler validation, which must distinguish the unknown-data-set 404 from the not-configured-for-Marketplace 400.
Traces: dataexchange.send-data-set-notification-error-semantics.

## Evidence
Cause 1: dataexchange.dataset-lifecycle-create-get-delete — botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the ListDataSets operation: 1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1 (call made: dx.list_data_sets()). dataexchange.revision-list-and-delete-lifecycle — same ValidationException on ListDataSetRevisions for dx.list_data_set_revisions(DataSetId=...). dataexchange.list-data-sets-invalid-origin-filter-rejected — expected message 'The following parameter(s) are not valid: "origin" must be one of [OWNED, ENTITLED].', got "1 validation error detected: Value '0' at 'maxResults' ...". dataexchange.list-data-set-revisions-invalid-next-token-rejected — expected 'Invalid NextToken. Account Id does not match Caller's Account Id.', got the same maxResults message. Expected in all four: HTTP 200 with the list (or the Origin/NextToken-specific ValidationException).

Cause 2: dataexchange.send-data-set-notification-error-semantics — AssertionError: expected the exact not-configured-for-marketplace message, got 'invalid timestamp: failed to parse int'; the error code was ValidationException but produced by body parsing, with DataUpdatedAt sent as '%Y-%m-%dT%H:%M:%SZ'. Expected: ValidationException (400) for the existing-but-unpublished data set and ResourceNotFoundException (404) for a never-created DataSetId.

## Steps to Reproduce
Cause 1: against the emulator, (a) call dataexchange ListDataSets with no parameters -> 400 ValidationException about maxResults instead of 200 + DataSets; (b) CreateDataSet(AssetType=S3_SNAPSHOT, Name=...) then ListDataSetRevisions(DataSetId=<id>) with no MaxResults -> same 400; (c) ListDataSets(Origin="NOT_A_REAL_ORIGIN") -> the maxResults message rather than the Origin-enum refusal; (d) ListDataSetRevisions(DataSetId=<id>, NextToken="not-a-real-token!!") -> the maxResults message rather than the NextToken refusal.

Cause 2: CreateDataSet(AssetType=S3_SNAPSHOT, Name=...), then SendDataSetNotification(DataSetId=<id>, Type="DATA_UPDATE", Details={"DataUpdate": {"DataUpdatedAt": "2024-01-01T00:00:00Z"}}) -> 400 ValidationException 'invalid timestamp: failed to parse int'; repeat with DataSetId="c"*38 -> expect 404 ResourceNotFoundException.
