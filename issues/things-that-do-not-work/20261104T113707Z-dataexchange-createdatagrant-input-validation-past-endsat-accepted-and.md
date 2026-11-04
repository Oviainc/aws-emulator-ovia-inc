Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dataexchange.create-data-grant-ends-at-past-rejected dataexchange.create-data-grant-invalid-receiver-principal-rejected
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange.create-data-grant-ends-at-past-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: dataexchange.create-data-grant-invalid-receiver-principal-rejected|run#7|AssertionError|assert message == 'The following parameter(s) are not valid: "ReceiverPrincipal" with value "not-an-account-id" fails to match the AWS Account Id pattern.', f"e
Issue-Commit: 55cf77d522c1895c18e1a6aab58b83f0cfc3c7c5

# DataExchange CreateDataGrant input validation: past EndsAt accepted, and ReceiverPrincipal pattern refusal uses generic constraint wording instead of AWS's message

> _Note: this issue was identified against the code base at commit `55cf77d522c1895c18e1a6aab58b83f0cfc3c7c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataGrant

## Description
CreateDataGrant in services/service-com-amazonaws-dataexchange-dataexchange accepts a request real AWS refuses, and refuses another one with the wrong message. Two net-new root causes, both in this one operation's request validation. (Neither is covered by the open issue 20261029T215303Z-dataexchange-canceljob-and-startjob-client-errors-use-emulator.md, which is about CancelJob/StartJob prose, nor by the CancelJob/StartJob job-state issue 20261029T205114Z-...-conflictexception-refusal-wording.md.)

Cause 1 of 2: a past EndsAt is accepted and a grant is created.
Failing check: dataexchange.create-data-grant-ends-at-past-rejected
AWS: Smithy (com.amazonaws.dataexchange#CreateDataGrant) models EndsAt as Timestamp (timestampFormat date-time, optional) and declares ValidationException (HTTP 400) for this operation; the model states no temporal bound, so the bound itself is recorded behavior only. The recording shows AWS refusing an EndsAt that is not at least midnight UTC two calendar days from the current date, with HTTP 400, error code ValidationException, and the message: 'The following parameter(s) are not valid: "endDate" must be greater than or equal to "<EARLIEST>T00:00:00.000Z".' Fixed text: 'The following parameter(s) are not valid: "endDate" must be greater than or equal to "' ... 'T00:00:00.000Z".' — note AWS names the parameter 'endDate', not the wire member 'EndsAt'. Variable part: <EARLIEST> is the ISO date (YYYY-MM-DD) of today's UTC date plus two days, computed from the server clock, NOT a literal from any one recording. No grant is created by the refused call (ListDataGrants is unchanged).
Emulator: the handler performs no EndsAt check at all. The call returns HTTP 201 with a fully created grant whose EndsAt is in the past (AcceptanceState PENDING_RECEIVER_ACCEPTANCE), and the grant is inserted into state, so ListDataGrants grows.
Why: a client that relies on AWS rejecting an already-expired or too-soon expiry silently gets a grant that can never be usefully accepted, and its error-handling path for ValidationException is never exercised. Worse, the emulator mutates state for a request AWS leaves with no side effect, so subsequent list/count assertions diverge.
Fix: the CreateDataGrant handler's request validation — add the EndsAt lower-bound guard alongside the existing Description/Tags bounds, before any id is minted or anything is inserted, returning the modeled ValidationException with the message above.
Traces: dataexchange.create-data-grant-ends-at-past-rejected.

Cause 2 of 2: ReceiverPrincipal pattern refusal carries the generic boundary-validator message.
Failing check: dataexchange.create-data-grant-invalid-receiver-principal-rejected
AWS: Smithy models CreateDataGrant.ReceiverPrincipal as required with @pattern ^\\d{12}$ and declares ValidationException (HTTP 400). The recording shows AWS refusing a non-account-id value with HTTP 400, code ValidationException, and the message: 'The following parameter(s) are not valid: "ReceiverPrincipal" with value "<VALUE>" fails to match the AWS Account Id pattern.' Fixed text: 'The following parameter(s) are not valid: "' + member name + '" with value "' + value + '" fails to match the AWS Account Id pattern.' Variable parts: the member name as modeled ('ReceiverPrincipal') and the offending value taken verbatim from the request. The refused call creates no grant.
Emulator: the status and code are right (400 / ValidationException) but the message is the framework's generic constraint prose: "1 validation error detected: Value 'not-an-account-id' at 'receiverPrincipal' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$" — it lowercases the member to 'receiverPrincipal', leaks the raw regex, and uses a wholly different sentence from DataExchange's.
Why: clients and tests that match on the refusal text (or surface it to users) see a message DataExchange never emits; anything keying off the AWS wording, or off the member name as modeled, fails against the emulator while passing against AWS.
Fix: the CreateDataGrant pattern-violation path — the ReceiverPrincipal constraint refusal must be produced with DataExchange's message rather than delegated to the shared boundary validator's generic constraint-violation serializer.
Traces: dataexchange.create-data-grant-invalid-receiver-principal-rejected.

## Evidence
Cause 1 — expected: ClientError ValidationException (HTTP 400), message 'The following parameter(s) are not valid: "endDate" must be greater than or equal to "<today+2d>T00:00:00.000Z".', and ListDataGrants unchanged.
Actual (dataexchange.create-data-grant-ends-at-past-rejected): the call SUCCEEDED — HTTPStatusCode 201 with a created grant: {'Name': 'sim2real-dx-grant-past-...', 'SenderPrincipal': '000000000000', 'ReceiverPrincipal': '111122223333', 'AcceptanceState': 'PENDING_RECEIVER_ACCEPTANCE', 'EndsAt': <yesterday>, 'GrantDistributionScope': 'NONE', 'DataSetId': ..., 'SourceDataSetId': ..., 'Id': ..., 'Arn': 'arn:aws:dataexchange:<region>:<acct>:data-grants/<id>', 'CreatedAt': ..., 'UpdatedAt': ...}. Harness raised: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {...}. Handler source confirms no EndsAt guard: services/service-com-amazonaws-dataexchange-dataexchange/src/ops/create_data_grant.rs lines 186-197 bound only Description and Tags; input.ends_at is copied straight into the stored record at line 237.

Cause 2 — expected message: 'The following parameter(s) are not valid: "ReceiverPrincipal" with value "not-an-account-id" fails to match the AWS Account Id pattern.'
Actual (dataexchange.create-data-grant-invalid-receiver-principal-rejected): code ValidationException (correct) but message "1 validation error detected: Value 'not-an-account-id' at 'receiverPrincipal' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$". Handler source comment at create_data_grant.rs lines 181-185 states ReceiverPrincipal is left to "the boundary validator", which is the source of the generic wording.

## Steps to Reproduce
1. Build and boot the emulator, point a botocore dataexchange client at it.
2. Create a data set (CreateDataSet AssetType=S3_SNAPSHOT), a revision, an asset, and finalize the revision, so a valid SourceDataSetId exists.
3. Cause 1: call CreateDataGrant with Name=<any>, GrantDistributionScope='NONE', ReceiverPrincipal='111122223333', SourceDataSetId=<that data set id>, EndsAt=now(UTC)-1 day. Observe HTTP 201 and a created grant instead of HTTP 400 ValidationException 'The following parameter(s) are not valid: "endDate" must be greater than or equal to "<today+2d>T00:00:00.000Z".'; ListDataGrants now contains the grant that should not exist.
4. Cause 2: call CreateDataGrant with the same arguments but ReceiverPrincipal='not-an-account-id' and a future EndsAt. Observe ValidationException with the generic "1 validation error detected: ... failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$" message instead of 'The following parameter(s) are not valid: "ReceiverPrincipal" with value "not-an-account-id" fails to match the AWS Account Id pattern.'
5. Re-replay dataexchange.create-data-grant-ends-at-past-rejected and dataexchange.create-data-grant-invalid-receiver-principal-rejected to confirm a fix.
