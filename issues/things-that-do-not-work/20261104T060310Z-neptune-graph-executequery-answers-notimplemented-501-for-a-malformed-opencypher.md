Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable
Issue-Shape-Id: com.amazonaws.neptunegraph#AmazonNeptuneGraph
Issue-Key: neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: e8571d025aeac7270f8ce9c804fecdb89f8ea73b

# ExecuteQuery answers NotImplemented (501) for a malformed openCypher query instead of ValidationException (400) with the parser's message

> _Note: this issue was identified against the code base at commit `e8571d025aeac7270f8ce9c804fecdb89f8ea73b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteQuery

## Description
Cause 1 of 1: ExecuteQuery has no openCypher query validation, so a syntactically invalid query is refused with the generic scaffold refusal rather than the modeled ValidationException.

AWS: Smithy: ExecuteQuery (POST /queries, hostPrefix "{graphIdentifier}.") models ValidationException with httpError 400 among its errors (also AccessDeniedException 403, ConflictException 409, UnprocessableException 422, ThrottlingException 429, InternalServerException 500). Input members: graphIdentifier (httpHeader+hostLabel, pattern ^g-[a-z0-9]{10}$), queryString (jsonName "query"), language (enum). Recorded behavior: a query with a genuine openCypher syntax error — e.g. a MATCH pattern missing its closing parenthesis — is refused with HTTP 400, error code "ValidationException", and the message produced by the openCypher parser. The recorded message for the probe query has the form "Invalid input '<offending-token>': expected whitespace, comment, COPY, node labels, a property map, ')', MapLiteral or a relationship pattern (line <L>, column <C> (offset: <O>))". The fixed text of that class is "Invalid input '" ... "': expected " <parser's alternatives list> " (line " L ", column " C " (offset: " O "))"; the variable parts are the offending token, the alternative list the parser expects at that position, and the 1-based line/column plus 0-based offset computed from the submitted queryString. They must be derived from the request's query text, never emitted as constants from one recording. A valid query on the same graph (the check's positive control, "RETURN 1 AS one") must still succeed afterwards and stream back one row, i.e. the refusal must not disturb the graph or the session.

Emulator: the call is refused with error code "NotImplemented" and message "operation ExecuteQuery is scaffolded but not implemented in this emulator" (HTTP 501). The replayed check asserts the error code and fails: AssertionError: expected ValidationException, got 'NotImplemented'. Because the refusal is the blanket scaffold refusal, neither the 400 status, the ValidationException code, nor any parser message is produced, and the positive-control query after it is never reached.

Why: a client that submits openCypher and classifies failures by error code cannot distinguish a bad query from an outage. Against AWS, ValidationException is a non-retryable client fault that surfaces the parse position so the caller can fix the query; the emulator's NotImplemented is an unmodeled code for this operation, so SDK error classes for ExecuteQuery never match, retry/backoff policies keyed on the modeled errors do not fire, and tooling that renders the parser's line/column to a user has nothing to render. Query-authoring and validation paths therefore cannot be exercised against the emulator at all.

Why this is not covered by the open stub issue: 20261023T175427Z-neptune-graph-executequery-stub.md asks for ExecuteQuery to be implemented and carries the replay keys of 8 other units (the happy-path/roundtrip/summary ones); it does not name this check or the syntax-error rejection path, so implementing the happy path alone would leave this refusal-path requirement unverified. This issue records that requirement and its replay key.

Fix: ExecuteQuery's handler and its query validation — the openCypher parse step must reject malformed queryString values with ValidationException (HTTP 400) carrying the parser's message template above, with the offending token, expected-alternatives list and line/column/offset computed from the submitted query; the handler must leave graph state untouched on that path so a subsequent valid query on the same graph still succeeds.

Traces: neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable (see file citation: services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/execute_query.rs).

## Evidence
Check neptune-graph.neptune-graph-execute-query-invalid-syntax-unprocessable, verdict mismatch.

Expected (AWS): ExecuteQuery with queryString="MATCH (n RETURN n", language=OPEN_CYPHER on an AVAILABLE graph reached through its private endpoint -> HTTP 400, Error.Code "ValidationException", Error.Message = the openCypher parser's message, recorded as "Invalid input 'R': expected whitespace, comment, COPY, node labels, a property map, ')', MapLiteral or a relationship pattern (line 1, column 10 (offset: 9))" for that query (fixed text = "Invalid input '<tok>': expected <alternatives> (line <L>, column <C> (offset: <O>))"; <tok>, <alternatives>, <L>, <C>, <O> derived from the request's query text). Then ExecuteQuery with "RETURN 1 AS one" -> 200 with payload results == [{"one": 1}].

Actual (emulator): botocore ClientError code "NotImplemented", message "operation ExecuteQuery is scaffolded but not implemented in this emulator" (HTTP 501).

Replay output:
  File "check_neptune_graph_neptune_graph_execute_query_invalid_syntax_unprocessable.py", line 32, in run
    assert code == "ValidationException", f"expected ValidationException, got {code!r}"
  AssertionError: expected ValidationException, got 'NotImplemented'

## Steps to Reproduce
1. Boot the neptune-graph emulator.
2. CreateGraph (provisionedMemory=16, deletionProtection=false, publicConnectivity=false) and poll GetGraph until status AVAILABLE; attach a private graph endpoint via CreatePrivateGraphEndpoint and poll until AVAILABLE.
3. Using a client configured with retries total_max_attempts=1, call ExecuteQuery(graphIdentifier=<graph id>, queryString="MATCH (n RETURN n", language="OPEN_CYPHER").
4. Observe error code NotImplemented / HTTP 501 instead of HTTP 400 ValidationException with the openCypher parser message "Invalid input 'R': expected ... (line 1, column 10 (offset: 9))".
5. Call ExecuteQuery(graphIdentifier=<graph id>, queryString="RETURN 1 AS one", language="OPEN_CYPHER") and observe it also fails with NotImplemented instead of returning results [{"one": 1}].
