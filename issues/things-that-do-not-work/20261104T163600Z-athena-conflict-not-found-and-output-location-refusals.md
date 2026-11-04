Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.athena-workgroup-duplicate-name-rejected athena.create-workgroup-duplicate-name-rejected athena.concurrent-workgroup-create-duplicate-name athena.create-data-catalog-duplicate-name-rejected athena.data-catalog-duplicate-name-rejected athena.op-contract-get-work-group athena.query-execution-invalid-workgroup-fails athena.start-query-execution-missing-output-location-rejected athena.query-execution-missing-output-location-fails athena.query-without-output-location-rejected athena.data-catalog-lambda-missing-metadata-function-rejected athena.data-catalog-lambda-type-missing-required-parameter-fails athena.stop-query-execution-cancels-and-blocks-results
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|CreateWorkGroup|InvalidRequestException
Issue-Key: athena.create-workgroup-duplicate-name-rejected|run#8|AssertionError|assert message == "WorkGroup is already created", message
Issue-Key: athena.concurrent-workgroup-create-duplicate-name|run#12|AssertionError|assert all(e == race_conflict or e == duplicate for e in errors), (
Issue-Key: athena.create-data-catalog-duplicate-name-rejected|run#12|AssertionError|assert message == f"DataCatalog {name} has already been created.", repr(message)
Issue-Key: athena.data-catalog-duplicate-name-rejected|run#9|AssertionError|assert message == f"DataCatalog {catalog_name} has already been created.", message
Issue-Key: athena.op-contract-get-work-group|run#17|AssertionError|assert message == "WorkGroup is not found.", f"unexpected GetWorkGroup message: {message!r}"
Issue-Key: athena.query-execution-invalid-workgroup-fails|run#9|AssertionError|assert message == "WorkGroup is not found.", (
Issue-Key: athena.start-query-execution-missing-output-location-rejected|run#10|AssertionError|assert err == expected, f"expected {expected!r}, got {err!r}"
Issue-Key: athena.query-execution-missing-output-location-fails|run#8|AssertionError|assert message == (
Issue-Key: athena.query-without-output-location-rejected|run#9|AssertionError|assert message == (
Issue-Key: athena.data-catalog-lambda-missing-metadata-function-rejected|run#13|AssertionError|assert message == "Lambda catalog parameters cannot be empty.", message
Issue-Key: athena.data-catalog-lambda-type-missing-required-parameter-fails|run#11|AssertionError|assert message == "Lambda catalog parameters cannot be empty.", message
Issue-Key: athena.stop-query-execution-cancels-and-blocks-results|run#20|AssertionError|assert err == expected, f"expected {expected} fetching results of a cancelled query, got {err}"
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Athena conflict, not-found and output-location refusals carry emulator-authored message text instead of AWS's wording (7 causes)

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkGroup

## Description
Athena's control-plane refusals reach the right HTTP status (400) and the right exception code (InvalidRequestException), but the Message is a sentence the emulator composed. In several cases the emulator interpolates the requested name into text AWS leaves wholly fixed, and in others it rewrites AWS's verb or substitutes an entirely different explanation. The message is part of the error contract, so a client (or an infrastructure tool) that branches on it mis-reads the refusal. 7 causes.

The open issue 20261104T063259Z-athena-refusals-omit-athenaerrorcode-errorcode-and-carry-emulator.md reports the same class from the wire corpus; it does not carry any of the behavioral check keys below, and causes 3, 6 and 7 here are not in it at all.

Cause 1 of 7: CreateWorkGroup with a Name that already exists.
AWS: Smithy: CreateWorkGroup binds InvalidRequestException (httpError 400, members AthenaErrorCode + Message). A duplicate Name is refused with the wholly fixed Message 'WorkGroup is already created' — AWS interpolates no name and adds no trailing period — and the existing workgroup is left untouched.
Emulator: Message 'WorkGroup <Name> already exists.' — the requested Name interpolated into text AWS leaves fixed, a different verb, and a trailing period.
Why: an idempotent create-if-absent flow that keys on AWS's conflict sentence does not recognise the conflict.
Fix: the athena crate's CreateWorkGroup conflict path (message text) and the Athena error serializer.
Traces: athena.athena-workgroup-duplicate-name-rejected, athena.create-workgroup-duplicate-name-rejected.

