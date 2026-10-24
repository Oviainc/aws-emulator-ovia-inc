Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 43c3b52fd22125f2604949f65e29a8c5612a98db

# StopAssessment never moves the portfolio servers' dataCollectionStatus to dataCollectionTaskStopped

_Filed: 2026-10-24T07:04:50Z_
_Operation: StopAssessment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `43c3b52fd22125f2604949f65e29a8c5612a98db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StopAssessment stops the data collection an assessment is running over the account's servers. In AWS the stop is observable on the SERVERS as well as on the assessment record: each server the assessment was collecting for reports ServerDetail$dataCollectionStatus = dataCollectionTaskStopped afterwards (readable through ListServers and GetServerDetails), which is the per-server mirror of the assessment's STOPPED status.

Expected: StartAssessment moves every server it covers to a running RunTimeAssessmentStatus (dataCollectionTaskScheduled / dataCollectionTaskStarted), and StopAssessment moves the ones still collecting to dataCollectionTaskStopped; a later ListServers / GetServerDetails reports that status.

Actual: StopAssessment only touches the Assessment record (status -> STOPPED, completionTime, statusMessage, in_progress = 0). No code path in the crate ever WRITES ServerRecord::data_collection_status — it is only read (to_server_detail, matches_criteria "ANALYSIS_STATUS"). So a server's dataCollectionStatus is whatever it was before the stop (absent for anything the service itself produced), and the assessment lifecycle is invisible on the server read path. The same half is missing on the start side: StartAssessment inserts the Assessment and counts servers but sets no server status either.

This also makes ListServers' serverCriteria=ANALYSIS_STATUS filter unusable for the statuses the assessment lifecycle is supposed to produce: no server ever carries dataCollectionTaskStarted/Stopped unless some other path seeded it.

## Evidence

Spec: StopAssessment (POST /stop-assessment) is documented "Stops the assessment of an on-premises environment"; ServerDetail$dataCollectionStatus targets com.amazonaws.migrationhubstrategy#RunTimeAssessmentStatus whose values are dataCollectionTaskToBeScheduled | dataCollectionTaskScheduled | dataCollectionTaskStarted | dataCollectionTaskStopped | dataCollectionTaskSuccess | dataCollectionTaskFailed | dataCollectionTaskPartialSuccess — dataCollectionTaskStopped exists precisely for a collection a caller stopped. The repo's own recorded-trace corpus names this contract: trace id `migrationhubstrategy.stop-assessment-transitions-data-collection-status-to-stopped` (listed in the open issue 20261023T004429Z-migrationhubstrategy-startassessment-silently-accepts-a-request-in-an.md), whose assertions were never reached in that replay, so no filed issue covers this transition.
Handler: src/ops/stop_assessment.rs:161-180 locks the store, resolves the assessment and calls `assessment.stop(now)` — nothing else; src/state.rs Assessment::stop (lines ~350-363) writes only status/completion_time/status_message/in_progress/completion_due on the assessment. src/ops/start_assessment.rs:195-241 inserts the Assessment and reads `imported_server_count()` but writes no server record.
Grep of the whole crate for `data_collection_status` returns only declarations and READS (src/state.rs:520, 596, 634; src/ops/types/shared_types.rs:268) — there is no assignment anywhere outside tests.
Tests: tests/all/import_and_assess.rs::stopping_an_assessment_is_observed_by_the_next_read asserts only the assessment's own status; tests/all/server_inventory.rs and server_details.rs seed data_collection_status directly into ServerRecord fixtures, so no test asserts that any operation sets it.

## Steps to Reproduce

1. Have at least one server in the portfolio inventory (e.g. via an import, or a pre-seeded ServerRecord) and note its id.
2. POST /start-assessment {"s3bucketForAnalysisData":"migrationhub-strategy-bkt"} -> 200 with an assessmentId.
3. POST /list-servers {} (or GET /get-server-details/{serverId}) -> expected the server's dataCollectionStatus to be a running status (dataCollectionTaskScheduled/dataCollectionTaskStarted); emulator reports the field unchanged/absent.
4. POST /stop-assessment {"assessmentId":"<id>"} -> 200.
5. POST /list-servers {} again -> expected dataCollectionStatus == "dataCollectionTaskStopped" for the servers that were collecting; emulator still reports the field unchanged/absent, while GET /get-assessment/{id} does report status STOPPED.
6. POST /list-servers {"serverCriteria":"ANALYSIS_STATUS","filterValue":"dataCollectionTaskStopped"} -> expected the stopped servers; emulator returns an empty serverInfos.
