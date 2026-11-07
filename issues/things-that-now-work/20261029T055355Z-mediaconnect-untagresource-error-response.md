Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 12966d24554ed06eeaba7911ae25ff590cd06862

# An UntagResource request with no `tagKeys` query parameter answers 501 instead of the modelled BadRequestException the crate says an absent required member gets

_Filed: 2026-10-29T05:53:55Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `12966d24554ed06eeaba7911ae25ff590cd06862`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResourceRequest$TagKeys` is `@required` and bound to the query string (`@httpQuery("tagKeys")`), so the generated REST route for `DELETE /tags/{ResourceArn}` carries `RequiredBinding::Query("tagKeys")` and the shared route lookup rejects the route when the parameter is absent. A `DELETE /tags/<arn>` with no query string resolves to no operation: `src/ops/untag_resource.rs::handle` never runs, `override_routing_error` returns `None`, and the router renders the restJson1 routing default — HTTP 501 NotImplemented.

This contradicts the rule this crate writes down for exactly this case: `src/service.rs` states that a request the framework refuses before any handler runs — "a required member absent" — is answered with `BadRequestException` (HTTP 400) in this service's one `Validation failed:` envelope, and tests/all/error_envelopes.rs asserts that answer for the body-bound twin ("the required Name member absent from a create"). 501 also misreports an implemented operation as unimplemented. `UntagGlobalResourceRequest$TagKeys` carries the identical `@required` + `@httpQuery("tagKeys")` pair, so `DELETE /tags/global/<arn>` with no query answers 501 the same way; the fix belongs in the one documented seam in `src/service.rs` (`override_operation` / `override_routing_error`, both currently `None`), which covers both tagging paths at once.

## Evidence

Model — `smithy_query(query="shape", shape="UntagResourceRequest")`:
```
"TagKeys": {"target": "__listOfString", "httpQuery": "tagKeys", "clientOptional": true, "required": true, "type": "list"}
```
with `BadRequestException (HTTP 400)` among the operation's errors (`UntagGlobalResourceRequest$TagKeys` is identical).

Side 1 — the rule this service states for an absent required member, src/service.rs:160-192:
```
            // request the framework refuses before any handler runs (a required member
            // absent, a value outside a declared bound, an ARN failing its modelled
            // pattern) is worded with the code a MediaConnect client branches on, and the
            // message states each violation the framework found.
...
                Some(Refused {
                    code: "BadRequestException".to_string(),
                    message: crate::state::validation_failed_message(&detail),
                })
```
Side 2 — what is rendered instead, src/service.rs:198-210:
```
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
```
and the crate's own test pinning the 400 for the body-bound case, tests/all/error_envelopes.rs:657-665 + 745-749:
```
        RefusalCase {
            case: "the required Name member absent from a create",
            at_the_boundary: true,
...
            (400, Some("BadRequestException")),
```

Why the route is rejected (cited, not filed against): vendor/smithy-macros/src/lib.rs:599-612 turns every `@required` + `@httpQuery` member into `RequiredBinding::Query(..)`; vendor/smithy-schema-server/src/rest_xml.rs:367-369 — "a candidate route is **rejected** when any of its [`RequiredBinding`]s is absent from the request" — implemented at rest_xml.rs:561-575 (`if !present { return None; }`); rest_xml.rs:378-380 says that lookup is shared with `RestJsonServerProtocol`, this service's protocol.

The handler that never runs: src/ops/untag_resource.rs:168 (`async fn handle`), which would otherwise answer through `store.tags_of_mut(&input.resource_arn)` at src/ops/untag_resource.rs:183-192.

Test coverage: the untagging suites drive every other removal shape — one carried key, an absent key, a repeat, two keys, a non-ASCII key, a URI-significant key, a key at the documented maximum length (tests/all/global_resource_untagging.rs:107-150) — and the request helper even carries an unused empty-query branch (tests/all/global_resource_untagging.rs:56-60: `let uri = if query.is_empty() { path } else { format!("{path}?{query}") };`), but no row sends a removal with no `tagKeys` at all, so nothing pins the current 501.

## Steps to Reproduce

1. Create a flow: POST /v1/flows with {"name":"untag-route","source":{"name":"s","protocol":"rtp","whitelistCidr":"10.0.0.0/16"}}; note flowArn.
2. Tag it: POST /tags/<percent-encoded flowArn> with {"tags":{"env":"prod"}} -> 204.
3. Send DELETE /tags/<percent-encoded flowArn> with NO query string at all.
4. Observe HTTP 501: the route carries `RequiredBinding::Query("tagKeys")`, so the route lookup rejects it and `UntagResource::handle` never runs.
5. Expected per the model and src/service.rs:160-192: HTTP 400 with `__type` `BadRequestException` and a `Validation failed:` message naming the absent required member.
6. Contrast with DELETE /tags/<flowArn>?tagKeys=absent-one, which routes and answers 204; and repeat step 3 against DELETE /tags/global/<routerInputArn> for the same 501.
