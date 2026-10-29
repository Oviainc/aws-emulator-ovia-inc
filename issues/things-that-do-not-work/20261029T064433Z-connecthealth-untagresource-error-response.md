Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# An UntagResource request carrying no tagKeys answers 501 NotImplemented, although the crate's own test states no operation of this service can answer 501 over HTTP any more

_Filed: 2026-10-29T06:44:33Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` binds `tagKeys` as a `@required` `@httpQuery` member. The generated REST route therefore carries a `RequiredBinding::Query("tagKeys")`, and the shared REST router REJECTS a candidate route whose required query binding is absent — so `DELETE /tags/{resourceArn}` with no `tagKeys` parameter resolves to NO operation at all.

This crate keeps the framework default for an unrouted request (`override_operation` claims a request only when the routing table can serve it, and `override_routing_error` returns `None`), and for restJson1 that default is HTTP 501 `NotImplemented`. The consequences, all of them inside this crate:

1. The request never reaches the boundary validator, so the modelled refusal for an absent `@required` member is never rendered, and it never reaches the handler, whose own 400 `ValidationException` wording for a key this service does not accept (`removed_key_violation`) is therefore unreachable for the absent-member case.
2. A fully IMPLEMENTED operation answers the scaffold's "nobody has written this yet" signal. `src/scaffolded.rs` states that claim is a different one and must not be used for a bad request.
3. It contradicts the rule the crate's own HTTP test writes down: "this service has no scaffolded operation left, so the framework's own 501 body is unreachable over HTTP here" — the test then deliberately sends `?tagKeys=env`, with a comment noting the member is required, so the no-key form is never exercised.

The error_detail wrapper cannot repair it either: its own module doc says a failure answered before any operation is served "is not reached from here, because this layer is an operation wrapper", so the 501 body is not even renamed into this service's `Message` member. `tagKeys` is the only `@required` `@httpQuery`/`@httpHeader` member on any WIRED operation of this model (`StartMedicalScribeListeningSession`'s required headers belong to the operation the generator skipped), so `UntagResource` is the only affected route.

## Evidence

Model — `smithy_query(query="shape", shape="UntagResourceInput")`:
```
"tagKeys": {"target": "TagKeyList", "httpQuery": "tagKeys", "required": true, "length": {"min": 1, "max": 50}, "type": "list", "doc": "<p>The tag keys to remove from the resource</p>"}
```

The route table marks it a routing discriminator — vendor/smithy-macros/src/lib.rs:4046-4066 and 568-577:
```
            // Every `@required` member bound to a header or a query parameter is
            // a routing discriminator: ... Handing the router the model's
            // required-binding set lets it reject a candidate whose required
            // header/query is absent
            let required_bindings = required_http_bindings(op_shape, &model);
```
```
/// Only members that are BOTH `@required` and bound to a header (`@httpHeader`) or a query parameter
/// (`@httpQuery`) qualify
```

The router drops the route when it is absent (restJson1 routes through the same matcher) — vendor/smithy-schema-server/src/rest_xml.rs:554-575 and 378-380:
```
/// Returns `Some(n)` when **every** required binding is present (`n` being
/// how many there were), or `None` when any is missing — in which case the
/// route must not match at all, because the operation could not be invoked
/// without it.
...
            RequiredBinding::Query(name) => request_query.iter().any(|&(k, _)| k == *name),
        };
        if !present {
            return None;
```
```
/// Shared by [`RestXmlServerProtocol`] and
/// [`crate::rest_json::RestJsonServerProtocol`] — both REST protocols route
/// identically off the URI pattern table; only the body codec differs.
```

What an unrouted restJson1 request is answered with — vendor/smithy-server-runtime/src/pre_dispatch.rs:30:
```
//! | awsJson / restJson1 | **501** `NotImplemented` | no (falls to `InternalServerError`) | caller-supplied |
```

This service keeps that default — services/service-com-amazonaws-connecthealth-connecthealth/src/service.rs:101-110 and 126-138:
```
        error_detail::routes(&self.entry, req).then_some(&self.error_detail as _)
```
```
        // Returning `None` renders the protocol-correct default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
```

The rule this breaks, written by the crate's own HTTP test — services/service-com-amazonaws-connecthealth-connecthealth/tests/all/error_detail_member.rs:347-364:
```
    // Every operation at the ARN-addressed tagging URI renders its refusal through the
    // same wrapper, whichever method the request used: this service has no scaffolded
    // operation left, so the framework's own 501 body is unreachable over HTTP here and
    // the rename of that body is pinned by the unit test beside the wrapper
    // (`src/service/error_detail.rs`).
...
    // `tagKeys` is a required `@httpQuery` list of 1..50 keys
    // (`smithy_query(query="operation",operation="UntagResource")`), so a removal names at
    // least one.
...
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{encoded}?tagKeys=env"),
        ),
```

The refusal the handler would have worded, now unreachable for this request — src/ops/untag_resource.rs:208-212:
```
        if let Some(violation) =
            crate::service::tag_bounds::removed_key_violation(input.tag_keys.iter())
        {
            return Err(crate::scaffolded::Scaffolded::validation(violation));
        }
```

Why the 501 is the wrong claim — src/scaffolded.rs:209-213:
```
    /// Reach for this when the request is wrong in a way the model cannot express,
    /// so the boundary validator passed it: ... Answer it INSTEAD of
    /// `not_implemented` — a 501 says "nobody has written this yet", which is a
    /// different claim and is counted as one.
```

And why the service's `Message` rename never reaches this body — src/service/error_detail.rs:30-33:
```
//! path. A failure answered before any operation is served — a request naming no modelled
//! operation, a transport-level failure, a pre-dispatch authentication refusal — is not
//! reached from here, because this layer is an operation wrapper.
```

Test coverage: every untag case in the suite sends at least one key — tests/all/error_detail_member.rs:360-364 (`?tagKeys=env`), tests/all/resource_tag_removal.rs and tests/all/resource_tagging.rs build a `tagKeys=` query for each named key — so the no-key form is pinned by no test.

## Steps to Reproduce

1. POST /domain with {"name":"clinic"} and capture `arn` from the 201 body.
2. POST /tags/<percent-encoded domain arn> with {"tags":{"env":"prod"}} -> 204.
3. Send DELETE /tags/<percent-encoded domain arn> with NO query string at all (the wire form of an omitted required `tagKeys`).
4. Observe HTTP 501 with error code `NotImplemented` and the body "operation ... is scaffolded but not implemented" wording path, instead of a 400 refusal naming the absent required member — even though src/ops/untag_resource.rs is fully implemented and words its own 400 for an unacceptable key.
5. Send DELETE /tags/<percent-encoded domain arn>?tagKeys=env -> 204, showing the only difference is the absent query parameter.
6. Compare with tests/all/error_detail_member.rs:347-351, which states that "this service has no scaffolded operation left, so the framework's own 501 body is unreachable over HTTP here".
