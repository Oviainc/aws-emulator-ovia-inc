Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# A nextToken this service never issued is served as a page: any hex-encoded 26-character identifier is accepted as a cursor, including one the OTHER list operation issued

_Filed: 2026-10-28T01:41:06Z_
_Operation: ListClusters | Class: pagination_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PageBounds::resolve` refuses a `nextToken` only when it fails to decode to the SHAPE of a resource identifier. Any hex encoding of a 26-character `[a-z0-9]` string is accepted and used as "resume after this key", even though no page of either list operation ever issued it. Two consequences:

* a client can fabricate a cursor (`hex("00000000000000000000000000")`) and ListClusters/ListStreams answer 200 with a page starting after an arbitrary key, instead of the refusal the crate documents and words for exactly this case;
* the cursor carries no binding to the operation that issued it, so a `nextToken` ListClusters issued (a cluster identifier) is accepted by ListStreams as a stream cursor and silently skips every stream whose identifier sorts below that cluster id — while the refusal this crate words says the value must be "a nextToken a previous page of THIS OPERATION returned".

The decoder's own doc states the stricter rule the code does not implement, and the existing refusal test enumerates "every form a value can fail to be a cursor" without the well-formed-but-never-issued form, so the case is both claimed and uncovered.

## Evidence

The decoder claims to refuse anything unissued:

`services/service-com-amazonaws-dsql-dsql/src/state.rs:1390-1400`
```
/// Decode a cursor produced by [`encode_pagination_cursor`]: a hex-encoded
/// 26-character resource identifier.
///
/// `None` for anything this service did not issue — not only a token that is not
/// hex, but also one that decodes to anything other than a value
/// [`is_resource_identifier`] accepts, so a fabricated cursor is refused rather
/// than served as a page starting after some arbitrary key.
pub fn decode_pagination_cursor(token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    let cursor = String::from_utf8(decoded).ok()?;
    is_resource_identifier(&cursor).then_some(cursor)
}
```
...but `is_resource_identifier` only checks the SHAPE:

`services/service-com-amazonaws-dsql-dsql/src/state.rs:1372-1377`
```
pub fn is_resource_identifier(value: &str) -> bool {
    value.len() == RESOURCE_ID_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}
```
The refusal the handlers would have given states the rule the code does not enforce:

`services/service-com-amazonaws-dsql-dsql/src/state.rs:2076-2082`
```
pub fn unissued_pagination_token(value: &str) -> ValidationException {
    invalid_member_value(
        NEXT_TOKEN_MEMBER,
        "must be a nextToken a previous page of this operation returned",
        value,
    )
}
```
Both readers resolve the cursor through it and treat whatever it returns as a resume key:

`services/service-com-amazonaws-dsql-dsql/src/ops/list_clusters.rs:164-175` ("A `nextToken` this service never issued is refused with the service's OWN `ValidationException` ... decided before any cluster is read") and `services/service-com-amazonaws-dsql-dsql/src/ops/list_streams.rs:174-179`.

The existing test's table omits the form: `services/service-com-amazonaws-dsql-dsql/tests/all/pagination_token_refusal.rs:54-83`
```
/// Values this service never issued as a cursor, spanning every form the failure
/// takes: not hex at all (so it cannot decode), and hex of something that is not
/// the `^[a-z0-9]{26}$` identifier a cursor encodes (wrong case, one character
/// short, one character long, not UTF-8 at all, the empty string).
```
Every entry is mis-shaped; none is a well-formed identifier the service never handed out.

## Steps to Reproduce

1. Create two or more clusters so ListClusters has something to page.
2. Call `GET /cluster?next-token=<hex of "00000000000000000000000000">` (a cursor no page ever issued). Expected per `state.rs:1390-1400` and `unissued_pagination_token`: 400 ValidationException naming `nextToken`. Actual: 200 with a page of clusters.
3. Call `GET /cluster?max-results=1` and keep the `nextToken` it returns (the hex of a CLUSTER identifier).
4. Pass that token to the other reader: `GET /stream/{clusterIdentifier}?next-token=<that token>`. Actual: 200, the token is used as a stream cursor and streams whose identifier sorts below the cluster id are skipped — although the refusal text promises only tokens "a previous page of this operation returned" are served.
