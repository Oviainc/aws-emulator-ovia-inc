Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2a30e9f7a31e09aa2b907716a7c9b6be0706cfea

# The not-found identifier echo renders the supplied value whole, so NotFoundException$Message runs past its modelled 1000-character bound for ConnectionId and ResourceArn

_Filed: 2026-10-29T06:11:48Z_
_Operation: GetConnection | Class: error-response_

> _Note: this issue was identified against the code base at commit `2a30e9f7a31e09aa2b907716a7c9b6be0706cfea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service words every `NotFoundException` body with the one shared rule `crate::state::identifier_not_found(member, supplied)`, which formats `<member>=<value as supplied>` and, as its own doc says, "never normalises the value". The body it produces is carried in `NotFoundException$Message`, whose target `ErrorMessage` is modelled `length(min: 1, max: 1000)`.

Two input members this crate echoes are modelled LONGER than that ceiling, so a request the boundary validator accepts produces a refusal body the model forbids:

* `GetConnection$ConnectionId` is `length(min: 1, max: 1024)` with pattern `^[a-zA-Z0-9+/=]{1,1024}$`. A 1024-character id that names no stored connection yields `connectionId=` + 1024 = 1037 characters.
* `ResourceArn` on the three tagging operations (`ListTagsForResource`, `TagResource`, `UntagResource`) targets `Arn`, `length(min: 0, max: 1011)`, whose pattern leaves the partition suffix, the region field and the resource-type field unbounded (`arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/...`). A 1011-character ARN that names nothing this store holds yields `resourceArn=` + 1011 = 1023 characters.

No other echoed member can overrun: `SiteId`/`QuoteIdentifier` are capped at 255, `OutpostIdentifier`/`OutpostId` at 180, `AssetId` at 100, `OrderId` at 20, `CapacityTaskId`/`QuoteOptionIdentifier` at 21, `CatalogItemId` at 10. So the fix is a bound on the echo in the one shared helper, not per handler.

The crate's other refusal rule already avoids this trap deliberately — `ValidationException` "never renders the offending value" — and nothing in `state.rs`, in `tests/all/error_message_rules.rs` or in `tests/all/page_token_refusal_shape.rs` states a reason for letting the not-found echo exceed the modelled message length: the rule-A test rows all use short identifiers, so the overrunning case is untested rather than justified.

## Evidence

The modelled ceiling — smithy_query(query="shape", shape="NotFoundException"):
```
"NotFoundException": {"httpError": 404, "members": {"Message": {"target": "ErrorMessage", "length": {"min": 1, "max": 1000}, "pattern": "^[\\S \\n]+$"}}}
```

The two members that exceed it — smithy_query(query="operation", operation="GetConnection,ListTagsForResource"):
```
"GetConnection": {"input": {"ConnectionId": {"target": "ConnectionId", "httpLabel": true, "required": true, "length": {"min": 1, "max": 1024}, "pattern": "^[a-zA-Z0-9+/=]{1,1024}$"}}
"ListTagsForResource": {"input": {"ResourceArn": {"target": "Arn", "httpLabel": true, "required": true, "length": {"min": 0, "max": 1011}, "pattern": "^(arn:aws([a-z-]+)?:outposts:[a-z\\d-]+:\\d{12}:([a-z\\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$"}}
```

The unbounded echo, in the one place the rule lives — services/service-com-amazonaws-outposts-outpostsolafservice/src/state.rs:5380-5392:
```
/// How this service states that an identifier names no resource it holds.
///
/// The body is the identifier ECHO: the name of the input member the identifier
/// arrived in, `=`, and the value EXACTLY as the client sent it — so a caller
/// that sent the ARN form reads its ARN back, not the bare id the service
/// resolved it to, and a caller can parse the identifier it sent out of the
/// message. It is not an English sentence, and it never normalises the value.
///
/// `member` is the member's validation-path name (see [`SITE_ID_MEMBER`]);
/// `supplied` is the raw input member, before [`resolve_id`] touches it.
pub fn identifier_not_found(member: &str, supplied: &str) -> String {
    format!("{member}={supplied}")
}
```
with the two member names that blow the bound — src/state.rs:5457-5459 and src/state.rs:5433-5435:
```
/// The validation-path name of the `ConnectionId` input member an operation names
/// an Outpost-server installation connection by. See [`SITE_ID_MEMBER`].
pub const CONNECTION_ID_MEMBER: &str = "connectionId";
```
```
/// The validation-path name of the `ResourceArn` input member every tagging
/// operation names a resource by. See [`SITE_ID_MEMBER`].
pub const RESOURCE_ARN_MEMBER: &str = "resourceArn";
```

The call sites that reach it with the raw label — src/ops/get_connection.rs:162-172:
```
        let record = store
            .connection(&region, &input.connection_id)
            .ok_or_else(|| {
                GetConnectionErrorModeled::NotFoundException(NotFoundException {
                    // The identifier echo, of the value as supplied.
                    message: Some(crate::state::identifier_not_found(
                        crate::state::CONNECTION_ID_MEMBER,
                        &input.connection_id,
                    )),
                })
            })?;
```
and src/ops/list_tags_for_resource.rs:171-180 (identically in src/ops/tag_resource.rs:169-178 and src/ops/untag_resource.rs:170-179):
```
        let tags = store
            .tags(&region, &resource_type, &resource_id)
            .ok_or_else(|| {
                ListTagsForResourceErrorModeled::NotFoundException(NotFoundException {
                    message: Some(crate::state::identifier_not_found(
                        crate::state::RESOURCE_ARN_MEMBER,
                        &input.resource_arn,
                    )),
                })
            })?;
```

The crate's own contrast — the sibling refusal rule that does NOT render the value, src/state.rs:5368-5378:
```
// How this service words a refused request
//
// Two rules, each applied to every operation in this crate that states that
// class of failure, so no handler can word one its own way:
//
// * `NotFoundException` — the body is the IDENTIFIER ECHO
//   `<member>=<value as supplied>`. See [`identifier_not_found`].
// * `ValidationException` — the body is the standard AWS validation report, and
//   it never renders the offending value. See [`validation_report`].
```

Test coverage today: tests/all/error_message_rules.rs:7-12 states rule A ("the body is the IDENTIFIER ECHO `<member>=<value as supplied>` … the value is what the client sent, verbatim") and its rows use short identifiers only — e.g. the longest, tests/all/error_message_rules.rs:57-61:
```
const FOREIGN_SITE_ARN: &str =
    "arn:aws-us-gov:outposts:us-gov-west-1:123456789012:site/os-11111111111111111";
const FOREIGN_OUTPOST_ARN: &str =
    "arn:aws-cn:outposts:cn-north-1:123456789012:outpost/op-2222222222222222a";
```
No case anywhere in the suite sends a 1011/1024-character identifier, so the overrun is untested and nothing states a reason for it.

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. GET /connections/<id> where <id> is 1024 characters drawn from [a-zA-Z0-9+/=] (e.g. "A".repeat(1024)) — a value the modelled ConnectionId length 1..1024 and pattern ^[a-zA-Z0-9+/=]{1,1024}$ both accept, so it reaches the handler.
3. Observe HTTP 404 NotFoundException with Message = "connectionId=" + the 1024 characters = 1037 characters, although NotFoundException$Message targets ErrorMessage with length(min: 1, max: 1000).
4. Same overrun on the tagging route: GET /tags/<arn> with arn = "arn:aws:outposts:" + "a".repeat(958) + ":123456789012:widget/os-11111111111111111" padded to exactly 1011 characters (the modelled Arn maximum, which its pattern admits because the region and resource-type fields are unbounded).
5. Observe HTTP 404 with Message = "resourceArn=" + the 1011 characters = 1023 characters — again past the modelled 1000.
6. Contrast with the sibling rule: ask any paged list with NextToken="1##outposts" on a route that did not mint it and the 400 ValidationException body carries the fixed report and no caller value at all (tests/all/page_token_refusal_shape.rs:87-92), which is how this crate keeps that message inside the same ErrorMessage bound.
