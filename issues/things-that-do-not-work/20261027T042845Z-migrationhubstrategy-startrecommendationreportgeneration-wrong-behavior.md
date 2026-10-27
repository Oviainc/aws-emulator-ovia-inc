Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# The report's server scope is counted without materialising the import completions the clock already owes, so a report started after an already-due import covers 0 servers

_Filed: 2026-10-27T04:28:44Z_
_Operation: StartRecommendationReportGeneration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartRecommendationReportGeneration stores `servers_covered` — the number of portfolio servers the request's `groupIdFilter` selected — by reading `RegionState::servers` directly, without first materialising the completions the clock already owes this portfolio.

In this crate a server an import file task ingested does NOT live in `RegionState::servers` while the task is still running: it is held in `ImportFileTask::pending_servers` and only moved into the inventory by `RegionState::settle_imports`, which every other handler of the crate calls (through `settle_all`) before it reads or writes. StartRecommendationReportGeneration's only settlement is `in_progress_report_id`, which settles the recommendation-report records alone.

Consequence: an import whose completion span has already elapsed but which no read has yet touched is invisible to the start call. The task is minted with `servers_covered = 0` and, once it settles, `GetRecommendationReportDetails` reports the status message "0 server(s) of the portfolio were in scope" — while the very same start call issued after any sibling read (ListServers, GetServerDetails, StartAssessment) counts the server, because that read materialised the completion. The number a caller is told therefore depends on whether some unrelated caller happened to read first, and the sibling start call does the opposite: StartAssessment settles before it counts the same inventory.

## Evidence

The start call counts without settling imports or assessments — `src/ops/start_recommendation_report_generation.rs:271-274`:

    // The report covers the servers the filter names, counted from this Region's
    // inventory as it stands now. The count is what the task keeps; the filter itself
    // is not stored.
    let servers_covered = store.region(&region).servers_in_groups(&group_id_filter);

Its only settlement is of report records — `src/ops/start_recommendation_report_generation.rs:228-230` calls `portfolio.in_progress_report_id(now)`, whose body settles reports only (`src/state.rs:1842-1850`):

    pub fn in_progress_report_id(&mut self, now: SystemTime) -> Option<String> {
        for report in self.recommendation_reports.values_mut() {
            report.settle(now);
        }

The sibling start call does settle before counting the same map — `src/ops/start_assessment.rs:289-295`:

    // An assessment collects data for the servers the portfolio holds when it starts,
    // so count them with every completion the clock already owed materialised first —
    // including the records an import whose completion is now due brought in.
    let id = store.mint_assessment_id(&region);
    let portfolio = store.region_entry(&region);
    portfolio.settle_all(now);
    let servers = portfolio.server_count();

and the crate states that rule on the counter itself — `src/state.rs:2058-2063`:

    /// How many servers an assessment started now would cover.
    ///
    /// Every server of this Region's inventory ... Call [`Self::settle_all`] first so a server brought in by an import whose
    /// completion is already due is counted.

That a pending import's servers are not in `servers` until settlement is `src/state.rs:1703-1719` (`settle_imports` → `self.servers.insert(record.id.clone(), record)`) and `src/state.rs:275-283` (`pending_servers`: "The servers this import ingested from its file, held here until the task reaches its terminal status and they enter the inventory").

The count is observable through the settled task's message — `src/state.rs:761-765`:

    self.status_message = Some(format!(
        "{} server(s) of the portfolio were in scope. ...",
        self.servers_covered, self.output_format
    ));

Test coverage: `tests/all/recommendation_report.rs:166-176` seeds servers straight into `portfolio.servers` ("One emulator whose inventory holds `servers` — the same `servers` map an import's ingestion writes"), so the scope table at `tests/all/recommendation_report.rs:397-490` never exercises a server that is still pending in an import task — the clock-owed path is untested here, unlike `tests/all/clock_owed_before_a_config_update.rs`, which pins the same rule for UpdateApplicationComponentConfig.

## Steps to Reproduce

1. Create the import bucket in the emulator's S3 and put an import document declaring one SERVER entry (group `groupId`) under a key.
2. Call POST /start-import-file-task naming that bucket/key, so the task is minted with the server in `pending_servers` and `completion_due = now + COMPLETION_SPAN` (500 ms).
3. Wait until the completion span has elapsed, and make NO other call to the service (no ListServers, no GetImportFileTask, no StartAssessment).
4. Call POST /start-recommendation-report-generation with an empty body.
5. Wait for the task's completion span and call GET /get-recommendation-report-details/{id}: `recommendationReportDetails.statusMessage` reports "0 server(s) of the portfolio were in scope".
6. Repeat the whole sequence but insert one GET /list-servers between step 3 and step 4: the same start call now reports "1 server(s) of the portfolio were in scope", showing the stored scope depends on whether an unrelated read settled the import first.
