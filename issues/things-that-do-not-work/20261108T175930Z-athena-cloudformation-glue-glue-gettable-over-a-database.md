Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__cloudformation__glue.cfn-glue-table-over-missing-database-rejected
Issue-Key: athena__cloudformation__glue.cfn-glue-table-over-missing-database-rejected|run#18|AssertionError|assert got == ("EntityNotFoundException", "Entity Not Found"), (
Issue-Commit: c0b127c099d8543a6fa74c4b10d6d49cf92e859e

# athena__cloudformation__glue: Glue GetTable over a database that does not exist answers the request-derived 'Database <name> not found.' instead of Glue's fixed 'Entity Not Found' (1 cause)

> _Note: this issue was identified against the code base at commit `c0b127c099d8543a6fa74c4b10d6d49cf92e859e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
Cause 1 of 1: glue:GetTable whose DatabaseName names a database that does not exist is refused with the right class but with GetDatabase's templated sentence instead of GetTable's fixed text.
Failing check ids for this cause: athena__cloudformation__glue.cfn-glue-table-over-missing-database-rejected.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): GetTable takes required DatabaseName and Name and models EntityNotFoundException (client, HTTP 400, documented "A specified entity does not exist"). Recorded behavior: a GetTable that cannot resolve the named table — including when the named DATABASE itself does not exist — is refused with EntityNotFoundException whose message is the invariant text `Entity Not Found`: no database name, no table name, no trailing period, and the same text whichever of the two lookups missed. The per-operation difference is deliberate: GetDatabase on a missing name answers EntityNotFoundException with the TEMPLATED `Database ` + the request's Name + ` not found.`, and GetTable does not reuse that wording.

Emulator: HTTP 400 and the code `EntityNotFoundException` are already right; only the message diverges — it is `Database <DatabaseName> not found.`, i.e. GetDatabase's template with the request's DatabaseName interpolated and a trailing period.

Why: the message is the only thing on the wire that distinguishes this refusal, so a client that verifies a Glue table reference is unresolvable (here: confirming that a rolled-back CloudFormation stack left no catalog entry) never matches GetTable's documented sentence. Worse, the emulator's text varies per request where AWS's is constant, so no exact match is possible at all.

Fix: the database-lookup-miss branch of the GetTable handler (services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs) must raise the same fixed `Entity Not Found` text the table-lookup-miss branch already uses, instead of the shared `database_not_found` template; GetDatabase's own templated message must stay as it is.

Related open issues, named for the fixer but NOT covering this check (neither carries its replay key): 20261101T211008Z-glue-gettable-reports-an-absent-database-with-the.md describes exactly this defect but is grounded only in athena__cloudformation__glue.cfn-stack-delete-releases-glue-table-and-its-provenance, and 20261029T185330Z-glue-10-net-new-root-causes-ten-refusal.md describes it as its Cause 1 of 10 grounded only in glue.get-table-not-found, glue.reject-table-in-nonexistent-database and glue.database-delete-cascades-to-tables. Fixing that shared lookup-miss path should close this check too, but it must be re-replayed to confirm.

Traces: athena__cloudformation__glue.cfn-glue-table-over-missing-database-rejected.

## Evidence
Check athena__cloudformation__glue.cfn-glue-table-over-missing-database-rejected:
`AssertionError: the rolled-back table lookup returned ('EntityNotFoundException', 'Database sim2real-athorigin-nodb-<hex> not found.'), expected a missing-database EntityNotFoundException`

Expected (AWS): glue:GetTable(DatabaseName=<a name never created>, Name=<table>) -> HTTP 400, Error.Code 'EntityNotFoundException', Error.Message exactly `Entity Not Found`.
Actual (emulator): HTTP 400, Error.Code 'EntityNotFoundException', Error.Message `Database <DatabaseName> not found.`

Message anatomy: AWS's text here is 100% FIXED — `Entity Not Found`, no variable part. The emulator's text is 'Database ' + the request's DatabaseName + ' not found.'; the generated `sim2real-athorigin-nodb-<hex>` name in the replay line is a per-run value filling that template, not part of the requirement.

Everything before this assertion in the check passes and is the control: the positive control stack (table in an existing database) reaches CREATE_COMPLETE and GetTable resolves it; the negative stack reaches ROLLBACK_COMPLETE with exactly ['AthenaTable'] CREATE_FAILED. Only GetTable's database-miss wording diverges.

## Steps to Reproduce
1. glue CreateDatabase <db>.
2. cloudformation CreateStack with the glue-table-only template parameterised by DatabaseName/TableName, using <db> -> CREATE_COMPLETE; glue GetTable(DatabaseName=<db>, Name=<table>) resolves (control).
3. cloudformation CreateStack with the same template but DatabaseName=<a name never created> -> ROLLBACK_COMPLETE with the AthenaTable resource CREATE_FAILED (already correct).
4. glue GetTable(DatabaseName=<that missing name>, Name=<the table name>).
   Expected: HTTP 400 EntityNotFoundException with Message exactly 'Entity Not Found'.
   Observed: HTTP 400 EntityNotFoundException with Message 'Database <name> not found.'
5. The divergence is reachable without CloudFormation: glue GetTable(DatabaseName=<never created>, Name=anything).
6. Control that must stay correct: glue GetDatabase(Name=<never created>) -> EntityNotFoundException / 'Database <name> not found.'
7. Or replay the check id in trace_ids.
