Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: cc767c9c860d2029dd9667f4e637f1644e38e746

# An UntagResource request carrying no tagKeys answers 501 NotImplemented, although the model declares an empty tagKeys list valid and this crate's own rule is that no operation answers 501 any more

_Filed: 2026-10-29T06:25:32Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `cc767c9c860d2029dd9667f4e637f1644e38e746`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` binds `tagKeys` as a `@required` `@httpQuery` member whose target list is `@length(min: 0, max: 200)` — so an EMPTY list is a valid input, and the wire form of an empty query-bound list is a `DELETE /tags/{resourceArn}` with no `tagKeys` parameter at all.

The REST router matches a route only when EVERY required header/query binding is present, so such a request resolves to NO operation. This service keeps the framework default for that case (`override_operation` and `override_routing_error` both return `None`), and for a `rest*` protocol that default is HTTP 501 NotImplemented. The request therefore never reaches the boundary validator (which this crate declares answers an input-constraint failure with the modelled `ValidationException`, HTTP 400) and never reaches the handler (whose `retain` over `tag_keys` would answer a no-op 200 for an empty list).

Two things in this crate contradict that outcome: (1) the handler already computes the right answer for an empty key list, and (2) the crate's own census test states the rule that NO operation of this service answers 501 to a caller any more — yet a modelled operation on a modelled route does exactly that for an input the model permits. `tagKeys` is the only `@required` `@httpQuery` member in this model, so `UntagResource` is the only route affected.

## Evidence

Model — smithy_query(query="operation", operation="UntagResource"): `"tagKeys": {"target": "TagKeyList", "httpQuery": "tagKeys", "required": true, "length": {"min": 0, "max": 200}, "type": "list"}`, errors include `ValidationException (HTTP 400)`. The minimum length is 0, so an empty list is a valid request.

Router (restJson1 shares this matcher) — vendor/smithy-schema-server/src/rest_xml.rs:557-575: "Returns `Some(n)` when **every** required binding is present (`n` being how many there were), or `None` when any is missing — in which case the route must not match at all, because the operation could not be invoked without it." / `RequiredBinding::Query(name) => request_query.iter().any(|&(k, _)| k == *name), ... if !present { return None; }`.

The 501 default — vendor/smithy-server-runtime/src/service.rs:220-229: "Render the framework-level routing failure — the request reached the one claiming service but named no modeled operation ... (query -> 400 InvalidAction text/xml, awsJson -> 404, rpcv2Cbor -> 404, rest -> 501)." This service keeps it: services/service-com-amazonaws-appfabric-fabricfrontendservice/src/service.rs:181-193 `fn override_routing_error(...) { let _ = req; None }` with the comment "Returning `None` renders the protocol-correct default for this service's envelope (query -> 400 InvalidAction text/xml, awsJson -> 404, rpcv2Cbor -> 404, rest -> 501)", and src/service.rs:145-155 `fn override_operation(...) { let _ = req; None }`.

The crate's declared channel for an input-constraint refusal — src/service.rs:48-54: "The error code this service answers an input-constraint violation with: the modelled `ValidationException` (`@httpError 400`, `com.amazonaws.appfabric#ValidationException`)", applied in `override_refusal` at src/service.rs:172-178 (`Refusal::Invalid(violations) => Some(Refused { code: VALIDATION_EXCEPTION ... })`). A request that never routes never gets there.

The crate's own rule that the 501 breaks — tests/all/app_bundle_ingestion_lifecycle.rs:1437-1445: "RULE: no operation of this service is left scaffolded, so none of them answers the scaffold's HTTP 501 any more. ... so an operation landing as, or reverting to, a stub fails here instead of answering 501 to a caller." (The test only scans handler files for the marker, so it cannot see this reachable 501.)

The handler that would have answered — src/ops/untag_resource.rs:151-153: "// Exactly the named keys go, whatever their values and whether the resource held them at all; every other tag keeps its place." / `held.retain(|tag| !input.tag_keys.iter().any(|key| key == &tag.key));` — a no-op success for an empty list.

Test coverage: tests/all/resource_tags.rs:54-67 always builds at least one parameter (`keys.iter().map(|key| format!("tagKeys={}", encode(key))).collect::<Vec<String>>().join("&")`) and every untag case (lines 285-320) names at least one key; tests/all/created_resource_tag_set.rs:326 likewise sends `?tagKeys=env`. No test sends the no-key form, so this answer is unpinned.

## Steps to Reproduce

1. POST /appbundles with {} and note appBundle.arn in the 201 body.
2. POST /tags/<url-encoded bundle arn> with {"tags":[{"key":"env","value":"prod"}]} -> 200.
3. Send DELETE /tags/<url-encoded bundle arn> with NO query string at all — the wire form of tagKeys: [], which the model's @length(min: 0) on TagKeyList permits.
4. Observe HTTP 501 NotImplemented from the routing default, instead of the no-op 200 src/ops/untag_resource.rs:153 computes for an empty key list (or the modelled 400 ValidationException the crate's override_refusal declares for an input-constraint failure).
5. Send DELETE /tags/<url-encoded bundle arn>?tagKeys=env -> 200, showing the only difference is the absent query parameter.
6. GET /tags/<url-encoded bundle arn> to confirm the store was never consulted on the 501 path.
