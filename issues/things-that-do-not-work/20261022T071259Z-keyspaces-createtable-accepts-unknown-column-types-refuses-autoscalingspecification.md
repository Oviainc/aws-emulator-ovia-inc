Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: keyspaces.create-table-invalid-column-type-rejected keyspaces.table-autoscaling-settings-reflect-config keyspaces.get-table-not-found keyspaces.table-delete-lifecycle
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces.create-table-invalid-column-type-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: keyspaces|CreateTable|NotImplemented
Issue-Key: keyspaces.get-table-not-found|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: keyspaces|DeleteTable|ResourceNotFoundException
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# keyspaces: CreateTable accepts unknown column types, refuses autoScalingSpecification with 501, and table-not-found message does not match AWS

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Three distinct root causes in the Amazon Keyspaces emulator slice, all grounded in failing behavioral checks. Model citations are from services/service-com-amazonaws-keyspaces-keyspacesservice/smithy-model/com-amazonaws-keyspaces-keyspacesservice.json.

=== Cause 1 of 3: CreateTable does not resolve/validate schemaDefinition column type names (unknown type is accepted) ===
Failing check: keyspaces.create-table-invalid-column-type-rejected
AWS: CreateTable models ResourceNotFoundException (HTTP 404) — "The operation tried to access a keyspace, table, or type that doesn't exist" (Smithy: CreateTable errors include ResourceNotFoundException httpError 404; schemaDefinition.allColumns[].type is ColumnDefinition$type, an Amazon Keyspaces data type). Recorded behavior: a column whose type name is not a recognized CQL primitive is treated as a reference to a user-defined type, so CreateTable is REJECTED with ResourceNotFoundException and the request-derived message naming the unresolved type (recorded example: "UDT not_a_real_cql_type does not exist"), and no table is created.
Emulator: CreateTable returns HTTP 200 with a minted resourceArn for schemaDefinition {allColumns:[{name:pk,type:text},{name:val,type:not_a_real_cql_type}], partitionKeys:[{name:pk}]}, and the malformed table is then stored and visible to GetTable/ListTables. crate::state::validate_schema checks only role/name agreement and duplicate column names; it never inspects a column's type string.
Why: a client that relies on the service to reject an unresolvable column type (a typo, or a UDT it forgot to create) silently gets a table it believes is well-formed. Negative-path handling and any UDT-creation ordering logic are never exercised, and the table it later reads back has a column type the real service would never have stored.
Fix: CreateTable's input validation — the schema validation shared by the operation handler (resolve a column type against the CQL primitive set, else against the keyspace's user-defined types, and return the modeled ResourceNotFoundException with the type name when neither resolves) with no table inserted into state.
Traces: keyspaces.create-table-invalid-column-type-rejected.

=== Cause 2 of 3: CreateTable refuses autoScalingSpecification with a 501 NotImplemented instead of accepting and reporting it ===
Failing check: keyspaces.table-autoscaling-settings-reflect-config
AWS: autoScalingSpecification is a modeled, optional CreateTable input member (Smithy: CreateTableRequest$autoScalingSpecification -> AutoScalingSpecification; "The optional auto scaling settings for a table in provisioned capacity mode"), and GetTableAutoScalingSettings returns autoScalingSpecification in its output (Smithy: GetTableAutoScalingSettingsResponse$autoScalingSpecification). CreateTable models no 501: its error set is AccessDenied/Conflict/InternalServer/ResourceNotFound/ServiceQuotaExceeded/Validation. AWS accepts the member on a PROVISIONED table and GetTableAutoScalingSettings reports back the configured minimumUnits/maximumUnits/autoScalingDisabled and the targetTrackingScalingPolicyConfiguration.targetValue.
Emulator: CreateTable with capacitySpecification throughputMode=PROVISIONED plus read/writeCapacityAutoScaling fails with error code NotImplemented and message "CreateTable is implemented in this emulator, but not its autoScalingSpecification member(s): omit them, or the table would be stored with settings no operation here can report." (services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/create_table.rs, the `unimplemented` member list, lines ~252-276). The same guard also refuses replicaSpecifications, cdcSpecification and warmThroughputSpecification.
Why: an unmodeled error code with an unmodeled status makes a legitimate CreateTable call unusable; the SDK surfaces NotImplemented, which no client retry/branch logic expects. Every auto-scaling workflow (create provisioned table, read settings back) is unreachable.
Fix: CreateTable's handler plus the table state record — store autoScalingSpecification on crate::state::Table, and have GetTableAutoScalingSettings serialize it (and the ResourceNotFoundException when the table has none). Remove the member from the 501 guard once it round-trips.
Traces: keyspaces.table-autoscaling-settings-reflect-config.

