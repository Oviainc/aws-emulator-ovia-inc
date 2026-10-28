Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# CreateConnection accepts an attachPoint.arn from another Region, contradicting its own region-scope rule

_Filed: 2026-10-28T01:38:20Z_
_Operation: CreateConnection | Class: validation_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConnection`'s store judgement is documented as scoping EVERY connection it resolves to the Region the request addressed, naming the attach-point ARN explicitly. The code does not do that for the `arn` attach point: it calls `crate::state::connection_by_arn(arn)`, which is the one resolution path in the crate that does not use the `visible_in` predicate. Because `connection_by_arn` only requires the stored connection's own ARN to match the supplied text field-for-field, an ARN naming a LIVE connection created through another Region's endpoint resolves successfully — the stored ARN carries that other Region, so the comparison succeeds. A request signed for `eu-west-1` can therefore mint a connection attached to an `arn:aws:interconnect:us-east-1:...:connection/mcc-xxxxxxxx` attach point, even though every other way of reaching that record from `eu-west-1` (GetConnection, UpdateConnection, DeleteConnection, ListConnections, and all three tagging operations) answers as if it did not exist.

The three sibling ARN-identified handlers in this same crate close exactly this hole by calling `crate::state::arn_scope_violation(&parsed, &crate::state::request_region(ctx))` before resolving; `CreateConnection` never calls it, so the `AttachPoint$arn` member — whose target is the SAME `AmazonResourceName` shape those handlers validate — is the only ARN input of the service that escapes the request's scope.

This is distinct from the already-fixed issue "CreateConnection accepts any attachPoint without checking it is an attach point valid for the Environment": that one introduced the `connection_by_arn` resolution; this one is that the resolution it introduced is not scoped to the Region the request addressed, which the handler's own doc claims it is.

## Evidence

Handler doc states the rule — services/service-com-amazonaws-interconnect-interconnect/src/ops/create_connection.rs:152-155:
```
/// `region` is the Region the request addressed, which is the scope every connection
/// this judgement resolves is looked up in: a connection belongs to the Region it was
/// created in, so neither a `clientToken` nor an attach-point ARN of another Region
/// resolves here.
```
The code under it breaks it — create_connection.rs:285-287 (note `region` is not passed):
```
    if let AttachPoint::Arn(arn) = &input.attach_point {
        if store.connection_by_arn(arn).is_none() {
```
The resolver it calls is unscoped — src/state.rs:441-443:
```
    pub fn connection_by_arn(&self, arn: &str) -> Option<&StoredConnection> {
        let id = connection_id_from_arn(arn)?;
        self.connections.get(id).filter(|stored| holds_arn(stored, arn))
```
with src/state.rs:1030-1031: `stored.connection.arn == arn && stored.connection.state != DELETED_CONNECTION_STATE` — no `visible_in`, so the ARN's own Region (the stored one) satisfies it.

The crate's single stated scoping rule — src/state.rs:210-214:
```
/// This service's endpoints are regional (`https://interconnect.{Region}.{dnsSuffix}`),
/// so a connection belongs to the Region it was created in: every read, update and
/// delete issued against another Region's endpoint sees nothing, exactly as it sees
/// nothing for an identifier that was never allocated. ONE predicate, so every
/// resolution path below scopes the store the same way.
```

Sibling handler that DOES apply the request-scope check for the same `AmazonResourceName` shape — src/ops/tag_resource.rs:176-183:
```
        if let Some(message) =
            crate::state::arn_scope_violation(&parsed, &crate::state::request_region(ctx))
        {
            return Err(TagResourceErrorModeled::InterconnectValidationException(
                InterconnectValidationException { message },
            )
            .into());
        }
```
(identical blocks in src/ops/untag_resource.rs:176-183 and src/ops/list_tags_for_resource.rs:182-191).

Test coverage: tests/all/create_connection_attach_point_validation.rs:174-177 only covers "an ARN naming NO connection of another Region" (an unallocated id, which 404s for a different reason); no row passes an ARN naming an EXISTING connection of another Region. tests/all/arn_scope_is_relative_to_the_request.rs:21 limits its table to `ListTagsForResource,TagResource,UntagResource`, so this path is unpinned.

## Steps to Reproduce

1. Create a connection through the us-east-1 endpoint (CreateConnection signed for us-east-1) and note the `connection.arn` it returns, e.g. arn:aws:interconnect:us-east-1:<account>:connection/mcc-abcd1234.
2. Sign a CreateConnection for eu-west-1 with `attachPoint: { "arn": "arn:aws:interconnect:us-east-1:<account>:connection/mcc-abcd1234" }` and an environment/bandwidth that is otherwise valid.
3. Observe HTTP 200: the out-of-Region ARN resolved and a connection was provisioned onto it.
4. Confirm the inconsistency: call ListTagsForResource in eu-west-1 with that same ARN — it is refused HTTP 400 InterconnectValidationException as out of the request's scope; and GetConnection in eu-west-1 for mcc-abcd1234 answers 404.
5. Expected, per create_connection.rs:152-155 and state.rs:210-214: the attach-point ARN of another Region does not resolve here.
