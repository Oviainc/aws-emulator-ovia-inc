Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__cloudformation__glue.glue-get-table-for-absent-entity-rejected athena__cloudformation__glue.cfn-glue-table-over-missing-database-rejected athena__cloudformation__glue.cfn-glue-table-defaults-applied
Issue-Key: athena__cloudformation__glue.glue-get-table-for-absent-entity-rejected|run#14|AssertionError|assert (code, message) == ("EntityNotFoundException", "Entity Not Found"), (
Issue-Key: athena__cloudformation__glue.cfn-glue-table-over-missing-database-rejected|run#16|AssertionError|assert failed == ["AthenaTable"], f"resources that CREATE_FAILED: {failed!r}"
Issue-Key: athena__cloudformation__glue.cfn-glue-table-defaults-applied|run#7|AssertionError|assert diff == {}, f"defaults not applied (got, want): {diff!r}"
Issue-Commit: 9b689e7a3706c4d3e9e7bd76513ebd3e34560844

# athena__cloudformation__glue: 3 net-new root causes — GetTable's absent-database refusal is request-derived, a rolled-back stack reports the STACK itself as CREATE_FAILED, and a created table omits its empty Parameters maps

> _Note: this issue was identified against the code base at commit `9b689e7a3706c4d3e9e7bd76513ebd3e34560844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
THREE causes, all in services that DO answer here (glue and cloudformation are bootstrapped); none of the three is an Athena divergence — no Athena call is involved in any of them. THREE causes must be closed before this issue is submitted.

Cause 1 of 3: Glue GetTable for a table in a database that does not exist.
AWS: Smithy: GetTable declares EntityNotFoundException (client, httpError 400) — 'A specified entity does not exist'. The refusal for an absent entity on this path is the fixed, request-free sentence "Entity Not Found" (no variable parts: neither the database nor the table name appears).
Emulator: 400 EntityNotFoundException with the request-derived sentence "Database <name from the request> not found." (recorded: 'Database sim2real-athorigin-nodb-<id> not found.').
Why: callers (and Athena, which relays Glue's message verbatim) see a message real Glue never emits; assertions and log matching on Glue's fixed text fail, and the refusal shape differs from every other absent-entity refusal.
Fix: the GetTable handler's not-found branch message (do not interpolate the request's DatabaseName). NOTE: the open issue 20261101T211008Z-glue-gettable-reports-an-absent-database-with-the.md describes this same text for a different replay key and owns athena__cloudformation__glue.cfn-stack-delete-releases-glue-table-and-its-provenance; it does NOT carry the check below, so that check is ungated.
Traces: athena__cloudformation__glue.glue-get-table-for-absent-entity-rejected

Cause 2 of 3: CloudFormation emits a CREATE_FAILED stack event for the STACK itself during rollback.
AWS: recorded behavior only. When the single resource of a stack is refused by its handler, the stack rolls back to ROLLBACK_COMPLETE and DescribeStackEvents reports exactly ONE LogicalResourceId with ResourceStatus 'CREATE_FAILED' — the failing resource ('AthenaTable'). The stack's own resource events are CREATE_IN_PROGRESS / ROLLBACK_IN_PROGRESS / ROLLBACK_COMPLETE; the stack is never reported with ResourceStatus 'CREATE_FAILED'.
Emulator: the stack reaches ROLLBACK_COMPLETE correctly, but DescribeStackEvents reports TWO CREATE_FAILED LogicalResourceIds: the failing resource AND an event whose LogicalResourceId is the stack's own name (recorded: 'sim2real-athorigin-04999bcfa51f').
Why: the standard way to find the cause of a failed deploy is to filter events for CREATE_FAILED; the extra stack-level event makes the emulator report a resource that does not exist in the template, so tooling that maps those ids to template resources mis-attributes the failure and operators cannot tell which resource actually failed.
Fix: the CloudFormation stack-event recorder/rollback path — stack-level events must carry the stack lifecycle statuses, not the resource status CREATE_FAILED.
Traces: athena__cloudformation__glue.cfn-glue-table-over-missing-database-rejected

Cause 3 of 3: a Glue table created with no Parameters comes back with those members absent instead of as empty maps.
AWS: Smithy: TableInput/Table carry Parameters (ParametersMap), StorageDescriptor.Parameters (ParametersMap) and StorageDescriptor.SerdeInfo.Parameters (ParametersMap). For a table created without any of them (here through an AWS::Glue::Table resource), GetTable returns each of those members present and EMPTY ({}), alongside the SerdeInfo.SerializationLibrary default.
Emulator: GetTable omits all three members entirely — Table.Parameters, Table.StorageDescriptor.Parameters and Table.StorageDescriptor.SerdeInfo.Parameters are absent (read as None by the SDK), while SerializationLibrary is returned correctly.
Why: a client that reads table['Parameters'] or SerdeInfo['Parameters'] — a map AWS always materialises — gets a KeyError/None instead of an empty map, so round-tripping a table (GetTable -> modify -> UpdateTable) and any code iterating those maps breaks. Related open issue 20261029T201859Z-glue-responses-omit-modeled-members-whose-value-is.md describes the same family from the static surface but does not carry this check.
Fix: the Glue table serializer / default materialisation for CreateTable state — emit modeled ParametersMap members as empty maps rather than omitting them.
Traces: athena__cloudformation__glue.cfn-glue-table-defaults-applied

## Evidence
Cause 1 — AssertionError: GetTable(absent database) returned ('EntityNotFoundException', 'Database sim2real-athorigin-nodb-9c042634bf44 not found.'); expected ('EntityNotFoundException', 'Entity Not Found').
Cause 2 — AssertionError: resources that CREATE_FAILED: ['AthenaTable', 'sim2real-athorigin-04999bcfa51f']; expected ['AthenaTable'] (set of DescribeStackEvents LogicalResourceId where ResourceStatus == 'CREATE_FAILED'; the stack itself reached ROLLBACK_COMPLETE as required).
Cause 3 — AssertionError: defaults not applied (got, want): {'Parameters': (None, {}), 'sd.Parameters': (None, {}), 'sd.SerdeInfo': ({'SerializationLibrary': 'org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe'}, {'SerializationLibrary': 'org.apache.hadoop.hive.serde2.lazy.LazySimpleSerDe', 'Parameters': {}})}.

## Steps to Reproduce
Cause 1: glue:GetTable(DatabaseName=<a name that was never created>, Name=<any>) and read Error.Code/Error.Message.
Cause 2: cloudformation:CreateStack from a template whose only resource is an AWS::Glue::Table ('AthenaTable') whose DatabaseName does not exist; wait for ROLLBACK_COMPLETE; cloudformation:DescribeStackEvents and collect the distinct LogicalResourceId values whose ResourceStatus is 'CREATE_FAILED'.
Cause 3: deploy a stack containing an AWS::Glue::Table with no Parameters, no StorageDescriptor.Parameters and no SerdeInfo.Parameters; glue:GetTable the created table and inspect Table.Parameters, Table.StorageDescriptor.Parameters and Table.StorageDescriptor.SerdeInfo.Parameters.
