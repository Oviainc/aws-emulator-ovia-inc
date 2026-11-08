Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 424f227b7e616b3d6e5534953489c6aa083295fd

# A connection that does not exist answers 400 when addressed by identifier but 404 when addressed by activationKey

_Filed: 2026-11-08T07:49:48Z_
_Operation: DescribeConnectionProposal | Class: error-response_

> _Note: this issue was identified against the code base at commit `424f227b7e616b3d6e5534953489c6aa083295fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

One and the same stored record — a `StoredConnection` in this crate's own `connections` map — is reachable by two handles: its `ConnectionId` (GetConnection/DeleteConnection/UpdateConnection) and its `activationKey` (DescribeConnectionProposal/AcceptConnectionProposal). When the handle resolves to nothing, the three identifier-addressed handlers deliberately override the modelled status and answer the crate's `CLIENT_FAULT_STATUS` (400) while keeping the `ResourceNotFoundException` shape, and the two key-addressed handlers return the same shape as a plain `Modeled` error, so it serializes with the model's `@httpError` 404.

The crate states the rule it breaks in two places. `CLIENT_FAULT_STATUS`'s own doc says 400 "is the status a refused request is answered with whatever shape names the fault", and the `ClientFault` variant's doc names this exact case as the one it exists for ("a request naming a resource the account does not hold, say"). The ARN-addressed suite's rule statement says the same thing for the other handle on the same record ("Neither class is ever the connection-scoped 404"). Even the refusal wording comes from one template family in `state.rs` — `connection_not_retrievable_message` and `proposal_not_retrievable_message` are the same sentence with a different subject — yet one is serialized 400 and the other 404.

So a client that branches on the HTTP status before reading the error code sees two different statuses for "this connection is not here", decided only by which handle it used. The two sites are `describe_connection_proposal.rs:231-240` and the proposal lookup in `accept_connection_proposal.rs:273-282`; both are the same rule and a single fix covers them.

## Evidence

The identifier-addressed read overrides the modelled status, and says why — services/service-com-amazonaws-interconnect-interconnect/src/ops/get_connection.rs:157-169:
```
        // An identifier that names no connection of this endpoint is a fault in the
        // REQUEST, so it is answered with the status this service gives a client fault
        // (`crate::scaffolded::CLIENT_FAULT_STATUS`) while keeping the modeled
        // connection-scoped error shape and its request-derived wording.
        let Some(connection) = found else {
            return Err(crate::scaffolded::Scaffolded::client_fault(
                GetConnectionErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    message: Some(crate::state::connection_not_retrievable_message(
                        &input.identifier,
                    )),
                }),
            ));
        };
```
The same crate, same store, same message family, no override — services/service-com-amazonaws-interconnect-interconnect/src/ops/describe_connection_proposal.rs:226-240:
```
        // A key that resolves to no stored proposal — one never issued, one issued
        // through another Region's endpoint, or one naming a connection a
        // `DeleteConnection` has been made on — is the modelled 404: "the request
        // specifies a resource that does not exist on the server". `ActivationKey` is
        // `@sensitive`, so the refusal names the member, never the key's value.
        let Some(output) = proposal else {
            return Err(
                DescribeConnectionProposalErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::proposal_not_retrievable_message()),
                    },
                )
                .into(),
            );
        };
```
and the sibling site on the other key-addressed operation — src/ops/accept_connection_proposal.rs:273-282:
```
        let Some(stored) = stored else {
            return Err(
                AcceptConnectionProposalErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::proposal_not_retrievable_message()),
                    },
                )
                .into(),
            );
        };
```
The rule the crate states for this status — src/scaffolded.rs:180-203:
```
/// The HTTP status this service answers a fault in the REQUEST with.
///
/// Every client fault this service models carries it (`InterconnectClientException`,
/// `InterconnectValidationException`), and it is the status a refused request is
/// answered with whatever shape names the fault.
pub const CLIENT_FAULT_STATUS: u16 = 400;
...
    /// Reach for this where the service answers a client's fault with its own 400 even
    /// though the shape it names is modeled with another status — a request naming a
    /// resource the account does not hold, say.
    ClientFault(E),
```
and `http_status()` is what makes the two sites differ — src/scaffolded.rs:307-313:
```
    fn http_status(&self) -> u16 {
        match self {
            Self::NotImplemented(_) => 501,
            Self::Modeled(e) => e.http_status(),
            Self::ClientFault(_) | Self::ValidationException(_) => CLIENT_FAULT_STATUS,
        }
    }
```
with `ResourceNotFoundException` modelled `httpError: 404` (smithy_query query="operation", operation="GetConnection": `{"shape": "ResourceNotFoundException", "errorKind": "client", "httpError": 404}`).

The two refusals are one sentence family — src/state.rs:1125-1153:
```
/// How a connection READ (`GetConnection`, `DeleteConnection`) words an identifier
/// that names no stored connection: the retrieval could not be served.
pub fn connection_not_retrievable_message(identifier: &str) -> String {
    format!("{REFUSAL_ENVELOPE}Unable to retrieve connection {identifier}, as it does not exist")
}
...
pub fn proposal_not_retrievable_message() -> String {
    format!(
        "{REFUSAL_ENVELOPE}Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist"
    )
}
```
The other handle on the same record is pinned to "never the 404" — tests/all/arn_scope_refusals.rs:13-17:
```
//! shape the ARN arrived in. An ARN of that scope which resolves to no
//! resource is not a malformed request at all: the two tag WRITES refuse it as this
//! service's own client fault (HTTP 400 `InterconnectClientException`, worded with a
//! fixed sentence carrying nothing of the ARN), and the `@readonly` LISTING answers
//! HTTP 200 with the empty tag set. Neither class is ever the connection-scoped 404.
```
Test coverage: tests/all/connection_proposal.rs and tests/all/accept_connection_proposal.rs exercise the unknown-key refusal (code and message), tests/all/client_fault_status_and_validation_wording.rs pins the 400 the identifier-addressed refusals carry; no test pins the status of the key-addressed refusal, which is why the divergence survives.

## Steps to Reproduce

1. Boot the emulator and point an Interconnect client at it.
2. Call GetConnection(identifier="mcc-00000000") — an id that names no connection. Observe error code ResourceNotFoundException with HTTP status 400 (ResponseMetadata.HTTPStatusCode), per src/ops/get_connection.rs:162 `Scaffolded::client_fault`.
3. Call DescribeConnectionProposal(activationKey="QUFBQQ==") — well-formed base64 that no CreateConnection ever minted. Observe the same error shape ResourceNotFoundException, with the sister sentence "Unable to retrieve a connection proposal for the supplied activationKey, as it does not exist", but HTTP status 404.
4. Call AcceptConnectionProposal(activationKey="QUFBQQ==", attachPoint={"directConnectGateway": "12345678-1234-1234-1234-123456789012"}) — same key. Observe HTTP 404 again.
5. Compare with the ARN handle on the same record: TagResource with an in-scope ARN naming no connection answers HTTP 400 (tests/all/arn_scope_refusals.rs). One absent record, three handles, two statuses.
