Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 74b5bf98c0e6c3b6f7149573f3343ad08c47c036

# A deleted connection's activationKey still describes as a live connection proposal

_Filed: 2026-10-24T20:58:28Z_
_Operation: DescribeConnectionProposal | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `74b5bf98c0e6c3b6f7149573f3343ad08c47c036`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeConnectionProposal resolves its `activationKey` through the store's `activation_keys` index and answers from the stored connection record with no regard for that record's `ConnectionState`. A connection that has been deleted — state `deleting`, and then the terminal `deleted` — keeps its index entry for the full deleted-record retention (300s, `DELETED_RECORD_RETENTION`), so for that whole window the activation key of a connection that no longer provides any connectivity is still answered with HTTP 200 and a complete proposal body (`bandwidth`, `environmentId`, `provider`, `location`).

Expected: once DeleteConnection has been called, the activation key tied to that connection names nothing to confirm — the proposal/activation handle is void, and describing it is a refusal (the modelled not-found/client rejection this operation already produces for a key that resolves to nothing). Actual: HTTP 200 with a body indistinguishable from the one returned for a live, confirmable proposal; the output shape carries no `state` member, so a caller (the partner side, which is who holds this key) has no way to tell a void handle from a live one.

This is inconsistent with how the same store treats a deleted connection everywhere else: `state.rs::holds_arn` explicitly excludes `state == "deleted"`, so TagResource / UntagResource / ListTagsForResource all refuse a deleted connection's ARN, while the proposal read happily serves it.

Secondary symptom of the same root cause (the activation-key namespace is not maintained as a proposal namespace): the operation is documented as describing "the details of a connection proposal generated at a partner's portal", and its output deliberately carries no connection id or state, while AcceptConnectionProposal CREATES a `Connection` from such a key. The emulator instead maps keys minted by CreateConnection (keys the customer is meant to carry *to* the portal) into this namespace, which implies a key that both already has a connection and can be accepted into a second one. Scoping the index to keys that currently name an open proposal — and dropping the entry when the connection is deleted — fixes both.

## Evidence

Spec/model grounding (smithy_query): DescribeConnectionProposal is `@readonly`, input `activationKey` (required, @length 1..2048, @sensitive), output `bandwidth`/`environmentId`/`provider`/`location` all @required and NO connection id or state — it describes a proposal, not a connection; its doc is "Describes the details of a connection proposal generated at a partner's portal." AcceptConnectionProposal takes the same `activationKey` plus `attachPoint` and returns `connection` (Connection), i.e. an accepted key is what brings a Connection into existence. `ConnectionState` documents `deleted` as "the connection has been deleted", and DeleteConnection's doc states "the network connectivity provided by the Connection will cease to exist" and that the remote partner is informed of the deletion.

Handler: services/service-com-amazonaws-interconnect-interconnect/src/ops/describe_connection_proposal.rs lines 154-185 — `store.activation_keys.get(&input.activation_key)` then `store.connections.get(&id)` and the output is built from `stored.connection` with no state filter (contrast `src/state.rs::holds_arn`, which the tagging operations use to exclude `DELETED_CONNECTION_STATE`). The index entry is written by src/ops/create_connection.rs lines 364-366 and is only removed when the record itself is evicted (`src/state.rs::materialise_due_transitions` -> `remove_connection` after `DELETED_RECORD_RETENTION`); src/ops/delete_connection.rs lines 160-183 does not touch `activation_keys`.

Test coverage: none asserts this. tests/all/connection_proposal.rs `describe_connection_proposal_refuses_a_key_that_names_no_proposal` (lines 379-468) only exercises keys that were never issued (unrelated string, truncated/extended/lower-cased variants, length bounds); no test calls DescribeConnectionProposal after DeleteConnection, so the post-delete path is unverified rather than deliberately asserted.

Not covered by an existing issue: the open "Interconnect error semantics…" issue's Cause 2 covers only the (then-501) validation rejection of a non-base64 activationKey; the open region-scoping issue mentions the shared `activation_keys` index only for cross-region visibility. No issue addresses the state of the connection a key resolves to.

## Steps to Reproduce

1. Boot the emulator and call ListEnvironments; pick an environment whose state is not `unavailable` and one of its `bandwidths.available` values.
2. CreateConnection on that environment with a listed attachPoint; note the returned `connection.activationKey` and `connection.id`.
3. Call DescribeConnectionProposal with that activationKey and confirm HTTP 200 with the proposal body (the live, pre-delete baseline).
4. Call DeleteConnection with the connection id; GetConnection confirms the record is `deleting` and shortly after `deleted`.
5. Call DescribeConnectionProposal again with the SAME activationKey: expect the key to no longer name a describable proposal (the modelled client/not-found refusal, as for a key that resolves to nothing); observe HTTP 200 with the same bandwidth/environmentId/provider/location body as in step 3.
6. For contrast, call ListTagsForResource with the deleted connection's ARN in the same window: it refuses (the store already treats a `deleted` connection as gone), showing the two reads disagree about the same record.
