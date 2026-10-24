Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# Return the whole channel-configuration listing instead of splitting it at an invented page size of 10

_Filed: 2026-10-24T09:06:52Z_
_Operation: ListSlackChannelConfigurations | Class: pagination_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListSlackChannelConfigurations` truncates every response at `CHANNEL_PAGE_SIZE == 10` and mints a `nextToken`, even though the account can hold at most 20 channel configurations and the operation exposes no page-size input.

Expected: a listing bounded by the documented 20-channel account quota comes back in one page — HTTP 200 with every stored configuration in `slackChannelConfigurations` and `nextToken` absent.

Actual: an account with 11-20 configurations gets 10 entries plus a `nextToken`, so a client that reads the first response without following the token sees a listing that is silently short, and a response-body comparison against AWS diverges on array length and on the presence of `nextToken`. `src/state.rs` itself records that the page size is a CHOSEN value picked to keep the token path exercised rather than one the service publishes — the fidelity cost is that the emulator paginates a listing AWS returns whole.

## Evidence

Spec: crate `OPERATIONS.md` lines 61-67 — `ListSlackChannelConfigurations` input is `nextToken` only (`@paginated(inputToken=nextToken, outputToken=nextToken)`, no pageSize member), output `slackChannelConfigurations` (required) + optional `nextToken`. The 20-channel account maximum is the documented quota quoted in `src/state.rs` lines 15-21 and enforced by `src/ops/create_slack_channel_configuration.rs` lines 262-281.

Handler: `src/ops/list_slack_channel_configurations.rs` lines 166-187 (`take(CHANNEL_PAGE_SIZE)` then `encode_cursor`), with `CHANNEL_PAGE_SIZE = 10` at `src/state.rs` line 29.

Tests: `tests/all/slack_channel_configurations.rs` (`pagination_reports_every_stored_configuration_exactly_once`, `drain_pages`) asserts pages of at most the crate's own `CHANNEL_PAGE_SIZE` constant, so no test pins 10 as AWS-observed behaviour.

## Steps to Reproduce

1. POST /control/register-slack-workspace-for-organization with teamId=T012ABCDEFG.
2. POST /control/create-slack-channel-configuration 12 times with distinct channelIds (notifyOnCaseSeverity="none", a valid channelRoleArn).
3. POST /control/list-slack-channel-configurations with body {}.
4. Observe 10 of the 12 configurations plus a `nextToken`; AWS, with a 20-configuration account quota and no page-size parameter, returns all 12 and no `nextToken`.
