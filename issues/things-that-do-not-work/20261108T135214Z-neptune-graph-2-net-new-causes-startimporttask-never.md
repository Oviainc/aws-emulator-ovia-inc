Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptune-graph.import-task-invalid-role-arn-rejected neptune-graph.neptune-graph-query-cancel-transitions-state
Issue-Shape-Id: com.amazonaws.neptunegraph#AmazonNeptuneGraph
Issue-Key: neptune-graph.import-task-invalid-role-arn-rejected|run#13|AssertionError|assert code3 == "ValidationException", f"CODE={code3!r}"
Issue-Key: neptune-graph.neptune-graph-query-cancel-transitions-state|_poll_until#7|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Commit: ecb9233b282590e33fbf4eb4f0883534b01eb4b5

# neptune-graph: 2 net-new causes — StartImportTask never enforces the "format required for an S3 source" rule (and resolves the graph ahead of it), and no openCypher query is ever tracked so ListQueries/CancelQuery/GetQuery cannot follow a query's life

> _Note: this issue was identified against the code base at commit `ecb9233b282590e33fbf4eb4f0883534b01eb4b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartImportTask

## Description
The neptune-graph emulator is serving (CreateGraph/GetGraph/DeleteGraph work, graphs reach AVAILABLE, StartImportTask's pass-role denial now answers 403 as AWS does). Two distinct causes remain, both in neptune-graph. Smithy model consulted: services/service-com-amazonaws-neptunegraph-amazonneptunegraph/smithy-model/com-amazonaws-neptunegraph-amazonneptunegraph.json.

=== Cause 1 of 2: StartImportTask does not refuse an S3 `source` sent without `format`, and settles graph existence before that validation ===
AWS: StartImportTask takes its decisions in this order: (1) authorize `roleArn` (403 AccessDeniedException), (2) validate the request's own members, (3) resolve `graphIdentifier`. With an authorized, correctly-trusted role and an `s3://...` source sent WITHOUT `format`, AWS refuses with ValidationException (Smithy: StartImportTask errors ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException @httpError 400; input member `format` targets the `Format` enum and is NOT @required, so the "required when the source is S3" rule is a handler rule, not a shape rule) — and it does so for a graphIdentifier that matches the modelled pattern ^g-[a-z0-9]{10}$ but names no graph, i.e. the member validation is reached BEFORE the graph lookup. Recorded message: "Source format should be specified when Source type is S3." — that whole sentence is fixed text, it interpolates nothing from the request. HTTP 400, code ValidationException.
Emulator: answers ResourceNotFoundException (404, the graph-not-found message) for the same request. The handler validates only `source` emptiness and the format/blankNodeHandling dependency (services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/start_import_task.rs, the `violations` walk), has no "format is required when source is S3" clause at all, and then resolves the graph (`store.settled_graph(...)` -> ResourceNotFoundException) before any such clause could fire. So a well-formed-but-incomplete import request against any graph is reported as a missing graph, and against a real graph it is ACCEPTED with no format.
Why: a client cannot learn that it omitted `format` — it is told the graph does not exist, so it retries against a graph id it believes is wrong, or (on an existing graph) starts a task that can never load anything. A caller that branches on 404 (resource gone, stop) vs 400 (fix the request, retry) takes the wrong branch.
Fix: StartImportTask's validation (start_import_task.rs): add the "format must be specified for an S3 source" clause to the member-validation walk that already runs ahead of `settled_graph`, raising the modelled ValidationException with AWS's fixed sentence; state the rule once in crate::state beside import_source_requirement/blank_node_handling_clause so CreateGraphUsingImportTask, which shares the import members, cannot drift from it.
Traces: neptune-graph.import-task-invalid-role-arn-rejected (AssertionError: CODE='ResourceNotFoundException' where ValidationException with "Source format should be specified when Source type is S3." was required; the two AccessDeniedException assertions ahead of it now pass, so this is the next, uncovered decision in the same operation — the already-open 20261023T175427Z-neptune-graph-7-refusal-path-fidelity-causes-wrong.md covers only the roleArn-authorization ordering of this check, and 20261024T132502Z-neptune-graph-startimporttask-wrong-behavior.md covers only an accepted task's terminal status).

=== Cause 2 of 2: no openCypher query is ever admitted to the store, so ListQueries reports an in-flight query as absent and the cancel/读 lifecycle is unreachable ===
AWS: a long-running openCypher query submitted with ExecuteQuery is visible on the graph's own data plane while it runs: ListQueries(graphIdentifier, maxResults, state='RUNNING') reports a QuerySummary for it carrying its id and its queryString (Smithy: ListQueries GET /queries, hostPrefix '{graphIdentifier}.', output `queries` @required, `state` filter QueryStateInput one of ALL|CANCELLING|RUNNING|WAITING); CancelQuery (DELETE /queries/{queryId}) ends it; and GetQuery for that now-ended id is then refused with ValidationException (400) — not ResourceNotFoundException — whose message is "Supplied queryId <queryId> is invalid", where "Supplied queryId " and " is invalid" are fixed text and <queryId> is the id from the request.
Emulator: ListQueries succeeds (200) but reports an EMPTY `queries` list for a graph with a query in flight, so the query id a client needs for CancelQuery/GetQuery can never be obtained and the whole cancel-then-read transition is unobservable. Root cause: nothing admits a query record — ExecuteQuery, the only operation that starts one, is an unimplemented scaffold stub (src/ops/execute_query.rs returns Scaffolded::not_implemented), while ListQueries (src/ops/list_queries.rs, `store.live_queries_on(&graph_id, now)`) and GetQuery (src/ops/get_query.rs, `store.live_query(...)` -> crate::state::invalid_query_id) already read a store that is never written. NOTE: the auto-filed ExecuteQuery stub issue (20261023T175427Z-neptune-graph-executequery-stub.md) and the ExecuteQuery error-semantics causes in 20261023T175427Z-neptune-graph-7-refusal-path-fidelity-causes-wrong.md cover "ExecuteQuery must answer" and its refusal codes; neither covers this requirement — that an accepted query is REGISTERED as RUNNING for the duration of its run so the three query-plane operations can see, cancel and then reject it. Close them together.
Why: the entire openCypher query-control plane is inert. A client cannot enumerate what is running, cannot cancel a runaway query, and cannot distinguish a cancelled/finished id (400 ValidationException) from an id of another graph; a poll loop looking for its own query in ListQueries never converges.
Fix: ExecuteQuery's handler plus the query records in crate::state — admit a query record (id, queryString, state WAITING->RUNNING, waited/elapsed instants) into the store for the lifetime of an accepted query, so ListQueries' existing state filter and projection report it, CancelQuery transitions it to CANCELLING/ends it, and GetQuery's existing invalid_query_id refusal fires for the ended id.
Traces: neptune-graph.neptune-graph-query-cancel-transitions-state (AssertionError: condition not met within ~22.7s; last=None — ListQueries(state='RUNNING') returned no summary matching the submitted query, so the check never reached CancelQuery).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): 20/36 passed (55.6%); neptune-graph 20/36, 16 failed. 9 failures are the auto-filed ExecuteQuery stub cluster and 5 more are OWNED by open issues; the 2 net-new failures are the ones filed here.

