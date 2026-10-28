Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0f071a90384ece90fe46e5865745f415e3851c94

# The duplicate-endpoint ConflictException is hand-rolled and discloses the stored endpoint's vpcEndpointId, which this crate's one conflict rule forbids

_Filed: 2026-10-28T20:30:39Z_
_Operation: CreatePrivateGraphEndpoint | Class: error-response_

> _Note: this issue was identified against the code base at commit `0f071a90384ece90fe46e5865745f415e3851c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other conflict refusal in this crate is built by a single shared builder in `crate::state`, and the crate states twice over that such a refusal is derived from what the CALLER sent and discloses nothing about the record it collided with. CreatePrivateGraphEndpoint's "a second endpoint in this VPC" refusal is the one conflict in the service that is formatted inline in the handler, and the sentence it formats ends by naming the stored endpoint's minted `vpcEndpointId` — an identifier the request never addressed and the caller was never told. It also does not follow the crate's own precondition-refusal grammar (`Unable to <action> because <the record> <condition>.`), so the same client that can match every other 409 of this service cannot match this one.

## Evidence

The rule, stated in the one builder every other conflict reaches — src/state.rs:2825-2831: "The message is derived from the REQUESTED name and nothing else: the conflict is a statement about the name the caller sent, and the record that already holds it is a resource this request never addressed, so its identifier is not reported. Written once here and reached by every name-uniqueness check in the service, so no two of them word one refusal differently or disclose different amounts about the record they collided with."

The same rule for this very resource kind — src/state.rs:3047-3056, `private_graph_endpoint_state_conflict(...)` is built by `precondition_conflict(...)` whose contract the crate's suite states at tests/all/precondition_refusal_grammar.rs:6-12: "a `ConflictException` raised because a stored record did not meet a precondition states WHAT THE CALL COULD NOT DO and THE CONDITION that held it back — `Unable to <action> because <the record> <condition>.` — and nothing else: every one of these calls addressed its record by an identifier the caller sent, so no refusal fronts one."

The code that breaks it — src/ops/create_private_graph_endpoint.rs:216-228:
```
if let Some(existing) = store.settled_private_endpoint(&graph_id, &vpc_id, now) {
    return Err(CreatePrivateGraphEndpointErrorModeled::ConflictException(
        ConflictException {
            message: format!(
                "graph {graph_id} already has a private graph endpoint in vpc \
                 {vpc_id} with vpc endpoint id {}",
                existing.vpc_endpoint_id
            ),
```
`existing.vpc_endpoint_id` is the id this service minted for the OTHER record (src/state.rs:1606-1614 `next_vpc_endpoint_id`), not anything the request carried.

This is the same defect two FIXED issues of this crate already removed from the other two hand-rolled duplicate conflicts: 20261024T161935Z-neptune-graph-creategraphsnapshot-error-response.md ("Duplicate snapshotName conflict uses an emulator-authored message that also discloses the colliding snapshot's id") and 20261024T153312Z-neptune-graph-creategraphusingimporttask-error-response.md; the private-endpoint surface was not covered by either.

Test coverage: tests/all/private_graph_endpoint_lifecycle.rs:399-400 asserts only the 409 status and the `ConflictException` code for a second endpoint in the same VPC — the message text is unasserted, so rewording it through the shared builder changes no existing assertion.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName='endpoint-conflict-probe', provisionedMemory=16); poll GetGraph until status AVAILABLE.
3. CreatePrivateGraphEndpoint(graphIdentifier=<id>, vpcId='vpc-0abc', subnetIds=['subnet-1']) -> 201; note the returned vpcEndpointId.
4. Call CreatePrivateGraphEndpoint again with the SAME graphIdentifier and vpcId.
5. Observe 409 ConflictException whose message is "graph g-... already has a private graph endpoint in vpc vpc-0abc with vpc endpoint id vpce-..." — an emulator-authored sentence outside this crate's refusal grammar that also discloses the stored endpoint's minted vpcEndpointId, instead of a refusal built from the identifiers the request sent by the crate's one conflict builder.
