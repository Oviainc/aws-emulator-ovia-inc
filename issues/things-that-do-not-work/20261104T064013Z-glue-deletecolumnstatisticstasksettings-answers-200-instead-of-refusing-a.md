Issue-Source: trace-sweep-divergence
Issue-Trace: glue-column-statistics-task-settings-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-column-statistics-task-settings-error-paths glue-column-statistics-task-settings-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|DeleteColumnStatisticsTaskSettings|4xx->2xx|$
Issue-Key: glue|DeleteColumnStatisticsTaskSettings|4xx->2xx|status
Issue-Commit: 530b52a880c7a48571360fb72766276a47e62e70

# Glue DeleteColumnStatisticsTaskSettings answers 200 instead of refusing a missing settings entry

> _Note: this issue was identified against the code base at commit `530b52a880c7a48571360fb72766276a47e62e70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteColumnStatisticsTaskSettings

## Description
One net-new cause: the emulator's DeleteColumnStatisticsTaskSettings handler performs no existence check, so a delete of a (DatabaseName, TableName) pair that has no column-statistics task settings entry is reported as success instead of being refused.

Cause 1 of 1: DeleteColumnStatisticsTaskSettings for a table with no ColumnStatisticsTaskSettings entry.
AWS: Smithy: DeleteColumnStatisticsTaskSettings (com.amazonaws.glue#AWSGlue) takes required DatabaseName and TableName (NameString, @length 1..255) plus optional CatalogID, has an empty output shape, and models EntityNotFoundException (errorKind client, httpError 400) alongside InvalidInputException and OperationTimeoutException. When no settings entry exists for the named database/table, the call is refused with HTTP 400, __type EntityNotFoundException and Message the fixed text 'ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist'. That message is entirely fixed text — note that it names the request MEMBERS 'DatabaseName' and 'TableName' literally rather than interpolating their values, so no part of it comes from the request or from emulator state.
Emulator: HTTP 200 with an empty JSON body ({}), i.e. the delete is reported as having succeeded and no error members are emitted at all.
Why: a client cannot distinguish 'settings were removed' from 'there were never any settings'. Idempotency/cleanup logic that treats a 200 as proof the entry existed will skip a required create, and error handling written against EntityNotFoundException is never exercised, so the absent-entry branch is dead code against the emulator.
Fix: the DeleteColumnStatisticsTaskSettings handler in the glue service crate — look up the settings entry for (CatalogID or the account-root catalog, DatabaseName, TableName) in state before deleting and raise EntityNotFoundException with the fixed message when it is absent; the serializer already carries __type/Message for other Glue 400s.
Traces: glue-column-statistics-task-settings-error-paths#task#5 (also glue-column-statistics-task-settings-lifecycle#task#5).

Related open issues, neither of which carries this fixture step: '20261029T201859Z-glue-accepts-requests-aws-refuses-ten-create-delete.md' (its cause 3 covers CreateColumnStatisticsTaskSettings answering 200 where AWS refuses the un-assumable role, which is why the emulator has a settings entry here at all, and its causes 5/10 cover other delete paths, but it does not cover DeleteColumnStatisticsTaskSettings) and '20261025T170315Z-glue-2-net-new-root-causes-column-statistics.md' (column statistics task RUN state, not the settings delete path). This delete-path admission check must be fixed and re-replayed in its own right.

## Evidence
Replay of simparity-wire-aws-v1, fixture glue-column-statistics-task-settings-error-paths, step task#5, operation DeleteColumnStatisticsTaskSettings:
  status: expected 400, got 200
  $: empty_body — expected {'__type': 'EntityNotFoundException', 'Message': 'ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist'}, got {}
Cluster divergence_key: service=glue|DeleteColumnStatisticsTaskSettings|status=400->200|keys=$,status (affected_count 2).
Same divergence on glue-column-statistics-task-settings-lifecycle#task#5.
No stub_path and no owning issue were reported for this cluster, so it is net-new.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Re-replay glue-column-statistics-task-settings-error-paths#task#5 and glue-column-statistics-task-settings-lifecycle#task#5.
3. Each step calls DeleteColumnStatisticsTaskSettings for a database/table that has no column-statistics task settings entry and asserts HTTP 400 with __type EntityNotFoundException and Message 'ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist'; the emulator answers HTTP 200 with an empty body.
