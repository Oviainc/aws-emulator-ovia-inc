Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# An unresolvable Direct Connect walk is reported as a complete, empty attach-point page, which the crate's own verdict type says must never share an answer with "none exist"

_Filed: 2026-10-28T12:16:08Z_
_Operation: ListAttachPoints | Class: error-response_

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAttachPoints` resolves the account's attach points through `crate::state::direct_connect::attach_points`, which calls the shared walk and then THROWS AWAY the walk's `complete` flag (`walk_attach_points(ctx, region).await.0`). The walk sets that flag to `false` in two cases: Direct Connect could not be asked at all (the SDK hop errored), and the page bound was reached with a cursor still outstanding. In both cases the handler answers HTTP 200 with `attachPoints: []` (or a silently SHORT list) and `nextToken` absent — i.e. a positive, complete-looking claim that those are all the attach points the account has, for a `@required` output list.

The same crate says, in the same file, that this must not happen: `AttachPointResolution::Unresolvable`'s own doc states the account's gateways being unreadable is "this emulator failing to resolve the account's own state, not a caller naming something that does not exist, so the two are never answered the same way" — and the two sibling handlers that read the SAME walk honour exactly that distinction, answering `InterconnectServerException` for `Unresolvable` and `ResourceNotFoundException` for `NotAnAttachPoint`. So for one and the same unreadable-Direct-Connect condition a caller is told two contradictory things: the listing says the account owns no attach point (200), while `CreateConnection`/`AcceptConnectionProposal` on a gateway identifier say the attach points "could not be resolved" (500). A caller or test that trusts the listing concludes it must create a gateway first and loops forever.

The partial-page half is worse than the empty one: `walk_attach_points` returns the descriptors it has gathered so far together with `complete == false` on the first error, so a failure on page 2 produces a page of page-1 gateways with NO `nextToken`, which tells a paging caller the sequence ended.

Note the counter-evidence, which is why this is an internal contradiction rather than a plain omission: the module doc at direct_connect.rs:14-16 and the `attach_points` doc at :142-146 state the opposite rule for this one reader ("empty when Direct Connect cannot be asked at all", logged at `warn`). One of the two rules stated in this file is broken by the code under it; they cannot both hold, and today the three readers of one walk disagree.

## Evidence

The crate's rule that the two answers must differ — services/service-com-amazonaws-interconnect-interconnect/src/state/direct_connect.rs:104-110:
```
    /// The account's gateways were read, and none of them is that identifier.
    NotAnAttachPoint,
    /// The account's gateways could not be read to the end, so whether the identifier
    /// names one is UNKNOWN. That is this emulator failing to resolve the account's own
    /// state, not a caller naming something that does not exist, so the two are never
    /// answered the same way.
    Unresolvable,
```

The reader that discards the distinction — direct_connect.rs:147-152:
```
pub async fn attach_points(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Vec<AttachPointDescriptor> {
    walk_attach_points(ctx, region).await.0
}
```
where the discarded `.1` is documented at direct_connect.rs:154-157: "the descriptors it gathered, and whether it reached the END of the account's gateways — `false` when Direct Connect could not be asked, and when the page bound was reached with a cursor still outstanding, so the descriptors are not a complete account of what the account holds", and set at direct_connect.rs:172-184:
```
            Err(error) => {
                ::tracing::warn!(... "the account's Direct Connect gateways could not be resolved");
                return (descriptors, false);
            }
```

The handler that publishes it as a complete page — src/ops/list_attach_points.rs:196-222:
```
        // "Lists all Attach Points the caller has access to": ... An
        // account that owns none has no attach point, which is an empty
        // page: a descriptor is never synthesised for a gateway that does not exist.
        let descriptors = crate::state::direct_connect::attach_points(ctx, &region).await;
...
        Ok(ListAttachPointsOutput {
            attach_points,
            next_token,
        })
```
while `ListAttachPointsResponse$attachPoints` is `@required` (`smithy_query(query="shape",shape="ListAttachPointsResponse")`: `"attachPoints": {"target": "AttachPointDescriptorList", "required": true, "doc": "<p>The valid <a>AttachPoint</a></p>"}`), so an empty list is an assertion, not an absent answer.

The two siblings that DO honour the verdict — src/ops/create_connection.rs:361-386:
```
        if let AttachPoint::directConnectGateway(gateway) = &input.attach_point {
            match crate::state::direct_connect::resolve_attach_point(ctx, &region, gateway).await {
                ...NotAnAttachPoint => { return Err(...ResourceNotFoundException...) }
                ...Unresolvable => {
                    return Err(CreateConnectionErrorModeled::InterconnectServerException(
                        InterconnectServerException {
                            message: "the attach points of this account could not be resolved, \
                                      so the requested attach point could not be validated"
```
and the identical mapping in src/ops/accept_connection_proposal.rs:356-385.

Test coverage: NONE. `grep -n 'Unresolvable|could not be resolved|InterconnectServerException' tests/` returns no matches, and tests/all/emulator.rs:79-82 always mounts Direct Connect beside Interconnect, so no test exercises the unreadable-peer path on any of the three readers.

## Steps to Reproduce

1. Mount the Interconnect service on an engine where the Direct Connect service either is not mounted or fails `DescribeDirectConnectGateways` (e.g. register only Interconnect in a `MultiServiceRouter`, unlike tests/all/emulator.rs:73-82 which registers both).
2. Call `ListAttachPoints` with a seeded `environmentId` such as `env-csp-azure-us-east-1`.
3. Observe HTTP 200 with `attachPoints: []` and no `nextToken` — a complete-looking claim that the account holds no attach point, although nothing is known about what it holds.
4. Call `CreateConnection` on the same environment with `attachPoint: {"directConnectGateway": "90392BE3-219C-47FD-BBA5-03DF76D2542A"}`.
5. Observe HTTP 500 `InterconnectServerException` "the attach points of this account could not be resolved, so the requested attach point could not be validated" — the create reports the walk as UNRESOLVABLE for exactly the condition the listing reported as "none exist", which direct_connect.rs:106-110 says must never be answered the same way.
6. Variant for the partial page: make the peer fail only on the second page of gateways; `ListAttachPoints` returns the first page's descriptors with `nextToken` absent, so a paging caller is told the (truncated) sequence ended.
