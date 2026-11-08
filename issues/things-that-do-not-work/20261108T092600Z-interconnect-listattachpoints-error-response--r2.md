Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 54585fe1fcb32b6141e5d520142388c808238c6f

# ListAttachPoints resolves the environmentId resource before judging its own nextToken, so a resource verdict masks a request fault

_Filed: 2026-11-08T09:26:00Z_
_Operation: ListAttachPoints | Class: error-response_

> _Note: this issue was identified against the code base at commit `54585fe1fcb32b6141e5d520142388c808238c6f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate judges what the REQUEST itself carries before it resolves anything the request NAMES, and the crate explicitly puts a forged pagination cursor in that "fault in the request" class — a cursor is either one this endpoint issued or it is not, independently of the store. `ListAttachPoints` inverts that order: it resolves `environmentId` against the called Region's Environment catalog FIRST (and refuses an unresolvable id with the 400 `InterconnectClientException` it calls "this service's own client fault"), and only afterwards decodes `nextToken`.

So a request that carries BOTH an `environmentId` the called Region's catalog does not hold AND a `nextToken` this operation never issued is answered with the resource verdict (`InterconnectClientException`, "This environment model with id … could not be found"), and the input fault (`InterconnectValidationException`, "nextToken is not a pagination token issued by ListAttachPoints") never surfaces — exactly the masking the crate's stated rule forbids. Moving the cursor decode above the catalog lookup is all that is needed; neither check depends on the other.

The existing test rows exercise the two faults one at a time only, never together, which is why the inversion survives.

## Evidence

The rule, stated by this crate for the cursor class — services/service-com-amazonaws-interconnect-interconnect/src/ops/list_connections.rs:198-202:
```
        // connection means "nothing of yours is on that Environment" and is never a refusal. The two
        // unions above are a different case for a different reason — what they carry is
        // outside the MEMBER's own modelled constraints, which is a fault in the request
        // however the store looks — and so is the cursor below, which this endpoint
        // either issued or did not.
```
and the ordering rule itself — src/ops/create_connection.rs:403-405:
```
        // What the request itself carries is judged BEFORE anything it names is
        // resolved, so no verdict about a resource can mask a fault in the input.
        judge_request(&input)?;
```
repeated verbatim at src/ops/accept_connection_proposal.rs:440-442:
```
        // What the request itself carries is judged BEFORE anything it names is
        // resolved, so no verdict about a resource can mask a fault in the input.
        judge_request(&input)?;
```
and stated as a service-wide RULE naming this very resource verdict — tests/all/request_faults_precede_resource_verdicts.rs:7-16:
```
//! RULE: a fault the REQUEST itself carries ... is refused as invalid
//! input (the modelled `InterconnectValidationException`, HTTP 400, worded for that
//! fault) BEFORE anything the request NAMES is resolved, so no verdict about a resource
//! stands in its place: not a handle that resolves to nothing, not a proposal whose
//! acceptance has already been made, not a connection an acceptance can no longer be
//! made on, not an `environmentId` the called Region does not offer and not a
//! `clientToken` already used for another request.
```

The breach — src/ops/list_attach_points.rs:162-170, the resource verdict first:
```
        let Some(environment) = crate::state::environment_in_region(&region, &input.environment_id)
        else {
            return Err(ListAttachPointsErrorModeled::InterconnectClientException(
                InterconnectClientException {
                    message: crate::state::environment_not_found_message(&input.environment_id),
                },
            )
            .into());
        };
```
and only then, at src/ops/list_attach_points.rs:172-187, the request's own fault:
```
        // A cursor this operation did not issue is refused, rather than answered
        // with an empty page the caller cannot tell from "no more results".
        let cursor = match input.next_token.as_deref() {
            Some(token) => {
                let Some(key) =
                    crate::state::decode_cursor(token, crate::state::ATTACH_POINT_CURSOR_TAG)
                else {
                    return Err(ListAttachPointsErrorModeled::InterconnectValidationException(
                        InterconnectValidationException {
                            message:
                                "nextToken is not a pagination token issued by ListAttachPoints"
                                    .to_string(),
```

Test coverage: tests/all/attach_point_listing_needs_a_resolved_account.rs:228-247 exercises the two faults as SEPARATE rows ("an environment id no catalog holds" -> `InterconnectClientException`; "a nextToken this operation did not issue" -> `InterconnectValidationException`), with `token: None` on the environment rows and a valid `environment_id` on the token row — so no row sends both and nothing pins the precedence.

## Steps to Reproduce

1. Boot the emulator and point an Interconnect client at it (region us-east-1).
2. Call ListEnvironments to learn the published environmentId for the region (e.g. mce-aws-gcp-iad).
3. Call ListAttachPoints(environmentId="mce-aws-gcp-iad", nextToken="not-a-token-this-service-minted"). Observe HTTP 400 InterconnectValidationException "nextToken is not a pagination token issued by ListAttachPoints" — the request fault, as expected.
4. Call ListAttachPoints(environmentId="mce-aws-nobody-nowhere", nextToken="not-a-token-this-service-minted") — both faults in one request.
5. Observe HTTP 400 InterconnectClientException "Unable to process request: This environment model with id mce-aws-nobody-nowhere could not be found": the resource verdict stands in place of the input fault, which src/ops/create_connection.rs:403-405 and tests/all/request_faults_precede_resource_verdicts.rs:7-16 say must never happen.
6. Compare CreateConnection(environmentId="mce-aws-nobody-nowhere", attachPoint={"directConnectGateway": "not-a-uuid"}, …), which answers the input fault (InterconnectValidationException naming the malformed gateway id) rather than the environment verdict.