=== Cause 3 of 3: ResourceNotFoundException for a missing table carries the wrong request-derived message ===
Failing checks: keyspaces.get-table-not-found, keyspaces.table-delete-lifecycle
AWS: both GetTable and DeleteTable model ResourceNotFoundException (HTTP 404) for a table that does not exist (Smithy: GetTable/DeleteTable errors, ResourceNotFoundException httpError 404). The message is built from the request and is the Cassandra-style form naming only the bare table name: "unconfigured table <tableName>" — i.e. no keyspace qualifier, no quoting, no "was not found" phrasing. Two independent recordings of two different operations (GetTable on a never-created table, DeleteTable on an already-deleted table) show the same rule, so the rule, not one recorded string, is the requirement.
Emulator: the status (404) and code (ResourceNotFoundException) are correct, but the message is the keyspace-qualified form produced by crate::state::table_not_found (services/.../src/state.rs lines 276-287): format!("Table '{keyspace_name}.{table_name}' was not found.").
Why: clients and tools that parse or assert on the not-found message (including the AWS-recorded fixtures and Cassandra-compatible tooling that keys off "unconfigured table") diverge from real Keyspaces, even though the code and status agree.
Fix: the shared not-found constructor used by the table-reading/deleting handlers (crate::state::table_not_found) — emit the request-derived "unconfigured table <tableName>" form for table lookups; keyspace lookups keep their own form.
Traces: keyspaces.get-table-not-found, keyspaces.table-delete-lifecycle (the same message assertion also sits downstream in keyspaces.create-table-invalid-column-type-rejected).

## Evidence
Cause 1 — keyspaces.create-table-invalid-column-type-rejected [mismatch]:
  expected: ResourceNotFoundException, message "UDT not_a_real_cql_type does not exist" (recorded AWS), bad table absent from GetTable/ListTables
  actual:   HTTP 200, {'resourceArn': 'arn:aws:cassandra:us-west-2:000000000000:/keyspace/sim2real_behav_ks_.../table/sim2real_behav_bad_...'} — AssertionError: expected the call to be rejected, but it SUCCEEDED

Cause 2 — keyspaces.table-autoscaling-settings-reflect-config [error]:
  expected: CreateTable(capacitySpecification PROVISIONED 5/5, autoScalingSpecification read+write min 5 / max 50 / targetValue 60.0) succeeds; GetTableAutoScalingSettings echoes those values
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: "CreateTable is implemented in this emulator, but not its autoScalingSpecification member(s): omit them, or the table would be stored with settings no operation here can report."

Cause 3 — keyspaces.get-table-not-found [mismatch]:
  expected message 'unconfigured table sim2real_behav_missing_tbl_c922d53772c2'
  got "Table 'sim2real_behav_ks_30c996a9bbea.sim2real_behav_missing_tbl_c922d53772c2' was not found."
Cause 3 — keyspaces.table-delete-lifecycle [mismatch] (DeleteTable on an already-deleted table; code ResourceNotFoundException was correct):
  expected message 'unconfigured table sim2real_behav_tbl_19a79c579b7d'
  got "Table 'sim2real_behav_ks_6f3f7015b4e2.sim2real_behav_tbl_19a79c579b7d' was not found."
(message values above are examples from one run; the requirement is the status, the error code, and for cause 3 the request-derived "unconfigured table <tableName>" form)

## Steps to Reproduce
Boot the emulator and point a botocore keyspaces client at it.

Cause 1:
  1. create_keyspace(keyspaceName=KS)
  2. create_table(keyspaceName=KS, tableName=T1, schemaDefinition={'allColumns':[{'name':'pk','type':'text'}],'partitionKeys':[{'name':'pk'}]}) and wait for ACTIVE
  3. create_table(keyspaceName=KS, tableName=BAD, schemaDefinition={'allColumns':[{'name':'pk','type':'text'},{'name':'val','type':'not_a_real_cql_type'}],'partitionKeys':[{'name':'pk'}]})
     -> emulator returns 200 with a resourceArn; AWS returns 404 ResourceNotFoundException and creates nothing. Confirm with get_table/list_tables that BAD is absent.

Cause 2:
  1. create_keyspace(keyspaceName=KS)
  2. create_table(keyspaceName=KS, tableName=T, schemaDefinition={'allColumns':[{'name':'pk','type':'text'}],'partitionKeys':[{'name':'pk'}]},
     capacitySpecification={'throughputMode':'PROVISIONED','readCapacityUnits':5,'writeCapacityUnits':5},
     autoScalingSpecification={'writeCapacityAutoScaling':{'autoScalingDisabled':False,'minimumUnits':5,'maximumUnits':50,'scalingPolicy':{'targetTrackingScalingPolicyConfiguration':{'targetValue':60.0}}},'readCapacityAutoScaling':{...same...}})
     -> emulator raises NotImplemented; AWS accepts
  3. get_table_auto_scaling_settings(keyspaceName=KS, tableName=T) must report min 5 / max 50 / autoScalingDisabled False / targetValue 60.0

Cause 3:
  a. create_keyspace(keyspaceName=KS); get_table(keyspaceName=KS, tableName=MISSING)
     -> 404 ResourceNotFoundException with message "Table 'KS.MISSING' was not found." instead of "unconfigured table MISSING"
  b. create a table, wait ACTIVE, delete_table, wait until get_table 404s, then delete_table again
     -> same message divergence on DeleteTable

Replay: run the behavioral suite and re-run keyspaces.create-table-invalid-column-type-rejected, keyspaces.table-autoscaling-settings-reflect-config, keyspaces.get-table-not-found, keyspaces.table-delete-lifecycle.