Cause 2 of 7: concurrent CreateWorkGroup on one Name — the losers carry the same wrong text.
AWS: exactly one request wins; every loser is refused 400 InvalidRequestException with one of AWS's two fixed conflict sentences, 'The specified resource already exists.' or 'WorkGroup is already created'. Neither contains the workgroup name.
Emulator: every loser carries the request-derived 'WorkGroup <Name> already exists.'
Why: a race-tolerant client that treats AWS's conflict sentences as 'someone else won, continue' instead treats the refusal as an unknown error and aborts.
Fix: same conflict path as cause 1, exercised under concurrency.
Traces: athena.concurrent-workgroup-create-duplicate-name.

Cause 3 of 7: CreateDataCatalog with a Name that already exists.
AWS: refused 400 InvalidRequestException with a Message of the fixed form 'DataCatalog <Name> has already been created.' — fixed text 'DataCatalog ' + the Name from the request + ' has already been created.' The existing catalog's Type and Parameters are left unchanged.
Emulator: 'DataCatalog <Name> already exists.' — AWS's 'has already been created.' replaced by 'already exists.'
Why: text-matching clients and recorded-response comparisons fail on a message that is not AWS's.
Fix: the athena crate's CreateDataCatalog conflict path.
Traces: athena.create-data-catalog-duplicate-name-rejected, athena.data-catalog-duplicate-name-rejected.

