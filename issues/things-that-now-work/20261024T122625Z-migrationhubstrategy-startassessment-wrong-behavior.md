Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0788135e0bb0704427b37eee4190850f5fa33c6

# An assessment that reaches COMPLETE produces no analysis result: no server or application-component recommendation, no lastAnalyzedTimestamp, and the saved portfolio preferences influence nothing

_Filed: 2026-10-24T12:26:25Z_
_Operation: StartAssessment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0788135e0bb0704427b37eee4190850f5fa33c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartAssessment is the operation that makes Strategy Recommendations compute something: it "Starts the assessment of an on-premises environment", and when the assessment reaches AssessmentStatus COMPLETE the service has ANALYZED the portfolio. In AWS that analysis is observable on the resources, not only on the assessment record:

- each assessed server reports ServerDetail$recommendationSet (strategy + targetDestination + transformationTool) and ServerDetail$lastAnalyzedTimestamp ("The timestamp of when the server was assessed"), readable through ListServers and GetServerDetails, and GetServerStrategies then returns a non-empty serverStrategies list whose entries carry a StrategyRecommendation status (recommended / viableOption / notRecommended / potential);
- each application component reports ApplicationComponentDetail$recommendationSet / $lastAnalyzedTimestamp / $analysisStatus, and GetApplicationComponentStrategies returns the strategies computed for it;
- the recommendations the analysis produces are the ones the account's saved preferences select: PutPortfolioPreferences$applicationPreferences (managementPreference: awsManagedResources / selfManageResources / noPreference and its target destinations), $databasePreferences (database management/migration preference and target engine), $prioritizeBusinessGoals and $applicationMode are documented as the preferences the service applies when it recommends a strategy.

Expected: after StartAssessment and a poll to GetAssessment status COMPLETE, the portfolio's servers/components carry a computed recommendation whose targetDestination/strategy follows the saved preferences, and GetServerStrategies / GetApplicationComponentStrategies answer with those recommendations.

Actual: the only effect of an assessment in the emulator is the Assessment record itself (status, timestamps, the servers/success/inProgress counters derived from import record counts). Nothing in the crate ever writes ServerRecord::strategy, ServerRecord::target_destination, ServerRecord::last_analyzed, ServerRecord::data_collection_status, or any field of ApplicationComponentRecord — those fields are only ever READ (to_server_detail / to_server_strategies / to_detail) or seeded directly by test fixtures. Assessment::settle only flips status to COMPLETE, moves in_progress into success and writes a statusMessage. So GetServerStrategies on a server the service itself produced returns an empty serverStrategies list forever (only an explicit UpdateServerConfig preference can make it non-empty), GetServerDetails reports no recommendationSet and no lastAnalyzedTimestamp, and the preferences stored by PutPortfolioPreferences are write-only state that no read other than GetPortfolioPreferences ever consults.

Scope note to avoid overlap with the open backlog: the dataCollectionStatus half of the assessment lifecycle (dataCollectionTaskScheduled/Started/Stopped on each server) is already filed under StopAssessment/wrong-behavior, and the scoping of an assessment by assessmentTargets under StartAssessment/parameter-ignored. This issue is the missing ANALYSIS OUTPUT: even with those two fixed and with servers in the inventory, a COMPLETE assessment still recommends nothing.

## Evidence

Smithy model: StartAssessment (POST /start-assessment) "Starts the assessment of an on-premises environment"; ServerDetail$recommendationSet -> RecommendationSet, ServerDetail$lastAnalyzedTimestamp "The timestamp of when the server was assessed", ServerDetail$applicationComponentStrategySummary; ServerStrategy$status -> StrategyRecommendation (recommended|viableOption|notRecommended|potential) and $isPreferred "set to true if the recommendation is set as preferred"; GetServerStrategies "Retrieves recommended strategies and tools for the specified server"; ApplicationComponentDetail$recommendationSet/$lastAnalyzedTimestamp/$analysisStatus; PutPortfolioPreferences "Saves the specified migration and modernization preferences" with applicationPreferences/databasePreferences/prioritizeBusinessGoals/applicationMode.
Handler/state: src/ops/start_assessment.rs:215-262 inserts the Assessment and sets only the counters; src/state.rs Assessment::settle (lines ~337-353) writes status/completion_time/success/in_progress/status_message and nothing else. A grep of src/ for last_analyzed, data_collection_status and `.strategy =` finds declarations and reads only — the single write anywhere in src/ops is `server.preferred_strategy = Some(option)` in src/ops/update_server_config.rs:219 (an explicit caller preference, not an analysis result). src/state.rs:744-780 to_server_strategies is documented to return NO strategy for "a server nothing recommended and nobody chose a preference for", and to_server_detail leaves applicationComponentStrategySummary / antipatternReport* unset by construction.
Tests: tests/all/server_inventory.rs:879-918 and tests/all/server_details.rs:197-201 assert recommendationSet/lastAnalyzedTimestamp round-trip from records SEEDED directly into the servers map; tests/all/import_and_assess.rs asserts only the assessment record's own status/counters. No test asserts that running an assessment produces a recommendation, so nothing pins the current behavior as intended.

## Steps to Reproduce

1. Have at least one server in the portfolio inventory (via an import once ingestion works, or a seeded ServerRecord) and note its id.
2. POST /put-portfolio-preferences {"applicationPreferences":{"managementPreference":{"awsManagedResources":{"targetDestination":["AWS Elastic BeanStalk"]}}},"applicationMode":"ALL"} -> 200.
3. POST /start-assessment {"s3bucketForAnalysisData":"migrationhub-strategy-bkt"} -> 200 with an assessmentId.
4. Poll GET /get-assessment/{assessmentId} until dataCollectionDetails.status == "COMPLETE".
5. GET /get-server-strategies/{serverId} -> expected a non-empty serverStrategies carrying the strategy the analysis computed (with a StrategyRecommendation status); emulator answers 200 with an empty list.
6. GET /get-server-details/{serverId} (or POST /list-servers {}) -> expected recommendationSet and lastAnalyzedTimestamp to be present after the assessment; emulator reports both absent.
7. GET /get-applicationcomponent-details/{applicationComponentId} for a component of that server -> expected recommendationSet/analysisStatus/lastAnalyzedTimestamp from the analysis; emulator reports nothing the assessment produced.
