Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptunedata.create-ml-endpoint-nonexistent-training-job-rejected neptunedata.gremlin-query-malformed-queryid-rejected-by-status-and-cancel neptunedata.opencypher-query-malformed-queryid-rejected-by-status-and-cancel neptunedata.gremlin-malformed-query-rejected neptunedata.opencypher-malformed-query-rejected
Issue-Shape-Id: com.amazonaws.neptunedata#AmazonNeptuneDataplane
Issue-Key: neptunedata.create-ml-endpoint-nonexistent-training-job-rejected|run#6|AssertionError|assert code == "MLResourceNotFoundException", code
Issue-Key: neptunedata.gremlin-query-malformed-queryid-rejected-by-status-and-cancel|run#7|AssertionError|assert status_message == expected, f"expected message {expected!r}, got {status_message!r}"
Issue-Key: neptunedata.opencypher-query-malformed-queryid-rejected-by-status-and-cancel|run#6|AssertionError|assert status_code == "InvalidParameterException", (
Issue-Key: neptunedata.gremlin-malformed-query-rejected|run#8|AssertionError|assert code == "MalformedQueryException", (
Issue-Key: neptunedata.opencypher-malformed-query-rejected|run#8|AssertionError|assert error.get("Code") == "MalformedQueryException", (
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# neptunedata: 5 net-new refusal-path causes — ML, query-status and query-execution refusals answer 501 NotImplemented, and the one implemented refusal carries no client-visible message

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMLEndpoint, GetGremlinQueryStatus, GetOpenCypherQueryStatus, ExecuteGremlinQuery, ExecuteOpenCypherQuery

## Description
Five distinct root causes on the neptunedata data plane, all reached against a running emulator (the data-plane endpoint answered, so this is not the open issue 20261104T102956Z-neptunedata-is-entirely-unreachable-the-neptune-db-cluster.md: these calls were served and answered wrongly). Every cause is a refusal path: a client that relies on Neptune's modelled error classes cannot distinguish "bad request" from "feature absent".

Cause 1 of 5: CreateMLEndpoint / GetMLEndpoint answer 501 NotImplemented where Neptune answers 404 MLResourceNotFoundException.
AWS: Smithy (com.amazonaws.neptunedata) models CreateMLEndpoint POST /ml/endpoints with error MLResourceNotFoundException (httpError 404, "Raised when a specified machine-learning resource could not be found"), whose required members are detailedMessage, requestId, code. A CreateMLEndpoint naming an mlModelTrainingJobId that does not exist is refused with that class, and GetMLEndpoint on the id that was never created is refused with the same class. Recorded behavior: the message is wholly fixed text with no variable part — "Invalid ID provided. Please check whether a resource with the given ID exists."
Emulator: both operations are scaffold stubs, so the call is answered 501 with error code "NotImplemented" and the text "operation CreateMLEndpoint is scaffolded but not implemented in this emulator".
Why: a client cannot tell a nonexistent training job from an unavailable service; retry/branch logic keyed on MLResourceNotFoundException never fires, and nothing proves the rejected create left no endpoint behind.
Fix: the CreateMLEndpoint and GetMLEndpoint handlers and the ML-resource registry in this crate's state (an ml-endpoint/training-job lookup is what the 404 is derived from).
Traces: neptunedata.create-ml-endpoint-nonexistent-training-job-rejected.

Cause 2 of 5: GetGremlinQueryStatus / CancelGremlinQuery refuse an unparseable queryId with the right class but an EMPTY client-visible message.
AWS: Smithy models GetGremlinQueryStatus GET /gremlin/status/{queryId} and CancelGremlinQuery DELETE /gremlin/status/{queryId} with InvalidParameterException (httpError 400). Recorded behavior: the client-visible message is "Supplied queryId <queryId> is invalid" — fixed text "Supplied queryId " ... " is invalid" around the queryId taken verbatim from the request path.
Emulator: the status/cancel refusal is classified InvalidParameterException (400) but the text is written only into detailedMessage, and the field a client reads as the error message is absent, so botocore surfaces Message == '' (the empty string).
Why: every client-side diagnostic, log line and message-matching branch gets nothing; the refusal is unactionable even though the code is right.
Fix: the error payload builder these handlers share — crate::state::invalid_query_id and the error serialization for this crate's modelled exceptions must populate the message field AWS populates, not detailedMessage alone (same payload is reused by the sibling cancel handler).
Traces: neptunedata.gremlin-query-malformed-queryid-rejected-by-status-and-cancel.

