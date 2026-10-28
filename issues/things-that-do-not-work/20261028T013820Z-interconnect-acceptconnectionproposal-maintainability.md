Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# The Direct Connect gateway attach-point resolution verdict is mapped to refusals twice, once per handler

_Filed: 2026-10-28T01:38:20Z_
_Operation: AcceptConnectionProposal | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "how an `AttachPointResolution` becomes a response" — Valid continues, NotAnAttachPoint is the modelled 404 worded by `attach_point_not_valid_message`, Unresolvable is the modelled 500 with a fixed sentence — is written out twice, once in `create_connection.rs` and once in `accept_connection_proposal.rs`, as the same three-arm match over the same helper call. The 500's message string is also spelled out verbatim in both copies, whereas every other client-facing refusal in this crate is produced by a single `crate::state::*_message` helper so the wording cannot drift (`attach_point_not_valid_message`, `connection_not_retrievable_message`, `environment_not_found_message`, `proposal_not_retrievable_message`, …). Because the two copies are independent, a change to the classification or to the sentence in one handler silently leaves the other behind — the same drift risk the already-fixed bandwidth-rule maintainability issue removed by giving both writers one `bandwidth_availability_violation` helper. The crate already owns the shared half (`crate::state::direct_connect::resolve_attach_point` and the `AttachPointResolution` enum); only the verdict-to-refusal mapping and the message literal are duplicated. Collapsing them (a state-module helper returning the resolution's message, or the message literal alone) changes no response and no test.

## Evidence

Copy 1 — services/service-com-amazonaws-interconnect-interconnect/src/ops/create_connection.rs:357-382:
```
        if let AttachPoint::directConnectGateway(gateway) = &input.attach_point {
            match crate::state::direct_connect::resolve_attach_point(ctx, &region, gateway).await {
                crate::state::direct_connect::AttachPointResolution::Valid => {}
                crate::state::direct_connect::AttachPointResolution::NotAnAttachPoint => {
                    return Err(CreateConnectionErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: Some(crate::state::attach_point_not_valid_message(
                                gateway,
                                &environment.environment_id,
                            )),
                        },
                    )
                    .into())
                }
                crate::state::direct_connect::AttachPointResolution::Unresolvable => {
                    return Err(CreateConnectionErrorModeled::InterconnectServerException(
                        InterconnectServerException {
                            message: "the attach points of this account could not be resolved, \
                                      so the requested attach point could not be validated"
                                .to_string(),
                        },
                    )
                    .into())
                }
            }
        }
```
Copy 2 — src/ops/accept_connection_proposal.rs:354-383: the same three arms over the same `resolve_attach_point(ctx, &region, gateway)` call, with the identical 500 literal at accept_connection_proposal.rs:374-376:
```
                                message: "the attach points of this account could not be resolved, \
                                          so the requested attach point could not be validated"
                                    .to_string(),
```
The crate's own stated convention for refusal wording — src/state.rs:715-721:
```
/// The envelope this service puts on the client-facing refusals it words itself.
...
/// this one constant rather than each spelling the prefix out.
pub const REFUSAL_ENVELOPE: &str = "Unable to process request: ";
```

Existing tests that pin both copies' behaviour and must stay unchanged: tests/all/create_connection_attach_point_validation.rs (`an_attach_point_that_resolves_to_nothing_is_refused_and_provisions_nothing`, `every_listed_attach_point_is_accepted_and_read_back`), tests/all/refusals_and_gateway_attach_points.rs, tests/all/accept_connection_proposal.rs, tests/all/caller_created_gateways_are_attach_points.rs.

## Steps to Reproduce

1. Open src/ops/create_connection.rs at lines 357-382 and src/ops/accept_connection_proposal.rs at lines 354-383 side by side.
2. Observe the same three-arm match over `crate::state::direct_connect::resolve_attach_point`, with the same 404 wording helper and the same hard-coded 500 sentence in both files.
3. Change the 500 sentence in one file only and run the test suite: it still passes, showing the two copies can drift apart undetected.
4. Expected: the verdict-to-refusal mapping and its message live once in the crate (as `bandwidth_availability_violation` and the `*_message` helpers already do) with both handlers calling it.
