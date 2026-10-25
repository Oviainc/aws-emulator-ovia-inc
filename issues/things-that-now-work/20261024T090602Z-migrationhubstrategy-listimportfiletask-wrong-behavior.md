Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# Strategy Recommendations state is not Region-scoped: an import task, assessment, report or preference created through one Region's endpoint is read from every other

_Filed: 2026-10-24T09:06:02Z_
_Operation: ListImportFileTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MigrationHubStrategy is a regional service: the endpoint (`migrationhub-strategy.<region>.amazonaws.com`, endpointPrefix `migrationhub-strategy`, sigv4 signing name `migrationhub-strategy`) and the caller's credential scope select ONE Region's portfolio, and Strategy Recommendations data lives in that Region (the Migration Hub home Region) only. An import file task started through `us-east-1` is therefore invisible to `ListImportFileTask`/`GetImportFileTask` issued against `eu-west-1`, which lists nothing (empty `taskInfos`, no `nextToken`) and answers `ResourceNotFoundException` for the id respectively.

The emulator keeps ONE global store for the whole service. `crate::state::service_state(&*self.engine)` downcasts the engine and hands back a single `ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState`, and not one handler looks at the request's Region — every `handle` body opens with `let _ = ctx;` (or `let _ = (&input, ctx);`), and `ctx` is the only place the signing Region could come from. The consequence spans the whole service, because every resource lives in that one struct: `import_tasks` (StartImportFileTask / GetImportFileTask / ListImportFileTask), `assessments` + `latest_assessment_id` (StartAssessment / GetAssessment / GetLatestAssessmentId / StopAssessment), `recommendation_reports` (StartRecommendationReportGeneration / GetRecommendationReportDetails), `portfolio_preferences` (PutPortfolioPreferences / GetPortfolioPreferences) and the `servers` / `application_components` inventories.

Expected: a resource created through Region A's endpoint is reported only by reads signed for Region A; a read signed for Region B sees an account with no such resource (empty list, 404/validation refusal for the id, and `GetPortfolioPreferences`/`GetLatestAssessmentId` reporting nothing saved). Actual: every read answers from the same global store regardless of the Region the request was signed for, so cross-Region isolation is absent — a client that creates in one Region and reads in another gets a 200 where AWS gives it an empty account, and a per-Region quota/singleton (one preferences record per Region, one latest assessment id per Region) is collapsed into a single global one.

## Evidence

Spec/well-established behavior: the Smithy model's service traits make this a regional, sigv4-signed REST service (`sdkId: MigrationHubStrategy`, `endpointPrefix`/sigv4 name `migrationhub-strategy`), and AWS Region isolation of control-plane resources is well-established — Strategy Recommendations resources belong to one Region's (home Region's) portfolio and are not readable through another Region's endpoint.

Emulator code: `services/service-com-amazonaws-migrationhubstrategy-awsmigrationhubstrategyrecommendation/src/state.rs` — `pub fn service_state(engine) -> Option<ServiceState<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>>` (end of file) returns one unpartitioned store; the state struct's maps (`import_tasks`, `assessments`, `recommendation_reports`, `portfolio_preferences`, `servers`, `application_components`) carry no Region key, and the crate has no `request_region` helper at all (sibling crates, e.g. `services/service-com-amazonaws-migrationhuborchestrator-awsmigrationhuborchestrator/src/state.rs` and `services/service-com-amazonaws-migrationhubrefactorspaces-refactorspaces/src/state.rs`, define `pub fn request_region(parts)` and key their stores by it). Handlers: `src/ops/list_import_file_task.rs` (`let _ = ctx;`, then `store.page_of_import_tasks(...)`), `src/ops/start_import_file_task.rs`, `src/ops/get_import_file_task.rs`, `src/ops/get_latest_assessment_id.rs`, `src/ops/get_portfolio_preferences.rs`, `src/ops/list_servers.rs` — none reads a Region.

Test coverage: none. Every request in `tests/all/` is built by `import_and_assess::call`, which hardcodes one host (`migrationhub-strategy.eu-west-1.amazonaws.com`) and one credential scope (`SIGNED_FOR_EU_WEST_1`, `tests/all/import_and_assess.rs:22-24,116-117`); a repo-wide grep of this crate's tests for a second Region returns nothing, so no test asserts (or contradicts) cross-Region isolation.

## Steps to Reproduce

1. POST /start-import-file-task with `{"name":"import-1","S3Bucket":"migrationhub-strategy-import","s3key":"servers.csv"}`, signed for `us-east-1` (host `migrationhub-strategy.us-east-1.amazonaws.com`, credential scope `.../us-east-1/migrationhub-strategy/aws4_request`) -> 200 with an `id`.
2. GET /list-import-file-task signed for `eu-west-1` (host `migrationhub-strategy.eu-west-1.amazonaws.com`, credential scope `.../eu-west-1/...`) -> expected 200 with an EMPTY `taskInfos`; the emulator returns the us-east-1 task.
3. GET /get-import-file-task/{id} with that id signed for `eu-west-1` -> expected 404 `ResourceNotFoundException`; the emulator returns the task's full detail with 200.
4. Repeat the same shape for the other resources: POST /start-assessment (us-east-1) then GET /get-latest-assessment-id and GET /get-assessment/{id} (eu-west-1); POST /put-portfolio-preferences (us-east-1) then GET /get-portfolio-preferences (eu-west-1); POST /start-recommendation-report-generation (us-east-1) then GET /get-recommendation-report-details/{id} (eu-west-1) -> each eu-west-1 read should report an account with nothing saved, but answers from the us-east-1 state.