Cause 4 of 7: GetWorkGroup (and StartQueryExecution's workgroup resolution) for a workgroup that does not exist.
AWS: refused 400 InvalidRequestException with the wholly fixed Message 'WorkGroup is not found.' — AWS interpolates no workgroup name.
Emulator: 'WorkGroup <name> is not found.' — the requested name inserted into text AWS leaves fixed.
Why: the message is not AWS's, so a client comparing it (or an operator reading it) sees a different contract; the leak of the requested name also makes the text non-deterministic for clients that log/compare it.
Fix: the athena crate's GetWorkGroup not-found path and the workgroup resolution used by StartQueryExecution.
Traces: athena.op-contract-get-work-group, athena.query-execution-invalid-workgroup-fails.

Cause 5 of 7: StartQueryExecution with no resolvable output location.
AWS: refused 400 InvalidRequestException with the wholly fixed Message 'No output location provided. You did not provide an output location for  your query results. Either specify an S3 bucket location or enable Athena managed query results in your workgroup settings.' — no request-derived part; the doubled space after 'for' is AWS's own text.
Emulator: 'No output location provided. An output location is required either through the Workgroup result configuration setting or as an API input.'
Why: a client surfacing AWS's remediation text to an operator shows a different (and less actionable) instruction, and a client matching the sentence does not branch.
Fix: the athena crate's StartQueryExecution output-location validation (message text).
Traces: athena.start-query-execution-missing-output-location-rejected, athena.query-execution-missing-output-location-fails, athena.query-without-output-location-rejected.

Cause 6 of 7: CreateDataCatalog with Type=LAMBDA and empty Parameters.
AWS: refused 400 InvalidRequestException with the wholly fixed Message 'Lambda catalog parameters cannot be empty.' — a distinct rule about the parameter map being empty — and no catalog is created under that Name (a subsequent GetDataCatalog refuses with 'DataCatalog <Name> was not found.').
Emulator: 'Parameters for a LAMBDA data catalog must contain either function or both metadata-function and record-function.' — the emulator substitutes the wrong rule: AWS's 'either function or both metadata-function and record-function' message applies to a non-empty map that lacks the required keys, not to an empty map.
Why: the client is told the wrong remediation (add specific keys) when the actual rule is that the map must not be empty, so automated repair logic keyed on the message misbehaves.
Fix: the athena crate's CreateDataCatalog LAMBDA parameter validation — distinguish the empty-map rule from the missing-key rule and use each rule's own message.
Traces: athena.data-catalog-lambda-missing-metadata-function-rejected, athena.data-catalog-lambda-type-missing-required-parameter-fails.

Cause 7 of 7: GetQueryResults for a query that was cancelled.
AWS: Smithy: GetQueryResults binds InvalidRequestException (httpError 400). Reading the results of a CANCELLED query is refused with the wholly fixed Message 'Could not find results'.
Emulator: 'Query did not finish successfully. Final query state: CANCELLED' — the emulator reports the terminal state instead of AWS's result-absence message.
Why: a client that distinguishes 'results not materialised' from 'query in a bad state' (for retry vs abandon) keys on the wrong sentence.
Fix: the athena crate's GetQueryResults state check (message text for a cancelled query).
Traces: athena.stop-query-execution-cancels-and-blocks-results.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Every cause agrees on status (400) and code (InvalidRequestException); only the Message diverges.

Cause 1 — athena.athena-workgroup-duplicate-name-rejected, second CreateWorkGroup(Name=<same name>): expected Message='WorkGroup is already created'; actual {'Message': 'WorkGroup sim2real-athena-wg-137100bdae22 already exists.', 'Code': 'InvalidRequestException'}. athena.create-workgroup-duplicate-name-rejected: actual 'WorkGroup sim2real-athena-wg-80bbea42d764 already exists.'
Cause 2 — athena.concurrent-workgroup-create-duplicate-name: 'concurrent losers may only fail with (InvalidRequestException, The specified resource already exists.) or (InvalidRequestException, WorkGroup is already created), got [(InvalidRequestException, WorkGroup sim2real-behav-wg-race-b57f8889f642 already exists.) x4]'.
Cause 3 — athena.create-data-catalog-duplicate-name-rejected: expected 'DataCatalog <name> has already been created.'; actual 'DataCatalog sim2real-cat-dup-62cea0a073be already exists.' athena.data-catalog-duplicate-name-rejected: actual 'DataCatalog sim2real_behav_catdup_dd9d7834df33 already exists.'
Cause 4 — athena.op-contract-get-work-group: 'unexpected GetWorkGroup message: WorkGroup sim2real-wg-missing-c4027293a930 is not found.' (expected the fixed 'WorkGroup is not found.'). athena.query-execution-invalid-workgroup-fails: 'unexpected nonexistent-WorkGroup refusal message: WorkGroup sim2real-behav-wg-missing-ab036247fcc1 is not found.'
Cause 5 — athena.start-query-execution-missing-output-location-rejected: expected ('InvalidRequestException', 'No output location provided. You did not provide an output location for  your query results. Either specify an S3 bucket location or enable Athena managed query results in your workgroup settings.'); actual ('InvalidRequestException', 'No output location provided. An output location is required either through the Workgroup result configuration setting or as an API input.'). Same actual text in athena.query-execution-missing-output-location-fails and athena.query-without-output-location-rejected.
Cause 6 — athena.data-catalog-lambda-missing-metadata-function-rejected, CreateDataCatalog(Name=<name>, Type='LAMBDA', Parameters={}): expected 'Lambda catalog parameters cannot be empty.'; actual 'Parameters for a LAMBDA data catalog must contain either function or both metadata-function and record-function.' Same in athena.data-catalog-lambda-type-missing-required-parameter-fails.
Cause 7 — athena.stop-query-execution-cancels-and-blocks-results: expected ('InvalidRequestException', 'Could not find results'); actual ('InvalidRequestException', 'Query did not finish successfully. Final query state: CANCELLED').

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect the athena checks listed in trace_ids.
3. Minimal manual repro:
   a. CreateWorkGroup(Name=N) twice — the second must be 400 InvalidRequestException 'WorkGroup is already created' (no name interpolated); fire several concurrently and every loser must carry that sentence or 'The specified resource already exists.'
   b. CreateDataCatalog(Name=C, Type='GLUE', ...) twice — the second must be 400 InvalidRequestException 'DataCatalog C has already been created.'
   c. GetWorkGroup(WorkGroup=<absent>) — must be 400 InvalidRequestException 'WorkGroup is not found.' with no name in the text; likewise StartQueryExecution against an absent WorkGroup.
   d. StartQueryExecution with no ResultConfiguration.OutputLocation and a workgroup that sets none — must carry AWS's full 'No output location provided. You did not provide an output location for  your query results. ...' sentence.
   e. CreateDataCatalog(Name=C2, Type='LAMBDA', Parameters={}) — must be 400 InvalidRequestException 'Lambda catalog parameters cannot be empty.', and nothing may be created under C2.
   f. StartQueryExecution, StopQueryExecution, then GetQueryResults on that id — must be 400 InvalidRequestException 'Could not find results'.
