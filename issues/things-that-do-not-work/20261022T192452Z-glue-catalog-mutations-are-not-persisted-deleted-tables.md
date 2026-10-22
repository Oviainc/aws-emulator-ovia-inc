Issue-Source: trace-sweep-divergence
Issue-Trace: glue-batch-delete-table-all-success
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-batch-delete-table-all-success glue-database-update-and-table-batch-delete glue-database-table-partition-lifecycle glue-database-update-and-table
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetTable|4xx->2xx|$.Message
Issue-Key: glue|GetTable|4xx->2xx|$.__type
Issue-Key: glue|GetTable|4xx->2xx|status
Issue-Key: glue|GetTable|2xx->2xx|$.Table.Parameters
Issue-Key: glue|GetDatabase|2xx->2xx|$.Database.Description
Issue-Key: glue|GetDatabase|2xx->2xx|$.Database.Parameters.tier
Issue-Related: 20261022T192452Z-glue-table-and-database-projections-omit-modeled-catalog.md
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Glue catalog mutations are not persisted: deleted tables still readable, updates lost

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDeleteTable

## Description
Three wire-fixture clusters with one shared root cause: a Glue mutation reports success but the catalog state behind the read path does not change, so the next read returns the pre-mutation entity. Each cause must be closed before this issue is done.

Cause 1 of 3: BatchDeleteTable returns success but the table remains readable.
AWS: Smithy com.amazonaws.glue#BatchDeleteTable takes DatabaseName (required) and TablesToDelete (BatchDeleteTableNameList, required, 0..100) and returns output member Errors (TableErrors) - empty when every named table was deleted. After a BatchDeleteTable that reports no Errors, com.amazonaws.glue#GetTable on a deleted table fails: HTTP 400 with error code EntityNotFoundException (modeled on GetTable, httpError 400; shape doc 'A specified entity does not exist'), whose body carries __type 'EntityNotFoundException' and a Message.
Emulator: GetTable on the just-deleted table answers HTTP 200 with a Table body - neither __type nor Message is present, so the entity is still in the catalog even though BatchDeleteTable reported it deleted.
Why: a delete that does not delete is silent data corruption for every client that treats BatchDeleteTable's empty Errors list as authoritative - teardown/recreate flows see AlreadyExistsException on the recreate, and a client checking 'is it gone' with GetTable concludes the delete failed while the API said it succeeded.
Fix: the BatchDeleteTable handler (services/service-com-amazonaws-glue-awsglue/src/ops/batch_delete_table.rs) must remove each named table from the Glue catalog state the GetTable/GetTables read path consults, and report any table it could not remove in the Errors list instead of claiming a clean delete. Tables deleted this way must then produce EntityNotFoundException on GetTable exactly as the single-table DeleteTable path does.
Traces: glue-batch-delete-table-all-success#task#5 (cluster also covers glue-batch-delete-table-all-success#task#6 and glue-database-update-and-table-batch-delete#task#14).

Cause 2 of 3: UpdateTable does not persist TableInput.Parameters.
AWS: Smithy com.amazonaws.glue#UpdateTable takes DatabaseName (required), Name and TableInput, returns an empty 200 body, and a subsequent GetTable reflects the submitted TableInput - Table.Parameters (ParametersMap, 'these key-value pairs define properties associated with the table') contains the submitted entries.
Emulator: after UpdateTable, GetTable answers 200 but $.Table.Parameters is absent entirely rather than carrying the submitted map, so the update was accepted and dropped. (The same response also shows the Cause 3/Cause 4 projection omissions filed separately in the Glue Table/Database projection issue; the state defect here is the missing Parameters.)
Why: parameters are how clients carry table-level configuration (classification, compression, SerDe hints). A client that writes them and reads them back gets None, so configuration it believes it stored is invisible to itself and to downstream readers.
Fix: the UpdateTable handler (services/service-com-amazonaws-glue-awsglue/src/ops/update_table.rs) must merge the submitted TableInput into the persisted table record - Parameters included - and bump the table version, so the read path serves the updated entity.
Traces: glue-database-table-partition-lifecycle#task#10.

Cause 3 of 3: UpdateDatabase does not persist DatabaseInput.Description or Parameters.
AWS: Smithy com.amazonaws.glue#UpdateDatabase takes Name (required) and DatabaseInput (required) and returns an empty 200 body; the following GetDatabase returns the database with the submitted Description (DescriptionString) and Parameters (ParametersMap) in effect.
Emulator: GetDatabase after a successful UpdateDatabase still reports the values from CreateDatabase - the pre-update Description and the pre-update Parameters entry - so the mutation was acknowledged and discarded.
Why: a client cannot evolve a database definition at all. Read-after-write on a database is wrong, and any flow that updates then verifies (or that re-reads to decide whether an update is needed) loops or diverges from the real catalog.
Fix: the UpdateDatabase handler (services/service-com-amazonaws-glue-awsglue/src/ops/update_database.rs) must write the submitted DatabaseInput fields into the persisted database record that get_database.rs reads, replacing Description/Parameters/LocationUri as AWS does.
Traces: glue-database-update-and-table#task#7 (cluster also covers glue-database-update-and-table-batch-delete#task#7).

## Evidence
Replay of simparity-wire-aws-v1 (121 fixtures, 898 steps, state_reset_failures 0, so no cause here is attributable to leaked state from a previous fixture).

Cause 1 - glue-batch-delete-table-all-success#task#5 GetTable: expected HTTP 400 with $.__type 'EntityNotFoundException' and a Message (recorded example 'Entity Not Found'). Actual: HTTP 200, no __type, no Message - the table body was returned. 3 steps affected.

Cause 2 - glue-database-table-partition-lifecycle#task#10 GetTable after UpdateTable: expected $.Table.Parameters {'sim2real': 'updated'}. Actual: missing (None).

Cause 3 - glue-database-update-and-table#task#7 GetDatabase after UpdateDatabase: expected $.Database.Description 'sim2real probe database v2' and $.Database.Parameters.tier 'silver'. Actual: 'sim2real probe database v1' and 'bronze' - the values CreateDatabase set. 2 steps affected.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture glue-batch-delete-table-all-success to step task#5 - CreateDatabase, CreateTable(s), BatchDeleteTable naming every table (observe an empty Errors list), then GetTable on a deleted table: it returns 200 with the table instead of 400 EntityNotFoundException.
3. Cause 2: replay fixture glue-database-table-partition-lifecycle to step task#10 - CreateTable, UpdateTable with TableInput.Parameters set, then GetTable and inspect $.Table.Parameters.
4. Cause 3: replay fixture glue-database-update-and-table to step task#7 - CreateDatabase, UpdateDatabase with a new Description and a changed Parameters entry, then GetDatabase and inspect $.Database.Description and $.Database.Parameters.
