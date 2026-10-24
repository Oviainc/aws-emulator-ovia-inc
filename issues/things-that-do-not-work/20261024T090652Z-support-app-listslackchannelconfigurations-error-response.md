Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# Word the invalid-nextToken refusal in the Support App validation prose envelope

_Filed: 2026-10-24T09:06:52Z_
_Operation: ListSlackChannelConfigurations | Class: error-response_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `nextToken` is not a cursor this service minted, the handler refuses the request through the scaffold channel with the bare message "the specified nextToken is not valid".

Expected: every `ValidationException` this service raises from a handler is worded in the Support App's own prose envelope — "Your <Operation> request isn't valid. <rule> Try your request again." — which is exactly the template `crate::state::invalid_request_message` exists for and which was trace-confirmed for the severity/flag refusals on Create/Update. A refusal of a pagination token should name the operation and the member the same way, so a caller reading only the `message` learns which request was refused and why.

Actual: status (400) and error code (`ValidationException`) are right, but the `message` is an internal paraphrase that names neither the operation nor the member in the service's own wording — the same fault class as the already-fixed "error message templates diverge from AWS" issue, left on the one refusal path that does not go through `invalid_request_message`. `ListSlackWorkspaceConfigurations` shares the identical code path (`src/ops/list_slack_workspace_configurations.rs` lines 164-169), so both listings answer with this wording.

## Evidence

Spec/grounding: the Support App validation-message envelope is established for this service — `src/state.rs` lines 196-207 (`invalid_request_message`) implements it, and the fixed issue `20261022T011025Z-supportapp-error-message-templates-diverge-from-aws-for` records AWS's recorded text "Your CreateSlackChannelConfiguration request isn't valid. ... Try your request again." as a template rather than one literal string. `paginationToken` is a modelled member (len 1..256, pattern ^\S+$) of both listings.

Handler: `src/ops/list_slack_channel_configurations.rs` lines 159-164 — `crate::scaffolded::Scaffolded::validation("the specified nextToken is not valid")`.

Tests: `tests/all/slack_channel_configurations.rs::a_next_token_this_service_did_not_mint_is_refused` and `tests/all/slack_workspace_configurations.rs::a_next_token_this_service_did_not_mint_is_refused` assert only the 400 status and the `x-amzn-errortype` header; a grep of `tests/` for the token message text returns nothing, so no test pins the current wording.

## Steps to Reproduce

1. POST /control/list-slack-channel-configurations with body {"nextToken": "not-a-token"}.
2. Observe HTTP 400 / x-amzn-errortype: ValidationException with body message "the specified nextToken is not valid".
3. Compare with the service's own validation prose envelope used by Create/Update ("Your ListSlackChannelConfigurations request isn't valid. ... Try your request again."); repeat against /control/list-slack-workspace-configurations for the same wording.
