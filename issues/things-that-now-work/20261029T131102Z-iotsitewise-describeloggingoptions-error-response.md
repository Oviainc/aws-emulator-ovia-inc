Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7bcaea858dd8788316443d36e8dbe675c4b82465

# DescribeLoggingOptions never resolves workspaceName, so a workspace that does not exist is reported as a configured scope at the default level

_Filed: 2026-10-29T13:11:02Z_
_Operation: DescribeLoggingOptions | Class: error-response_

> _Note: this issue was identified against the code base at commit `7bcaea858dd8788316443d36e8dbe675c4b82465`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeLoggingOptions passes `input.workspace_name` straight into `logging_options_for` without ever asking whether that workspace exists, so `GET /logging?workspaceName=ghost` answers 200 with `loggingOptions.level = INFO` for a workspace nobody ever created. The operation models `ResourceNotFoundException` (404) and every other workspace-scoped read of this crate resolves the workspace against `store.workspaces` first and refuses an unknown one with that error, each stating it as the crate-wide rule.

This is the read-side complement of the open PutLoggingOptions gap, and it does not disappear when that one is fixed: once the put refuses an unknown workspace, the describe still manufactures a level for one, so the pair of operations on the same setting disagree about whether the workspace exists — a client can read a logging configuration for a scope the service would refuse to configure.

## Evidence

The read, with no workspace resolution — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/describe_logging_options.rs:156-166:
```
        let _ = ctx;

        // Whatever the last put on THIS scope stored, or the never-configured
        // default — read through the store, so the answer follows the setting
        // instead of being fabricated here.
        let logging_options = state
            .lock()
            .logging_options_for(input.workspace_name.as_deref());

        Ok(DescribeLoggingOptionsOutput { logging_options })
```
and the store helper that invents the default for any name — src/state.rs:5261-5268:
```
    pub fn logging_options_for(&self, workspace: Option<&str>) -> LoggingOptions {
        self.logging_options
            .get(workspace.unwrap_or(ACCOUNT_LOGGING_SCOPE))
            .map(|record| record.options.clone())
            .unwrap_or_else(|| LoggingOptions {
                level: DEFAULT_LOGGING_LEVEL.to_string(),
            })
    }
```
(grep for `workspaces` in describe_logging_options.rs: no matches.)

The rule the siblings state and apply — src/ops/describe_pipeline.rs:179-186 and src/ops/describe_enrichment_job.rs:217-222 both open with `if !store.workspaces.contains_key(&input.workspace_name) { return Err(not_found(crate::state::RESOURCE_WORKSPACE, &input.workspace_name)); }`; src/ops/batch_associate_data_segments_to_dataset.rs:272-281 words it: "// The workspace the request names is resolved FIRST — it rides the body here rather than the path — as every workspace-scoped entry point in this crate resolves the workspace before the resource scoped to it". src/ops/get_query_results.rs:185-194 and src/ops/cancel_query.rs:164-168 do the same for a `workspaceName` carried outside the path.

The modelled error is available — smithy_query(query="operation", operation="DescribeLoggingOptions"): errors include {"shape": "ResourceNotFoundException", "errorKind": "client", "httpError": 404}, and `workspaceName` is an optional httpQuery member.

Test coverage: tests/all/describe_logging_options_reads.rs:150-187 (`describing_a_scope_stores_nothing_for_it`) currently reads scopes "one"/"two"/"three" with no workspace created and asserts 200 + the default, but its stated reason is purity — "A describe is a pure READ: it materialises nothing for the scope it reports" — not workspace scoping, and the same test seeds its configured scopes with `put_level(&router, Some("one"), "INFO")` on an uncreated workspace, which the open PutLoggingOptions issue already requires to become a 404. Both suites take their scope names from `crate::logging_options::{put_level, describe}`, so creating the workspaces keeps the purity assertions intact.

## Steps to Reproduce

1. Start the emulator and create no workspace named "ghost".
2. GET /logging?workspaceName=ghost (DescribeLoggingOptions).
3. Observed: 200 with {"loggingOptions":{"level":"INFO"}} — a logging configuration reported for a workspace that does not exist.
4. Expected, by this operation's modelled ResourceNotFoundException (404) and by the rule its siblings state and apply (src/ops/describe_pipeline.rs:181, src/ops/describe_enrichment_job.rs:217, src/ops/batch_associate_data_segments_to_dataset.rs:276): ResourceNotFoundException naming the absent workspace.
5. Contrast: CreateWorkspace "ws1"; GET /logging?workspaceName=ws1 -> 200 default (correct), while GET /logging?workspaceName=ghost is indistinguishable from it.
