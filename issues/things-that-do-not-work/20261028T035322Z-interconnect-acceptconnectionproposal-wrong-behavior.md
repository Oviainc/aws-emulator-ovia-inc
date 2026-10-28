Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f84dda0f3deaf97a2c7ee51f672a37b240346b3

# AcceptConnectionProposal races the clock: 250 ms after a proposal exists the clock has already accepted it, so the selected attachPoint is never applied

_Filed: 2026-10-28T03:53:22Z_
_Operation: AcceptConnectionProposal | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f84dda0f3deaf97a2c7ee51f672a37b240346b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two independent code paths in this crate perform the same acceptance of a connection proposal, and the clock always wins.

`CreateConnection` stores every new connection in `requested` and hands it the clock-owed progression `requested -> pending -> available`, each leg due `ACTIVATION_SPAN` (250 ms) later. `AcceptConnectionProposal` applies the Attach Point the request selects ONLY while the connection is still in `requested`: `judge_against_store` refuses anything else. So from 250 ms after the proposal exists, an acceptance that selects an attach point the connection does not already hold - the normal case, because the model says the acceptance is precisely what selects it ("Upon accepting the proposal a connection will be made between the AWS network as accessed via the selected Attach Point") - is answered HTTP 400 `InterconnectClientException` "...has already been accepted as connection <id>, whose attach point cannot be changed", and the attach point, description and tags the acceptance carried are never written. There is nothing a caller can do to make the proposal acceptable again: the clock's advance is unconditional and irreversible.

The consequence is that one and the same well-formed request is a 200 or a 400 purely on wall-clock timing, and the operation's entire documented effect is reachable only inside a 250 ms window. The crate's own test suite admits this: it must issue the acceptance as "the very next call" after the create "so the proposal is accepted through the API rather than at the partner portal the clock stands in for" - i.e. the test passes only while it beats the clock, which makes the suite nondeterministic under load as well as leaving the operation's happy path effectively unreachable for a real client (which must call DescribeConnectionProposal in between, as the handler doc itself describes).

The crate also contradicts itself about whether the acceptance path exists: `owed_progression`'s doc justifies the clock-driven advance out of `requested` with "the emulator has no partner portal for anyone to accept at", while `accept_connection_proposal.rs` IS that acceptance and documents itself as performing it.

Note for a fixer: the clock-driven progression itself was introduced by the fixed issue "A created connection never leaves 'requested'" and must NOT be reverted. The defect is that the clock's acceptance and the `AcceptConnectionProposal` handler are two unreconciled writers of the same transition, so the handler's write is lost once the clock has fired.

## Evidence

The model sentence the acceptance is supposed to honour - smithy_query(query="doc", shape="AcceptConnectionProposal"):
```
Accepts a connection proposal which was generated at a supported partner's portal.
... Upon accepting the proposal a connection will be made between the AWS network as accessed via the selected Attach Point and the network previously selected network on the partner's portal.
```

The clock accepts the proposal by itself - services/service-com-amazonaws-interconnect-interconnect/src/state.rs:62-71:
```
/// How long ONE leg of a new connection's provisioning progression takes: the move
/// out of `requested`, and then the move out of `pending`.
...
pub const ACTIVATION_SPAN: Duration = Duration::from_millis(250);
```
src/state.rs:124-129 (the crate's own justification, which the crate contradicts):
```
/// This is the documented `ConnectionState` progression of a connection being
/// brought up ... and
/// it is owed by the CLOCK here because the emulator has no partner portal for
/// anyone to accept at.
```
src/state.rs:137-147:
```
pub fn owed_progression(state: &str, entered: Instant) -> Option<PendingTransition> {
    let next = match state {
        "requested" => "pending",
        "pending" => "available",
        _ => return None,
    };
    Some(PendingTransition { due: entered + ACTIVATION_SPAN, state: next.to_string() })
```
armed on every create - src/ops/create_connection.rs:463 and :502:
```
            state: "requested".to_string(),
...
                pending_transition: crate::state::owed_progression(&connection.state, now),
```

The acceptance is permitted from that one state only - src/state.rs:1034-1040:
```
/// The `ConnectionState` a connection whose proposal has not been accepted yet
/// reports ("requested: The initial state of a connection. The state will remain here
/// until the Connection is accepted on the Partner portal").
///
/// It is the one state an acceptance can be made from: before it there is no
/// connection to accept, and after it the acceptance has already happened.
pub const AWAITING_ACCEPTANCE_CONNECTION_STATE: &str = "requested";
```
src/ops/accept_connection_proposal.rs:215-232 - the refusal the clock makes inevitable:
```
    if connection.state != crate::state::AWAITING_ACCEPTANCE_CONNECTION_STATE {
        if crate::state::is_accepted_state(&connection.state) {
            if crate::state::acceptance_already_applied(
                connection, &input.attach_point, input.description.as_ref(), input.tags.as_ref(),
            ) { return Ok(StoreVerdict::AlreadyAccepted(Box::new(connection.clone()))); }
            return Err(
                AcceptConnectionProposalErrorModeled::InterconnectClientException(
                    InterconnectClientException {
                        message: crate::state::proposal_already_accepted_message(&connection.id),
```
where src/state.rs:1100-1105 words it "...has already been accepted as connection {connection_id}, whose attach point cannot be changed", and the write that is lost is src/ops/accept_connection_proposal.rs:417:
```
        stored.connection.attach_point = input.attach_point.clone();
```
while the handler's own doc (accept_connection_proposal.rs:289-302) states the effect it can no longer deliver: "accepting it associates that proposal's connection with the Attach Point the request selects and hands it on along the documented acceptance progression".

The crate's test admits the race - tests/all/accept_connection_proposal.rs:276-296:
```
        // The proposal, and then — as the very next call — the acceptance of it, so
        // the proposal is accepted through the API rather than at the partner portal
        // the clock stands in for.
        let proposal = create_proposal(&emulator, case).await;
        let activation_key = text(&proposal, "activationKey");
        let (status, body) = emulator.ic_in(case.region, "AcceptConnectionProposal", accept_request(...)).await;
        assert_eq!(status, 200, ...);
```
with rows whose acceptance deliberately selects the OTHER gateway (tests/all/accept_connection_proposal.rs:70-77: "accept_on: The gateway the acceptance selects — the same one on some rows and the other one on the rest"), so those rows 400 as soon as the create-to-accept round trip exceeds 250 ms. No test in the suite exercises an acceptance issued after the activation span has elapsed, so the refusal path this defect produces is unpinned.

## Steps to Reproduce

1. Boot the emulator; create a ready Direct Connect gateway G1 and a second one G2 so two distinct attach points exist.
2. Call ListEnvironments, pick a creatable environmentId and one of its available bandwidths, and call CreateConnection with attachPoint={"directConnectGateway": G1}. Note connection.activationKey and connection.state == "requested".
3. Wait more than 250 ms (ACTIVATION_SPAN), e.g. sleep 400 ms - or simply make one intervening DescribeConnectionProposal call on a loaded machine.
4. Call AcceptConnectionProposal with that activationKey and attachPoint={"directConnectGateway": G2}.
5. Observed: HTTP 400 InterconnectClientException "Unable to process request: The connection proposal for the supplied activationKey has already been accepted as connection <id>, whose attach point cannot be changed" - nobody accepted anything; the clock did. GetConnection still reports attachPoint G1, so the attach point the acceptance selected was never applied and no later call can apply it.
6. Repeat step 4 immediately after step 2 instead (no wait): HTTP 200 and the attach point is applied. The same request therefore succeeds or fails on wall-clock timing alone.