Cause 3 of 5: GetOpenCypherQueryStatus and CancelOpenCypherQuery answer 501 NotImplemented where Neptune answers 400 InvalidParameterException.
AWS: Smithy models GetOpenCypherQueryStatus GET /opencypher/status/{queryId} and CancelOpenCypherQuery DELETE /opencypher/status/{queryId}, both with InvalidParameterException (httpError 400). Recorded behavior: an unparseable queryId is refused with that class and the message "Supplied queryId <queryId> is invalid" (same template as Cause 2), while the sibling ListOpenCypherQueries still succeeds.
Emulator: both handlers are scaffold stubs and answer 501 with error code "NotImplemented".
Why: the openCypher query-management surface diverges from the Gremlin one that IS implemented, so a client written against Neptune's uniform queryId validation breaks on exactly one of the two query languages.
Fix: the GetOpenCypherQueryStatus and CancelOpenCypherQuery handlers, reusing the queryId registry and the shared invalid-queryId payload already present in this crate's state.
Traces: neptunedata.opencypher-query-malformed-queryid-rejected-by-status-and-cancel.

Cause 4 of 5: ExecuteGremlinQuery does not refuse a syntactically invalid Gremlin query with MalformedQueryException.
AWS: Smithy models ExecuteGremlinQuery POST /gremlin with MalformedQueryException (httpError 400, "Raised when a query is submitted that is syntactically incorrect or does not pass additional validation"). Recorded behavior: an unbalanced query is refused 400 MalformedQueryException with the parser's message, whose fixed text is "Failed to interpret Gremlin query: Query parsing failed at line 1, character position at " + <0-based character offset at which the parser stopped> + ", error message : no viable alternative at input '" + <the submitted query up to and including that character> + "'" — the offset and the quoted prefix come from the request's own query string, not from any recorded value.
Emulator: the operation is a scaffold stub, so the malformed query is answered 501 "NotImplemented" rather than refused as malformed; nothing in the emulator ever parses Gremlin. (The stub itself is covered by the auto-filed "Implement neptunedata ExecuteGremlinQuery" item; this cause is the refusal-path requirement that implementation must satisfy — classification plus the parser message — which that item does not state.)
Why: a client cannot distinguish its own syntax error from an emulator gap, and a test suite that asserts a bad query is rejected passes/fails for the wrong reason.
Fix: the ExecuteGremlinQuery handler and its query-parsing/validation step (classification + message construction), in services/service-com-amazonaws-neptunedata-amazonneptunedataplane/src/ops/execute_gremlin_query.rs.
Traces: neptunedata.gremlin-malformed-query-rejected.

Cause 5 of 5: ExecuteOpenCypherQuery does not refuse a syntactically invalid openCypher query with MalformedQueryException.
AWS: Smithy models ExecuteOpenCypherQuery POST /opencypher with MalformedQueryException (httpError 400). Recorded behavior: a misspelled clause is refused 400 MalformedQueryException with the openCypher parser's message, whose template is "Invalid input '<the offending character>': expected '<the alternatives the parser expected>' (line <line>, column <column> (offset: <0-based offset>))" — all variable parts derive from the submitted query (the recording of CREAT (n:`<label>`) SET n.x = yields "Invalid input ' ': expected 'e/E' (line 1, column 6 (offset: 5))").
Emulator: the operation is a scaffold stub and answers 501 "NotImplemented"; no node is created, but no malformed-query refusal is produced either. (As in Cause 4, the bare stub is covered by the auto-filed "Implement neptunedata ExecuteOpenCypherQuery" item; the refusal classification and message template above are the net-new requirement.)
Why: same as Cause 4 — the only signal a client gets for invalid Cypher is an emulator-gap status, and the overly-permissive/rejection contract of the data plane is unobservable.
Fix: the ExecuteOpenCypherQuery handler and its parse/validation step, in services/service-com-amazonaws-neptunedata-amazonneptunedataplane/src/ops/execute_open_cypher_query.rs.
Traces: neptunedata.opencypher-malformed-query-rejected.

