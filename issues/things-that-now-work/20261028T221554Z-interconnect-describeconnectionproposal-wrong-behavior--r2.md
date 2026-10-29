Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ebd02413e29cf57b2ef3e9c9311b8c37c19f24b

# The proposal's bandwidth is read live off the connection record, so an UpdateConnection retroactively rewrites the parameters the activation key captured

_Filed: 2026-10-28T22:15:54Z_
_Operation: DescribeConnectionProposal | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ebd02413e29cf57b2ef3e9c9311b8c37c19f24b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeConnectionProposal builds its `bandwidth` output from the CURRENT value stored on the connection record, and `UpdateConnection` rewrites exactly that field. The stored record keeps no creation-time copy of it, so after a bandwidth change the same activation key describes a proposal whose bandwidth is NOT the one the initial creation request captured.

The model states the opposite twice: the activation key "captures the desired parameters from the initial creation request", and the proposal "contains the Environment and bandwidth that were chosen on the partner's portal and cannot be modified". This crate restates the second sentence verbatim in AcceptConnectionProposal's own handler doc ("the Environment and the bandwidth ... are read from the proposal and nothing the request carries can change them"), yet the proposal's bandwidth IS modifiable through a sibling handler of the same crate.

The window is the ordinary one: `UPDATABLE_CONNECTION_STATES` includes `requested` (src/state.rs:1135), which is precisely the state a connection sits in while the caller still holds an unused activation key, and `proposal_in` resolves a proposal in any state that is not deleting/deleted (src/state.rs:399-404) — so describe keeps answering after the update and reports the rewritten value. A caller that reads the proposal, changes the bandwidth and re-reads gets two different "chosen on the partner's portal" bandwidths for one key.

This is distinct from the FIXED DescribeConnectionProposal/wrong-behavior issue (20261024T205828Z), which was about a DELETED connection's key still describing a live proposal; that fix added the deleting/deleted filter and touched nothing about which bandwidth is reported.

## Evidence

Spec, `smithy_query(query="doc",shape="DescribeConnectionProposalRequest",member="activationKey")`:
> "An Activation Key that was generated on a supported partner's portal. This key captures the desired parameters from the initial creation request."
(mirrored into the handler's own contract header, services/service-com-amazonaws-interconnect-interconnect/src/ops/describe_connection_proposal.rs:47-49: `//! * activationKey (ActivationKey, len 1..2048) **required** — An Activation Key that was` / `//!   generated on a supported partner's portal. This key captures the desired parameters from` / `//!   the initial creation request.`)

Spec, `smithy_query(query="doc",shape="AcceptConnectionProposal")`:
> "The proposal contains the Environment and bandwidth that were chosen on the partner's portal and cannot be modified."

The crate's own restatement of that rule — src/ops/accept_connection_proposal.rs:322-325:
```
    /// which the clock owes it. The Environment and the bandwidth were chosen on the
    /// partner's portal and "cannot be modified", so they are read from the proposal
    /// and nothing the request carries can change them; the optional `description` and
    /// `tags` are the caller's to set on the resulting Connection.
```

The describe path, which reads the live record — src/ops/describe_connection_proposal.rs:182-187:
```
                    DescribeConnectionProposalOutput {
                        bandwidth: connection.bandwidth.clone(),
                        environment_id: connection.environment_id.clone(),
                        provider,
                        location,
                    }
```

The write that modifies it — src/ops/update_connection.rs:252-254:
```
        if let Some(bandwidth) = input.bandwidth.clone() {
            if bandwidth != stored.connection.bandwidth {
                stored.connection.bandwidth = bandwidth.clone();
```
permitted from the very state an unredeemed proposal is in — src/state.rs:1135:
```
pub const UPDATABLE_CONNECTION_STATES: [&str; 2] = ["requested", "available"];
```
and the proposal still resolves afterwards, because only deleting/deleted is filtered — src/state.rs:399-404:
```
    pub fn proposal_in(&self, region: &str, activation_key: &str) -> Option<&StoredConnection> {
        self.activation_keys
            .get(activation_key)
            .and_then(|connection_id| self.connection_in(region, connection_id))
            .filter(|stored| !is_deleted_or_deleting(&stored.connection.state))
```
No creation-time copy exists to report instead — the stored record's non-response bookkeeping is only region/remote_account/pending_transition/evict_due (src/state.rs:152-171).

Test coverage: tests/all/connection_proposal.rs:8-9 states the pinned rule as "an `activationKey` resolves to the proposal the stored connection record holds, and the describe reports that record's bandwidth", and :267-283 asserts `proposal[member] == connection[member]` for `bandwidth`. Every row there creates and describes only — no row calls UpdateConnection (grep for `UpdateConnection` in that file: no match), so the mutated case is unpinned and those assertions stay green if the describe reports the captured creation-time bandwidth.

## Steps to Reproduce

1. Mount Interconnect beside Direct Connect (as tests/all/emulator.rs does) and create one ready Direct Connect gateway.
2. CreateConnection on a seeded environment that offers two bandwidths (e.g. env-csp-oraclecloud-eu-west-1, which asks for no remoteAccount) with bandwidth=1Gbps; record `connection.activationKey` and `connection.id`. The connection is returned in state `requested`.
3. DescribeConnectionProposal(activationKey=K) -> bandwidth "1Gbps".
4. UpdateConnection(identifier=<id>, bandwidth="10Gbps") -> accepted, because `requested` is in UPDATABLE_CONNECTION_STATES (src/state.rs:1135), and src/ops/update_connection.rs:252-254 overwrites the record's bandwidth.
5. DescribeConnectionProposal(activationKey=K) again -> bandwidth "10Gbps": the proposal the model says "cannot be modified", and whose key "captures the desired parameters from the initial creation request", now reports a bandwidth that was never part of that creation request.
