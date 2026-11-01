Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 3d18e7583331f690daaa55efe02de177ea510fbe

# AcceptConnectionProposal judges resource verdicts before the request's own faults, so a 404/already-accepted answer masks the input refusals its sibling handlers word first

_Filed: 2026-10-29T10:21:22Z_
_Operation: AcceptConnectionProposal | Class: error-response_

> _Note: this issue was identified against the code base at commit `3d18e7583331f690daaa55efe02de177ea510fbe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`judge_against_store` in `accept_connection_proposal.rs` orders its checks resource-first: (1) resolve the `activationKey`, answering 404 `ResourceNotFoundException`; (2) judge the request's `tags`; (3) judge the connection's `state`, answering 400 `InterconnectClientException`; (4) only then judge the `attachPoint` value against its member's own modelled `@length`/`@pattern` (inside `judge_attach_point_in_store`, whose first statement is `attach_point_violation`).

Two faults in the REQUEST are therefore reported as verdicts about a RESOURCE, contradicting two rules this crate states in its own comments:

(a) A tag key in the reserved `aws:` namespace (or one outside `TagKey`/`TagValue`'s modelled lengths) sent with an `activationKey` that resolves to nothing is answered 404 `ResourceNotFoundException` "Unable to retrieve a connection proposal for the supplied activationKey...". Both sibling tag writers in this same crate deliberately judge exactly those faults BEFORE any resource lookup precisely so a 404 cannot mask them. `AcceptConnectionProposal` is a tag writer too (it calls the same `tag_write_violation` and merges into `connection.tags`), but it only ever runs the post-resolution, merged half of that check — the request-level half (`tag_write_violation(None, ...)`) that `TagResource` runs first has no counterpart here.

(b) A malformed `attachPoint` — e.g. `{"directConnectGateway": "not-a-uuid"}`, which the boundary validator does not reach because the constraint sits on a union member one level down (a fact this crate documents at `state.rs:777-780`) — is answered, whenever the connection is no longer in `requested`, with 400 `InterconnectClientException` "...has already been accepted as connection <id>, whose attach point cannot be changed" (or the not-acceptable refusal), never with the invalid-attach-point refusal this crate already words. Because `CreateConnection` arms the clock to leave `requested` after `ACTIVATION_SPAN` (250 ms), that is the ordinary case, not an edge case: the SAME malformed value is refused as invalid input inside the 250 ms window and as a resource verdict afterwards.

The effect is that a client branching on the error code cannot tell "I sent something that is not an attach-point identifier" / "I sent a tag key I may not write" from "this proposal does not exist" / "this proposal was already accepted" — exactly the distinction `state.rs`'s own doc says every reader of an `AttachPoint` preserves, and the distinction the already-fixed CreateConnection issue established for the create path.

## Evidence

The resource-first order — services/service-com-amazonaws-interconnect-interconnect/src/ops/accept_connection_proposal.rs:202-215 (the 404, FIRST):
```
    let stored = store
        .activation_keys
        .get(&input.activation_key)
        .and_then(|connection_id| store.connection_in(region, connection_id));
    let Some(stored) = stored else {
        return Err(
            AcceptConnectionProposalErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::proposal_not_retrievable_message()),
                },
```
then the tags (accept_connection_proposal.rs:224-227) — the MERGED check only, with no request-level pass ahead of the lookup:
```
    if let Some(message) = input
        .tags
        .as_ref()
        .and_then(|tags| crate::state::tag_write_violation(connection.tags.as_ref(), tags))
