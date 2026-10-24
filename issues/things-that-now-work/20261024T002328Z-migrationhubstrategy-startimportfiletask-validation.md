Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# Undeclared enum values for dataSourceType and groupId[].name are accepted with HTTP 200

_Filed: 2026-10-24T00:23:28Z_
_Operation: StartImportFileTask | Class: validation_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartImportFileTask's dataSourceType targets the DataSourceType enumeration (ApplicationDiscoveryService | MPA | Import | StrategyRecommendationsApplicationDataCollector) and each groupId entry's name targets the GroupName enumeration. AWS refuses a value the enumeration does not declare with ValidationException (HTTP 400), which this operation models.

The emulator's handler validates only the two S3 bucket members and then stores the request verbatim: input.data_source_type and input.group_id go straight onto the ImportFileTask record with no enum-membership check, so e.g. dataSourceType="Bogus" or groupId=[{"name":"NotAGroupName","value":"x"}] answers HTTP 200 and mints a task. This crate's own code documents that the boundary validator does NOT enforce enum membership — ListServers (filter_of/descending/check_group_filter), PutPortfolioPreferences (check_enum) and StartRecommendationReportGeneration (OUTPUT_FORMAT_VALUES / GROUP_NAME_VALUES) all check it in the handler for exactly that reason — so StartImportFileTask is inconsistent with the rest of the service as well as with AWS. The stored bogus group also silently participates in ListServers' groupIdFilter scoping and StartRecommendationReportGeneration's servers_in_groups count.

## Evidence

Smithy model: StartImportFileTaskRequest$dataSourceType -> com.amazonaws.migrationhubstrategy#DataSourceType (values ApplicationDiscoveryService, MPA, Import, StrategyRecommendationsApplicationDataCollector); groupId -> list<Group> whose name member targets GroupName; the operation declares ValidationException (httpError 400, 'The request body isn't valid.'). Handler: src/ops/start_import_file_task.rs lines ~169-224 — only check_bucket_usable is called, then `data_source_type: input.data_source_type` and `group_id: input.group_id.unwrap_or_default()` are persisted unchecked. Contrast src/ops/start_recommendation_report_generation.rs lines ~183-214 and src/ops/list_servers.rs::check_group_filter, which refuse undeclared values and whose comments state the boundary validator does not enforce enum membership. No test in tests/all/import_and_assess.rs or import_file_task_listing.rs asserts a refusal for an undeclared dataSourceType or group name.

## Steps to Reproduce

1. POST /start-import-file-task with {"name":"t","S3Bucket":"migrationhub-strategy-bucket","s3key":"k","dataSourceType":"Bogus"}.
2. Expected: HTTP 400 with x-amzn-errortype ValidationException and no task created. Actual: HTTP 200 with an id, and the task is persisted with the undeclared dataSourceType.
3. POST /start-import-file-task with {"name":"t2","S3Bucket":"migrationhub-strategy-bucket","s3key":"k","groupId":[{"name":"NotAGroupName","value":"x"}]} -> expected 400 ValidationException; actual 200, and the undeclared group is stored and later honoured by ListServers' groupIdFilter.
4. Compare with POST /start-recommendation-report-generation {"groupIdFilter":[{"name":"NotAGroupName","value":"x"}]}, which correctly answers 400.
