Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# Stop paginating the workspace listing below the documented 5-workspace account maximum

_Filed: 2026-10-24T09:06:52Z_
_Operation: ListSlackWorkspaceConfigurations | Class: pagination_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator truncates `ListSlackWorkspaceConfigurations` at `WORKSPACE_PAGE_SIZE` records per page and mints a `nextToken` whenever more exist. `WORKSPACE_PAGE_SIZE` is `MAX_SLACK_WORKSPACE_CONFIGURATIONS / 2`, which in `usize` arithmetic is `5 / 2 == 2` — so an account with 3, 4 or 5 registered Slack workspaces gets a 2-record page plus a `nextToken`.

Expected: AWS Support App documents a maximum of 5 Slack workspaces per account (`CreateSlackChannelConfiguration` doc: "You can add up to 5 Slack workspaces for your account") and the operation exposes no page-size input (the `@paginated` trait carries only `inputToken`/`outputToken`, no `pageSize` member). A listing bounded by 5 is returned whole: HTTP 200 with every registered workspace in `slackWorkspaceConfigurations` and no `nextToken`.

Actual: with 3 registered workspaces the emulator answers a body with 2 entries and a `nextToken`, and the caller must make a second round trip to see the third. A client that reads `slackWorkspaceConfigurations` from the first response (rather than driving an SDK paginator) sees a listing that is silently short, and a response-shape comparison against AWS diverges on both the array length and the presence of `nextToken`.

The same construction is used for `ListSlackChannelConfigurations` (filed separately, lower severity because its quota is 20).

## Evidence

Spec: `services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json` — `ListSlackWorkspaceConfigurations` input is `nextToken` only (no maxResults/pageSize), output `slackWorkspaceConfigurations` + optional `nextToken` (crate `OPERATIONS.md` lines 69-75). The 5-workspace-per-account maximum is stated in the service documentation quoted in `src/state.rs` lines 31-40.

Handler: `src/ops/list_slack_workspace_configurations.rs` lines 171-188 takes `WORKSPACE_PAGE_SIZE` records and mints a cursor when a further record remains. `src/state.rs` line 49: `pub const WORKSPACE_PAGE_SIZE: usize = MAX_SLACK_WORKSPACE_CONFIGURATIONS / 2;` — integer division of 5 gives 2, which is not even the "half the quota" the comment intends.

Tests: `tests/all/slack_workspace_configurations.rs` (`pagination_reports_every_registered_workspace_exactly_once`, `drain_workspace_pages`) asserts pages of at most `WORKSPACE_PAGE_SIZE` and a `nextToken` exactly when `count > WORKSPACE_PAGE_SIZE` — i.e. it pins the emulator's own constant, not an AWS-observed page size, so raising the page size to the documented account maximum keeps those assertions satisfied.

## Steps to Reproduce

1. POST /control/register-slack-workspace-for-organization three times with distinct teamIds (e.g. T012ABCDEFG, T999ZZZZZZZ, T-third).
2. POST /control/list-slack-workspace-configurations with body {}.
3. Observe only 2 of the 3 registered workspaces in `slackWorkspaceConfigurations` plus a `nextToken`; AWS, whose per-account maximum is 5 and which takes no page-size parameter, returns all 3 with no `nextToken`.
