Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f9b86d9948f04d96bbe5506e2764e861468ce8ee

# GetAssessment materialises an assessment completion without first settling the imports the clock already owes, so the portfolio analysis permanently skips the records those imports brought in

_Filed: 2026-10-27T04:45:25Z_
_Operation: GetAssessment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f9b86d9948f04d96bbe5506e2764e861468ce8ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetAssessment reaches the store through `RegionState::settled_assessment_mut`, which settles the ONE assessment the id names and, on the completion transition, calls `finish_assessment` -> `analyze_portfolio`. It never calls `settle_imports` (nor `settle_all`) first.

In this crate a server or an application component an import file task ingested does NOT live in `RegionState::servers` / `RegionState::application_components` while the task is still running: it is held in `ImportFileTask::pending_servers` / `pending_components` and is only moved into the inventory by `RegionState::settle_imports`. `RegionState::settle_all` — the path every other read of this crate takes (ListServers, ListApplicationComponents, GetServerDetails, GetServerStrategies, GetApplicationComponentDetails, GetApplicationComponentStrategies) — deliberately runs `settle_imports` BEFORE settling the assessments, exactly so the analysis an assessment's completion stands for is recorded on the records an already-due import brought in.

Because an import's completion span and an assessment's completion span are the same `COMPLETION_SPAN`, and an assessment is started after the import it assesses, any instant at which the assessment's completion is due is also an instant at which the earlier import's completion is due. So a caller that goes StartImportFileTask -> StartAssessment -> poll GetAssessment (and makes no other call) materialises the assessment completion while the import's records are still pending: `analyze_portfolio` iterates a `servers` / `application_components` map that does not yet hold them.

The damage is permanent, not transient: `Assessment::settle` clears `completion_due`, so a later `settle_all` (from any sibling read) settles the import, moves the records into the inventory, and then finds nothing to complete — `assessment.settle(now)` returns `None` and `finish_assessment` never runs again. Those servers keep the import baseline recommendation (Rehost / Amazon EC2, from `analysis::import_baseline`) and `lastAnalyzedTimestamp` absent, while every server that happened to already be in the inventory carries the preference-derived recommendation and the completion instant. Whether a resource is analysed therefore depends on whether some unrelated call settled the import first.

`RegionState::stop_assessment` reaches the same helper (`settled_assessment_mut`), so StopAssessment can materialise the same incomplete analysis.

## Evidence

The read's only settlement is of the one assessment — `src/ops/get_assessment.rs:183-185`:

    let found = store
        .region_mut(&region)
        .and_then(|portfolio| portfolio.settled_assessment_mut(&input.id, now));

and that helper runs the portfolio-wide analysis with no import settlement — `src/state.rs:1724-1730`:

    pub fn settled_assessment_mut(&mut self, id: &str, now: SystemTime) -> Option<&mut Assessment> {
        let completed = self.assessments.get_mut(id)?.settle(now);
        if let Some(at) = completed {
            self.finish_assessment(at);
        }
        self.assessments.get_mut(id)
    }

`finish_assessment` -> `analyze_portfolio` reads only the inventory maps — `src/state.rs:1783-1786` and `src/state.rs:1802-1811`:

    pub fn finish_assessment(&mut self, at: SystemTime) {
        self.finish_server_collection(COLLECTION_SUCCESS);
        self.analyze_portfolio(at);
    }

    pub fn analyze_portfolio(&mut self, at: SystemTime) {
        let preferences = self.portfolio_preferences.clone();
        for server in self.servers.values_mut() {

The crate's own all-records settlement does it in the opposite order — `src/state.rs:1936-1948`:

    pub fn settle_all(&mut self, now: SystemTime) {
        self.settle_imports(now);
        let mut completed: Option<SystemTime> = None;
        for assessment in self.assessments.values_mut() {
            if let Some(at) = assessment.settle(now) {
    ...
        if let Some(at) = completed {
            self.finish_assessment(at);
        }

and that is the call every sibling read makes, e.g. `src/ops/list_servers.rs:230-234`:

    let now = ::std::time::SystemTime::now();
    let mut store = state.lock();
    if let Some(portfolio) = store.region_mut(&region) {
        portfolio.settle_all(now);
    }

and `src/ops/get_application_component_details.rs:201-205` (same three lines, with the comment "materialising what the clock already owed this portfolio — an assessment whose completion was due at this same instant ... and the analysis that completion stands for").

That the records are invisible until `settle_imports` runs is `src/state.rs:1703-1719`:

    pub fn settle_imports(&mut self, now: SystemTime) {
    ...
        for record in servers {
            self.servers.insert(record.id.clone(), record);
        }

and `src/state.rs:275-282`: "The servers this import ingested from its file, held here until the task reaches its terminal status and they enter the inventory ([`RegionState::settle_imports`])."

The crate states this very ordering rule elsewhere — `src/ops/start_import_file_task.rs:298-305`:

    // What the clock already owes the imports this Region ran is materialised
    // first, so this ingestion resolves against the portfolio as it stands
    ...
    if let Some(portfolio) = store.region_mut(&region) {
        portfolio.settle_imports(now);
    }

and `src/ops/update_application_component_config.rs:314-324`: "it is also what puts a record an already due import brought in into the inventory this update looks the component up in."

The clearing of `completion_due` that makes the loss permanent is `src/state.rs:649-663` (`Assessment::settle`: `self.completion_due = None;` and it answers `Some(due)` once only).

Test coverage: the assessment-completion cases call `get_import(&router, id)` before `start_assessment` — `tests/all/import_and_assess.rs:708-721` — so the imports are already settled by a read before the assessment is started, and `tests/all/assessment_analysis.rs:31` states the portfolio is "seeded" straight into the store rather than brought in by an import. No test drives StartImportFileTask -> StartAssessment -> GetAssessment with nothing in between, unlike `tests/all/clock_owed_before_a_config_update.rs:711` ("a_write_finds_a_record_whose_arrival_the_clock_already_owed"), which pins the same rule for the config updates.

## Steps to Reproduce

1. Seed a registered collector in the Region's portfolio (the out-of-band registration path `tests/all/import_and_assess.rs:118` uses), so StartAssessment is not refused by `has_data_source`.
2. Create the import bucket in the emulator's S3 and put an import document declaring one SERVER entry under a key.
3. Call POST /start-import-file-task naming that bucket/key. The server is held in `ImportFileTask::pending_servers` with `completion_due = now + COMPLETION_SPAN` (500 ms).
4. Immediately call POST /start-assessment (its own `settle_all` has not yet reached the import's due instant, so the assessment covers 0 servers).
5. Wait until both completion spans have elapsed and make NO other call to the service (no GetImportFileTask, no ListServers, no ListApplicationComponents).
6. Call GET /get-assessment/{assessmentId}: it reports `COMPLETE`, so the assessment's completion — and the analysis it stands for — has been materialised.
7. Call POST /list-servers: the server the import brought in is now reported, but its `recommendationSet` is still the import baseline (strategy `Rehost`, targetDestination `Amazon Elastic Cloud Compute (EC2)`) and `lastAnalyzedTimestamp` is absent, even though the assessment covering the portfolio reached COMPLETE and no later call will ever analyse it.
8. Repeat the sequence but insert one POST /list-servers between step 5 and step 6: the same server now carries the preference-derived recommendation and a `lastAnalyzedTimestamp`, showing the analysis depends on which read settled the import first.