```
then the state verdict (accept_connection_proposal.rs:244-261):
```
    if connection.state != crate::state::AWAITING_ACCEPTANCE_CONNECTION_STATE {
        if crate::state::is_accepted_state(&connection.state) {
            ...
            return Err(
                AcceptConnectionProposalErrorModeled::InterconnectClientException(
                    InterconnectClientException {
                        message: crate::state::proposal_already_accepted_message(&connection.id),
```
and only LAST the attach-point value (accept_connection_proposal.rs:292-297):
```
    crate::state::judge_attach_point_in_store::<AcceptConnectionProposalError>(
        store,
        region,
        &input.attach_point,
        &connection.environment_id,
    )?;
```
whose first statement is the member-constraint check — src/state.rs:837-839:
```
    if let Some(message) = attach_point_violation(attach_point) {
        return Err(E::invalid_attach_point(message));
    }
```

The rule the SIBLING tag writer states, and judges first — src/ops/tag_resource.rs:184-197:
```
        // What the request itself carries is judged before any resource is looked up:
        // a tag key or value outside the modelled lengths, a key inside the namespace
        // AWS reserves for its own system tags, or more tags in one
        // request than a resource can hold, is a fault in the INPUT whatever the
        // `arn` names, and judging it first keeps a 404 from masking it. ...
        if let Some(message) = crate::state::tag_write_violation(None, &input.tags) {
            return Err(TagResourceErrorModeled::InterconnectValidationException(
```
and src/ops/untag_resource.rs:184-189:
```
        // What the request itself names is judged before any resource is looked up,
        // and nothing is written when it is refused: a key AWS reserves for its own
        // system tags is not the caller's to remove, whatever the `arn` names, so the
        // refusal is the modelled 400 on the input rather than a 404 masking it.
```

The rule the attach-point helper states, which this handler breaks — src/state.rs:777-785:
```
/// The union's members are constrained in the model and NOTHING before the handler holds
/// them to it: the boundary validator holds a top-level member to its target's
/// constraints but not a member one level down, inside a union ... So every reader of
/// an `AttachPoint` judges the value here first, and a value outside the member's
/// constraints is a fault in the REQUEST, reported as invalid input, rather than an
/// identifier resolved against state and then reported as a resource the account does not
/// hold: a caller can tell "I sent something that is not an attach-point identifier" from
/// "this account holds no such attach point".
```
The sibling reader that does judge it first — src/ops/list_connections.rs:162-171 (`attach_point_violation` is the handler's very first act).

Why case (b) is the ordinary path, not an edge case — src/ops/create_connection.rs:475 and :518:
```
            state: "requested".to_string(),
...
                pending_transition: crate::state::owed_progression(&connection.state, now),
```
with src/state.rs:137-147 moving `requested -> pending` after `ACTIVATION_SPAN` (src/state.rs:71: `pub const ACTIVATION_SPAN: Duration = Duration::from_millis(250);`).

Test coverage: tests/all/attach_point_member_constraints.rs pins the invalid-attach-point refusal, and tests/all/reserved_tag_keys.rs the reserved-key refusal, but tests/all/accept_connection_proposal.rs:276-296 issues the acceptance as "the very next call" after the create (i.e. inside the 250 ms window) and never pairs a malformed `attachPoint` or a reserved tag key with an unresolvable/already-accepted proposal, so both orderings above are unpinned.

## Steps to Reproduce

1. Boot the emulator with a ready Direct Connect gateway so a creatable attach point exists.
2. Call AcceptConnectionProposal with an activationKey that was never issued and tags {"aws:owner": "x"}. Observed: HTTP 404 ResourceNotFoundException "Unable to process request: Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist". Expected per tag_resource.rs:184-191 / untag_resource.rs:184-189: the 400 InterconnectValidationException for the reserved `aws:` key, judged before any resource lookup, the way TagResource and UntagResource answer the same key.
3. Call ListEnvironments, pick a creatable environmentId and one of its available bandwidths, and CreateConnection with attachPoint={"directConnectGateway": <ready gateway>}. Note connection.activationKey.
4. Wait more than 250 ms (ACTIVATION_SPAN) so the clock moves the connection out of `requested`.
5. Call AcceptConnectionProposal with that activationKey and attachPoint={"directConnectGateway": "not-a-uuid"} (a value the boundary validator does not reach, since the constraint is on a union member).
6. Observed: HTTP 400 InterconnectClientException "Unable to process request: The connection proposal for the supplied activationKey has already been accepted as connection <id>, whose attach point cannot be changed" — a verdict about the resource. Expected: the invalid-input refusal this crate already words, InterconnectValidationException "Unable to process request: not-a-uuid is not the ID of a DirectConnect Gateway" (state.rs:770-772), which is exactly what the same request returns if issued inside the 250 ms window, and what ListConnections returns for the same malformed attachPoint filter.
