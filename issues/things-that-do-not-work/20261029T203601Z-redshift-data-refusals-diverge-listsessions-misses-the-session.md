Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-data-account-level-listings
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-data-account-level-listings redshiftdata-batch-execute-statement-sqls-bounds redshiftdata-list-sessions-mutual-exclusion-filters redshiftdata-required-and-range-parameter-validation redshiftdata-target-selection-validation
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data|ListSessions|4xx->2xx|$.Message
Issue-Key: redshift-data|ListSessions|4xx->2xx|$.__type
Issue-Key: redshift-data|ListSessions|4xx->2xx|status
Issue-Key: redshift-data|BatchExecuteStatement|4xx->4xx|$.message
Issue-Key: redshift-data|ListSessions|4xx->4xx|$.Message
Issue-Key: redshift-data|ExecuteStatement|4xx->4xx|$.message
Issue-Key: redshift-data|ListDatabases|4xx->4xx|$.Message
Issue-Commit: f7a4f905f306aa572c5614e7cbc89e431b85714d

# redshift-data refusals diverge: ListSessions misses the session-not-found error and four validation messages are emulator-authored (5 causes)

> _Note: this issue was identified against the code base at commit `f7a4f905f306aa572c5614e7cbc89e431b85714d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSessions

## Description
Replay of the simparity-wire-aws-v1 wire corpus against the emulator produced five net-new redshift-data divergences, all in the Data API's REFUSAL surface: one request AWS refuses is answered 200, and four refusals carry emulator-authored message text where AWS's wording is fixed. Model facts below are from services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json (awsJson1_1). This bundle carries FIVE causes; all five must be closed before submitting.

Related open issue (behavioral corpus, does NOT cover these wire steps): 20261022T180846Z-redshift-data-data-api-surface-diverges-resultformat-echoed.md, whose Cause 3 recorded ListSessions as a NotImplemented stub. ListSessions now answers, so Causes 1 and 3 here are about what it answers, not about it being unimplemented.

=== Cause 1 of 5: ListSessions with a SessionId that does not exist returns an empty 200 instead of refusing the read ===
AWS: Smithy: ListSessions models ResourceNotFoundException (client), ValidationException and InternalServerException; its input carries SessionId (target UUID, pattern ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$) and its output Sessions is @required. Recorded behavior: when SessionId is syntactically valid but names no session in the account, AWS REFUSES the listing — HTTP 400 with __type 'ResourceNotFoundException' and the fixed message 'Session not found.' (the whole message is fixed text; it interpolates nothing from the request). It does not return an empty page.
Emulator: HTTP 200 with a body carrying neither __type nor Message — the unknown session id is treated as a filter that simply matches nothing.
Why: 'this session does not exist' and 'this session exists and has nothing to report' are different answers. A client that resolves a session id before reusing it (the normal pattern for SessionKeepAliveSeconds-backed sessions) concludes the id is live and goes on to submit statements against it; a caller that handles ResourceNotFoundException to re-create the session never takes that branch, and any negative test asserting the refusal passes vacuously.
Fix: the ListSessions handler — when SessionId is supplied, resolve it against session state and raise the not-found error (status and __type as recorded, fixed message 'Session not found.') instead of degrading an unresolved id to an empty filter result.
Traces: redshift-data-account-level-listings#task#2.

=== Cause 2 of 5: BatchExecuteStatement's Sqls length refusal drops the rejected value from the message ===
AWS: Smithy: BatchExecuteStatementInput.Sqls is @required, target SqlList with @length(min 1, max 40). Recorded behavior: a request violating that bound is refused HTTP 400 with the AWS constraint-report message "1 validation error detected: Value '<rendered value>' at '<wire member name>' failed to satisfy constraint: Member must have length greater than or equal to 1". Fixed text: '1 validation error detected: Value ', " at ", ' failed to satisfy constraint: Member must have length greater than or equal to ', each in that order. Variable parts taken from the request and the model: the rendered rejected value in single quotes (an empty list renders as []), the wire member name ('sqls'), and the bound from the member's @length.
Emulator: same 400 and same error type, but the message omits the 'Value <value>' segment entirely — it emits "1 validation error detected: Value at 'sqls' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: AWS's constraint report is parsed and surfaced by SDK consumers and by users debugging a rejected batch; the rejected value is the part that identifies WHICH input was bad. A message missing it is not the message AWS sends, and any assertion or log scrape keyed on the AWS form fails against the emulator.
Fix: the shared request-validation message formatter used by redshift-data's input validation — always interpolate the rejected value, rendered as AWS renders it, between 'Value ' and " at '<member>'".
Traces: redshiftdata-batch-execute-statement-sqls-bounds#task#1 (cluster also covers task#2 of the same fixture).

=== Cause 3 of 5: ListSessions' ClusterIdentifier/WorkgroupName mutual-exclusion refusal is emulator-authored prose ===
AWS: Smithy: ListSessionsRequest carries both ClusterIdentifier and WorkgroupName and models ValidationException; the model does not state the exclusion, so this is recorded behavior only. Recorded: supplying both is refused HTTP 400 with the fixed message 'Only one of the following can be specified: ClusterIdentifier, WorkgroupName.' — fixed text including the trailing period, with the member list being exactly the mutually exclusive target selectors ListSessions models.
Emulator: same 400 and type, but the message is its own sentence: 'ClusterIdentifier and WorkgroupName cannot both be supplied: a session runs on either a provisioned cluster or a serverless workgroup'.
Why: the error message is part of the contract. A client branching on, asserting, or displaying AWS's wording sees text AWS never sends, and the two different sentences for the same condition make emulator and AWS runs non-interchangeable.
Fix: the ListSessions input validation — emit AWS's fixed mutual-exclusion sentence for the target-selector conflict rather than an emulator-authored explanation.
Traces: redshiftdata-list-sessions-mutual-exclusion-filters#task#5.

=== Cause 4 of 5: ExecuteStatement's string-length refusal message diverges from AWS's constraint report ===
AWS: Smithy: ExecuteStatementInput's length-constrained string members are ClusterIdentifier @length(1,63), WorkgroupName @length(3,128), ClientToken @length(1,64) and StatementName @length(max 2048); over-long input is refused with ValidationException (client, 400). Recorded behavior: the refusal is HTTP 400 carrying the same constraint-report form as Cause 2 — fixed text '1 validation error detected: Value ', " at ", ' failed to satisfy constraint: Member must have length ...' — and the rejected string is interpolated VERBATIM in single quotes, however long it is (the recording's value is a multi-thousand-character run of a single character, i.e. the submitted value one character past the member's max; that exact string is a value from the request, not part of the template).
Emulator: HTTP 400 with the matching error type, but a $.message that does not equal AWS's: the replay reports the divergence on $.message alone. Hypothesis: the same formatter defect as Cause 2 — the 'Value <rejected value>' segment is dropped — since both operations share redshift-data's validation message builder and Cause 2 shows exactly that omission.
Why: same consequence as Cause 2, on the Data API's single most used operation: a caller cannot tell from the emulator's message which value was rejected, and message-level assertions written against AWS fail.
Fix: the ExecuteStatement input validation / shared validation message formatter — render the full AWS constraint report, interpolating the rejected value untruncated and the member's own modeled bound.
Traces: redshiftdata-required-and-range-parameter-validation#task#7.

=== Cause 5 of 5: ListDatabases' target-selection refusal lists selectors the operation does not model ===
AWS: Smithy: ListDatabasesRequest models exactly ClusterIdentifier, Database, SecretArn, DbUser, NextToken, MaxResults and WorkgroupName — there is no Glue or SessionId selector on this operation — and it models ValidationException. The exclusion wording is recorded behavior only. Recorded: supplying more than one target selector is refused HTTP 400 with the fixed message 'There should be only one of the following: ClusterIdentifier, WorkgroupName' (no trailing period), the member list naming only the selectors THIS operation accepts.
Emulator: same 400 and type, but the message appends selectors the operation does not model: 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'.
Why: the message tells a caller which inputs it must choose between. Naming 'Glue resource' and 'SessionId' for an operation whose input shape has neither sends the caller after parameters it cannot pass, and the string differs from AWS's for an identical request.
Fix: the ListDatabases input validation — build the selector list from the operation's own modeled target selectors and emit AWS's fixed sentence verbatim (one shared template per operation, not one global list of every Data API selector).
Traces: redshiftdata-target-selection-validation#task#4.

## Evidence
Replay of simparity-wire-aws-v1: emulator_up true, bootstrap_signal false, 59 steps / 48 passed / 11 mismatched, score 81.4%, 5 divergence clusters — all five are redshift-data and all five are reported here (no stub_path, no owned_by on any).

Cause 1 — fixture redshift-data-account-level-listings, step task#2, ListSessions:
  expected: HTTP 400, $.__type 'ResourceNotFoundException', $.Message 'Session not found.'
  actual:   HTTP 200, no $.__type, no $.Message

Cause 2 — fixture redshiftdata-batch-execute-statement-sqls-bounds, steps task#1 and task#2, BatchExecuteStatement (both 400):
  expected $.message: "1 validation error detected: Value '[]' at 'sqls' failed to satisfy constraint: Member must have length greater than or equal to 1"
  actual   $.message: "1 validation error detected: Value at 'sqls' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 3 — fixture redshiftdata-list-sessions-mutual-exclusion-filters, step task#5, ListSessions (both 400):
  expected $.Message: 'Only one of the following can be specified: ClusterIdentifier, WorkgroupName.'
  actual   $.Message: 'ClusterIdentifier and WorkgroupName cannot both be supplied: a session runs on either a provisioned cluster or a serverless workgroup'

Cause 4 — fixture redshiftdata-required-and-range-parameter-validation, step task#7, ExecuteStatement (both 400):
  expected $.message: "1 validation error detected: Value '<the submitted over-long string, reproduced verbatim>' at '<member>' failed to satisfy constraint: Member must have length ..."
  actual:  $.message value_mismatch — status and error type agree, the message text does not. The replayed expected value is a multi-thousand-character single-character run (a request value, not part of the template); the emulator's rendering of that same refusal differs, consistent with the Cause 2 omission of the 'Value <value>' segment.

Cause 5 — fixture redshiftdata-target-selection-validation, step task#4, ListDatabases (both 400):
  expected $.Message: 'There should be only one of the following: ClusterIdentifier, WorkgroupName'
  actual   $.Message: 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId'

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): ListSessions errors ValidationException / ResourceNotFoundException / InternalServerException, input SessionId target UUID, output Sessions @required; BatchExecuteStatementInput.Sqls @required @length(min 1, max 40); ExecuteStatementInput ClusterIdentifier @length(1,63), WorkgroupName @length(3,128), ClientToken @length(1,64), StatementName @length(max 2048); ListDatabasesRequest has no Glue or SessionId member.

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout, then replay the simparity-wire-aws-v1 wire corpus against it (replay_wire_corpus).
2. Cause 1: replay fixture redshift-data-account-level-listings and inspect step task#2 — ListSessions with a syntactically valid SessionId that names no session. Expect HTTP 400 / __type ResourceNotFoundException / Message 'Session not found.'; observe HTTP 200 with neither field.
3. Cause 2: replay fixture redshiftdata-batch-execute-statement-sqls-bounds, steps task#1 and task#2 — BatchExecuteStatement violating Sqls @length. Expect the AWS constraint report with the rejected value interpolated; observe the same sentence with the "Value '<value>'" segment missing.
4. Cause 3: replay fixture redshiftdata-list-sessions-mutual-exclusion-filters, step task#5 — ListSessions with both ClusterIdentifier and WorkgroupName. Expect 'Only one of the following can be specified: ClusterIdentifier, WorkgroupName.'; observe the emulator's own sentence.
5. Cause 4: replay fixture redshiftdata-required-and-range-parameter-validation, step task#7 — ExecuteStatement with a string member one character past its modeled @length max. Expect the AWS constraint report with the over-long value interpolated verbatim; observe a differing $.message at the same 400.
6. Cause 5: replay fixture redshiftdata-target-selection-validation, step task#4 — ListDatabases with conflicting target selectors. Expect 'There should be only one of the following: ClusterIdentifier, WorkgroupName'; observe the list extended with 'Glue resource, SessionId'.
7. Re-replay issue_trace#issue_step plus every fixture#step in trace_ids to confirm all five causes are closed.
