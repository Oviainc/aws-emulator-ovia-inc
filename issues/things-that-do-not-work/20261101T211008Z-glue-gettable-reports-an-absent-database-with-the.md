Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__cloudformation__glue.cfn-stack-delete-releases-glue-table-and-its-provenance
Issue-Key: athena__cloudformation__glue.cfn-stack-delete-releases-glue-table-and-its-provenance|run#8|AssertionError|assert got_t == ("EntityNotFoundException", "Entity Not Found"), (
Issue-Commit: c3867ff5ad180ddb764d6f7e7211ec419ed4ca10

# Glue GetTable reports an absent DATABASE with the request-derived sentence 'Database <name> not found.' instead of Glue's fixed 'Entity Not Found' (1 cause)

> _Note: this issue was identified against the code base at commit `c3867ff5ad180ddb764d6f7e7211ec419ed4ca10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
Cause 1 of 1: when GetTable names a database that does not exist (here: both the table and its database were released by a CloudFormation stack delete), the emulator's EntityNotFoundException carries GetDatabase's templated sentence instead of the fixed text GetTable returns.

Service attribution: this cause is grounded in a check whose id leads with the composite token `athena__cloudformation__glue`, but the diverging call is glue:GetTable and the check's body calls only glue and cloudformation (it never calls athena), so the defect belongs to glue — the bootstrapped service whose handler produces the message — and is filed under it.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): GetTable takes required DatabaseName and Name (NameString, @length 1..255) and models EntityNotFoundException (client, HTTP 400, documented "A specified entity does not exist"). Recorded behavior: a GetTable that cannot resolve the named table — including the case where the named DATABASE itself does not exist — is refused with EntityNotFoundException whose message is the fixed, invariant text `Entity Not Found`: no database name, no table name, no trailing period, and the same text whichever of the two lookups missed. The recording shows the two operations' texts differ by design: in the same check, immediately after, GetDatabase on the very same deleted name is asserted to answer EntityNotFoundException with the TEMPLATED `Database ` + the request's Name + ` not found.`, and that assertion passes.

Emulator: the status (400) and the code (EntityNotFoundException) are already right; only the message diverges. services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs takes the database-lookup miss branch and builds the message with `crate::state::messages::database_not_found(&input.database_name)`, yielding `Database <DatabaseName> not found.` with the request's DatabaseName interpolated and a trailing period — while the table-lookup miss a few lines further down already uses the correct fixed `crate::state::messages::TABLE_ENTITY_NOT_FOUND`.

Why: the error message is the only thing on the wire that distinguishes this refusal, so clients, SDK helpers and provenance/teardown tests that match GetTable's documented sentence never recognize it; worse, the emulator's message varies per request where AWS's is constant, so nothing can match it exactly at all. A client verifying that a deleted stack really released its catalog entry sees an unexpected message and cannot tell a released table from an unrelated failure.

Fix: the database-lookup miss branch of the GetTable handler (services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs) must raise the same bare fixed text the table-miss branch already uses, instead of the shared `messages::database_not_found` template; GetDatabase's own templated message must stay as it is.

Related open issue, named for the fixer: 20261029T185330Z-glue-10-net-new-root-causes-ten-refusal.md describes this same defect as its Cause 1 of 10, but is grounded only in glue.get-table-not-found, glue.reject-table-in-nonexistent-database and glue.database-delete-cascades-to-tables — it does not carry this check's replay key, so it does not cover this check. Fixing that cause's shared lookup-miss path should close both.

Traces: athena__cloudformation__glue.cfn-stack-delete-releases-glue-table-and-its-provenance.

## Evidence
Check athena__cloudformation__glue.cfn-stack-delete-releases-glue-table-and-its-provenance:
`AssertionError: GetTable after delete returned ('EntityNotFoundException', 'Database sim2real-athorigin-b13fe7a5fa41-db not found.')`

Expected (AWS): after the stack reaches DELETE_COMPLETE, glue:GetTable(DatabaseName=<db>, Name=<table>) -> HTTP 400, Error.Code "EntityNotFoundException", Error.Message exactly `Entity Not Found`.
Actual (emulator): HTTP 400, Error.Code "EntityNotFoundException", Error.Message `Database <DatabaseName> not found.`

Message anatomy: AWS's text here is 100% FIXED — `Entity Not Found`, with no variable part. The emulator's text is "Database " + the request's DatabaseName + " not found."; the `sim2real-athorigin-b13fe7a5fa41-db` in the replay line is a per-run generated name filling that template, not part of the requirement.

The neighbouring assertions in the same check pass and are the control for the wording: glue:GetDatabase(Name=<db>) correctly answers EntityNotFoundException / `Database <db> not found.`, and cloudformation:DescribeStackResources(PhysicalResourceId=<table>) correctly answers ValidationError / `Stack for <table> does not exist` — so only GetTable's database-miss wording diverges, and the stack teardown itself is correct.

## Steps to Reproduce
1. cloudformation:CreateStack with the glue-table template (an AWS::S3::Bucket, an AWS::Glue::Database named by a parameter, and an AWS::Glue::Table in it whose StorageDescriptor.Location points at the bucket); poll DescribeStacks to CREATE_COMPLETE.
2. glue:GetTable(DatabaseName=<db>, Name=<table>) -> 200, Table.Name == <table>.
3. Empty the bucket's object versions, then cloudformation:DeleteStack and poll until the stack is gone (DELETE_COMPLETE).
4. glue:GetTable(DatabaseName=<db>, Name=<table>).
   Expected: HTTP 400 EntityNotFoundException with Message exactly 'Entity Not Found'.
   Observed: HTTP 400 EntityNotFoundException with Message 'Database <db> not found.'
5. Control (already correct, keep it correct): glue:GetDatabase(Name=<db>) -> EntityNotFoundException / 'Database <db> not found.'
6. The same divergence is reachable without CloudFormation: glue:GetTable(DatabaseName=<a name never created>, Name=anything) -> must be EntityNotFoundException / 'Entity Not Found'.
7. Re-replay the check id in trace_ids.