## Evidence
Cause 1 — expected: ClientError code MLResourceNotFoundException (HTTP 404), Message "Invalid ID provided. Please check whether a resource with the given ID exists." for both create_ml_endpoint(id=..., mlModelTrainingJobId=<nonexistent>) and the follow-up get_ml_endpoint(id=...). Actual: AssertionError: NotImplemented — code 'NotImplemented' (HTTP 501, "operation CreateMLEndpoint is scaffolded but not implemented in this emulator").

Cause 2 — expected: get_gremlin_query_status(queryId='<unparseable id>') -> InvalidParameterException with Message "Supplied queryId <that id> is invalid"; same for cancel_gremlin_query. Actual: code InvalidParameterException (correct) but "AssertionError: expected message 'Supplied queryId totally-invalid-query-id-not-a-uuid is invalid', got ''" — the text exists only in detailedMessage (crate::state::invalid_query_id formats it into detailed_message, services/service-com-amazonaws-neptunedata-amazonneptunedataplane/src/state.rs:388-394, used by src/ops/get_gremlin_query_status.rs:197-200).

Cause 3 — expected: InvalidParameterException (400) with Message "Supplied queryId <that id> is invalid" from GetOpenCypherQueryStatus and CancelOpenCypherQuery. Actual: "AssertionError: expected InvalidParameterException from GetOpenCypherQueryStatus on a malformed queryId, got 'NotImplemented'" — src/ops/get_open_cypher_query_status.rs:4 and src/ops/cancel_open_cypher_query.rs:4 are both marked "SCAFFOLDED, NOT IMPLEMENTED ... answers HTTP 501".

Cause 4 — expected: MalformedQueryException (400) with "Failed to interpret Gremlin query: Query parsing failed at line 1, character position at <offset>, error message : no viable alternative at input '<query prefix>'". Actual: "AssertionError: expected MalformedQueryException for a syntactically invalid Gremlin query, got 'NotImplemented'" (HTTP 501).

Cause 5 — expected: MalformedQueryException (400) with "Invalid input '<char>': expected '<alternatives>' (line <l>, column <c> (offset: <o>))". Actual: "AssertionError: a syntactically invalid openCypher query was rejected with 'NotImplemented', expected 'MalformedQueryException'" (HTTP 501).

## Steps to Reproduce
1. Stand up a Neptune DB cluster + one db.t4g.medium instance in the run's private network with IAM database authentication, and address the neptunedata client at the cluster endpoint on port 8182 (as each check's _own_neptune_cluster helper does).
2. Cause 1: create_ml_endpoint(id=<fresh id>, mlModelTrainingJobId=<id that was never created>), then get_ml_endpoint(id=<that fresh id>); inspect Error.Code/Error.Message of both, then get_engine_status().
3. Cause 2: get_gremlin_query_status(queryId='totally-invalid-query-id-not-a-uuid') and cancel_gremlin_query(queryId=<same>); inspect Error.Message (empty) while list_gremlin_queries() still succeeds.
4. Cause 3: get_open_cypher_query_status(queryId=<same unparseable id>) and cancel_open_cypher_query(queryId=<same>); both answer 501 NotImplemented while list_open_cypher_queries() is also needed as the positive control.
5. Cause 4: execute_gremlin_query(gremlinQuery="g.addV('<label>').property((((") and inspect Error.Code/Message.
6. Cause 5: execute_open_cypher_query(openCypherQuery="CREAT (n:`<Label>`) SET n.x = ") and inspect Error.Code/Message.
7. Replay: run_behavioral_suite and re-check the five check ids in trace_ids.
