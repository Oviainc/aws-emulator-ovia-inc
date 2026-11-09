Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dataexchange.create-data-grant-ends-at-past-rejected dataexchange.create-data-grant-invalid-receiver-principal-rejected
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange.create-data-grant-ends-at-past-rejected|_error#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: dataexchange.create-data-grant-invalid-receiver-principal-rejected|run#6|AssertionError|assert message == 'The following parameter(s) are not valid: "ReceiverPrincipal" with value "not-an-account-id" fails to match the AWS Account Id pattern.', f"e
Issue-Commit: 00353ccf65811f65ad7f73dbcefa75ea121211d3

# DataExchange CreateDataGrant request validation: a past EndsAt is accepted and persisted, and the ReceiverPrincipal pattern refusal uses generic constraint prose instead of DataExchange's message

> _Note: this issue was identified against the code base at commit `00353ccf65811f65ad7f73dbcefa75ea121211d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataGrant

## Description
Two net-new root causes, both in CreateDataGrant's request validation. NOTE: the open issue 20261104T113707Z-dataexchange-createdatagrant-input-validation-past-endsat-accepted-and.md describes the same two checks and was not marked as owning them by this replay, so if that issue is still being worked these two causes should be deduplicated against it rather than fixed twice.

Cause 1 of 2: a past EndsAt is accepted and a data grant is created.
Failing check: dataexchange.create-data-grant-ends-at-past-rejected
AWS: Smithy (com.amazonaws.dataexchange#CreateDataGrant) models EndsAt as Timestamp (timestampFormat date-time, optional) and declares ValidationException (httpError 400) for the operation; POST /v1/data-grants, success code 201. The model states no temporal lower bound, so the bound itself is recorded behavior only: the recording shows AWS refusing an EndsAt earlier than midnight UTC two calendar days from the current UTC date with HTTP 400, code ValidationException and message 'The following parameter(s) are not valid: "endDate" must be greater than or equal to "<EARLIEST>T00:00:00.000Z".'. Fixed text: 'The following parameter(s) are not valid: "endDate" must be greater than or equal to "' ... 'T00:00:00.000Z".' — AWS names the parameter 'endDate', not the wire member 'EndsAt'. Variable part: <EARLIEST> is the ISO calendar date (YYYY-MM-DD) of the server's current UTC date plus two days; it is computed from the clock, never a literal from any one run. The refused call has no side effect — ListDataGrants is unchanged.
Emulator: no EndsAt check at all. The call returns HTTP 201 with a fully formed grant (AcceptanceState PENDING_RECEIVER_ACCEPTANCE, EndsAt the past timestamp from the request, Id/Arn minted, CreatedAt/UpdatedAt set) and the grant is inserted into state, so it is then visible to ListDataGrants/GetDataGrant.
Why: a client relying on AWS rejecting an already-expired or too-soon expiry gets a grant that can never usefully be accepted, and its ValidationException path is never exercised; worse, the emulator mutates state for a request AWS leaves with no side effect, so any subsequent list/count assertion diverges.
Fix: the CreateDataGrant handler's request validation — add the EndsAt lower-bound guard before any id is minted or anything is inserted, and return the modeled ValidationException (HTTP 400) with the message above.
Traces: dataexchange.create-data-grant-ends-at-past-rejected.

Cause 2 of 2: the ReceiverPrincipal pattern refusal carries the shared boundary validator's generic prose.
Failing check: dataexchange.create-data-grant-invalid-receiver-principal-rejected
AWS: Smithy models CreateDataGrant.ReceiverPrincipal as required with @pattern ^\\d{12}$ and declares ValidationException (httpError 400). The recording shows AWS refusing a non-account-id value with HTTP 400, code ValidationException and message 'The following parameter(s) are not valid: "ReceiverPrincipal" with value "<VALUE>" fails to match the AWS Account Id pattern.'. Fixed text: 'The following parameter(s) are not valid: "' + member name + '" with value "' + value + '" fails to match the AWS Account Id pattern.'. Variable parts: the member name exactly as modeled ('ReceiverPrincipal') and the offending value copied verbatim from the request. The refused call creates no grant.
Emulator: status and code are right (400 / ValidationException) but the message is the framework's generic constraint prose: "1 validation error detected: Value 'not-an-account-id' at 'receiverPrincipal' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$" — it lower-cases the member name, leaks the raw regex, and uses a sentence DataExchange never emits.
Why: clients and tests that match on or display the refusal text see a message DataExchange never produces; anything keying off AWS's wording, or off the member name as modeled, passes against AWS and fails against the emulator.
Fix: the CreateDataGrant pattern-violation path — produce the ReceiverPrincipal refusal with DataExchange's own message instead of delegating to the shared boundary validator's generic constraint-violation serializer.
Traces: dataexchange.create-data-grant-invalid-receiver-principal-rejected.

## Evidence
Cause 1 — expected: ClientError ValidationException (HTTP 400) with message 'The following parameter(s) are not valid: "endDate" must be greater than or equal to "<today+2d>T00:00:00.000Z".' and ListDataGrants unchanged.
Actual (dataexchange.create-data-grant-ends-at-past-rejected): AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'Name': 'sim2real-dx-grant-past-...', 'SenderPrincipal': '000000000000', 'ReceiverPrincipal': '111122223333', 'AcceptanceState': 'PENDING_RECEIVER_ACCEPTANCE', 'EndsAt': <yesterday, from the request>, 'GrantDistributionScope': 'NONE', 'DataSetId': <id>, 'SourceDataSetId': <id>, 'Id': <id>, 'Arn': 'arn:aws:dataexchange:<region>:<account>:data-grants/<id>', 'CreatedAt': <now>, 'UpdatedAt': <now>} (ids, names and timestamps are per-run values, not requirements).

Cause 2 — expected message: 'The following parameter(s) are not valid: "ReceiverPrincipal" with value "not-an-account-id" fails to match the AWS Account Id pattern.'
Actual (dataexchange.create-data-grant-invalid-receiver-principal-rejected): code ValidationException (correct), message "1 validation error detected: Value 'not-an-account-id' at 'receiverPrincipal' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$".

## Steps to Reproduce
1. Build and boot the emulator; point a botocore dataexchange client at it.
2. Create a data set (CreateDataSet AssetType=S3_SNAPSHOT), a revision, an asset, and finalize the revision so a valid SourceDataSetId exists.
3. Cause 1: CreateDataGrant with Name=<unique>, GrantDistributionScope='NONE', ReceiverPrincipal='111122223333', SourceDataSetId=<that data set id>, EndsAt=now(UTC)-1 day. Observe HTTP 201 and a created grant (also now listed by ListDataGrants) instead of HTTP 400 ValidationException 'The following parameter(s) are not valid: "endDate" must be greater than or equal to "<today+2d UTC>T00:00:00.000Z".' with no side effect.
4. Cause 2: the same call with ReceiverPrincipal='not-an-account-id' and a future EndsAt. Observe ValidationException carrying "1 validation error detected: Value 'not-an-account-id' at 'receiverPrincipal' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$" instead of 'The following parameter(s) are not valid: "ReceiverPrincipal" with value "not-an-account-id" fails to match the AWS Account Id pattern.'
5. Re-replay dataexchange.create-data-grant-ends-at-past-rejected and dataexchange.create-data-grant-invalid-receiver-principal-rejected to confirm a fix.