Cause 1 — [mismatch] neptune-graph.import-task-invalid-role-arn-rejected:
  StartImportTask(graphIdentifier='g-0000000000', source='s3://<bucket>/prefix/', roleArn='not-an-arn')          -> AccessDeniedException "User: <callerArn> is not authorized to perform: null"   (matches AWS)
  StartImportTask(graphIdentifier='g-0000000000', source=..., roleArn='arn:aws:iam::111111111111:role/...')      -> AccessDeniedException "Cross-account pass role is not allowed."               (matches AWS)
  StartImportTask(graphIdentifier='g-0000000000', source=..., roleArn=<fresh role trusting neptune-graph.amazonaws.com>, no `format`)
      expected: ValidationException (HTTP 400), message "Source format should be specified when Source type is S3."
      actual:   ResourceNotFoundException (HTTP 404) — the graph-not-found refusal
      AssertionError: CODE='ResourceNotFoundException' (check line 59: assert code3 == "ValidationException")
  Source: src/ops/start_import_task.rs — the member-validation walk (lines ~234-264) carries only an import_source_requirement clause and a blank_node_handling_clause; the graph lookup at lines ~273-278 returns ResourceNotFoundException before any format rule could be evaluated. The modelled input (smithy_query operation=StartImportTask) shows `format` is optional at the shape level, so this rule can only live in the handler.

Cause 2 — [mismatch] neptune-graph.neptune-graph-query-cancel-transitions-state:
  CreateGraph -> AVAILABLE, private endpoint attached; ExecuteQuery('UNWIND range(1,1000000) AS a UNWIND range(1,1000000) AS b RETURN count(*) AS c') submitted on a background thread; then ListQueries(graphIdentifier=<id>, maxResults=10, state='RUNNING') polled every 0.5s.
      expected: a QuerySummary whose queryString contains the submitted query, then CancelQuery(queryId) and GetQuery(queryId) -> ValidationException "Supplied queryId <queryId> is invalid"
      actual:   every ListQueries answered 200 with queries=[] -> AssertionError: condition not met within 22.65s; last=None
  Source: src/ops/execute_query.rs is a Scaffolded::not_implemented stub, so no query record is ever stored; src/ops/list_queries.rs:232-238 filters `store.live_queries_on(graph_id, now)` (always empty) and src/ops/get_query.rs:200-207 reads `store.live_query(...)`, whose miss path already builds the correct crate::state::invalid_query_id ValidationException.

## Steps to Reproduce
Boot the emulator from this checkout and point a botocore neptune-graph client at it.

Cause 1:
1. iam.create_role(RoleName=<unique>, AssumeRolePolicyDocument=trust policy allowing sts:AssumeRole to Service 'neptune-graph.amazonaws.com') -> note Role.Arn.
2. neptune_graph.start_import_task(graphIdentifier='g-0000000000', source='s3://some-bucket/prefix/', roleArn=<that role arn>)  # no `format`
3. Observe ResourceNotFoundException (404) naming graph g-0000000000; expected ValidationException (400) with message "Source format should be specified when Source type is S3." — AWS validates the request's members before resolving the graph.
4. Repeat step 2 against a real AVAILABLE empty graph with no `format`: the emulator ACCEPTS the task (201) where AWS refuses it with the same ValidationException.

Cause 2:
1. create_graph(graphName=<unique>, provisionedMemory=16, deletionProtection=False, publicConnectivity=True); poll get_graph until status AVAILABLE.
2. On a background thread call execute_query(graphIdentifier=<id>, queryString='UNWIND range(1, 1000000) AS a UNWIND range(1, 1000000) AS b RETURN count(*) AS c', language='OPEN_CYPHER').
3. Poll list_queries(graphIdentifier=<id>, maxResults=10, state='RUNNING').
4. Observe queries=[] forever; expected one QuerySummary with the submitted queryString and an id.
5. With that id, cancel_query(graphIdentifier=<id>, queryId=<id>) then get_query(graphIdentifier=<id>, queryId=<id>) -> expected ValidationException (400) "Supplied queryId <queryId> is invalid"; today step 4 makes steps 5 unreachable.
