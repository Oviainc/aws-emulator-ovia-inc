Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# An UntagResource call with no tagKeys query parameter answers 501 instead of the modelled BadRequestException (gap the crate's own test calls "its own issue")

_Filed: 2026-10-29T04:01:06Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResourceRequest$tagKeys` is `@required` and bound to the query string (`@httpQuery("tagKeys")`), so the generated REST route for `DELETE /v1/tags/{resourceArn}` carries `RequiredBinding::Query("tagKeys")` and the shared route lookup rejects the route when the parameter is absent. A `DELETE /v1/tags/<arn>` with no `tagKeys` therefore resolves to no operation: `src/ops/untag_resource.rs::handle` never runs, and the router renders the restJson1 routing default, HTTP 501.

This crate's own test suite records the behaviour and explicitly defers it: "A request with NO `tagKeys` parameter at all is refused by the shared REST route lookup before any handler runs — its own issue". No such issue exists in the backlog, so the gap is untracked.

The answer is wrong twice over: (1) `src/service.rs::override_refusal` states that this service answers "an absent required member" with the modelled `BadRequestException` (HTTP 400), which is the error every operation of this service declares for incorrect input; and (2) 501 tells a client that `UntagResource` is not implemented, when it is implemented and only the request was malformed. A secondary symptom of the same defect: `tagKeys` is modelled `length 0..200`, so an EMPTY key list is a legal request, yet the only wire form that omits the parameter is the one the router refuses.

The crate owns the seams documented for this exact case — `override_operation` ("serve a request the model's routing key cannot name correctly") and `override_routing_error` ("how this service words a request that reached it but named no modeled operation") — and both currently return `None`.

## Evidence

Model — `smithy_query(query="operation", operation="UntagResource")`:
```
"tagKeys": {"target": "TagKeyList", "httpQuery": "tagKeys", "required": true, "length": {"min": 0, "max": 200}, "type": "list"}
```
with `{"shape": "BadRequestException", "errorKind": "client", "httpError": 400}` among its errors.

Side 1 — the crate's own test admits the gap, `tests/all/worker_configuration_delete_and_tags.rs:485-489`:
```
        TagCase {
            // A removal naming only keys the resource does not carry, several
            // at once: nothing goes. (A request with NO `tagKeys` parameter at
            // all is refused by the shared REST route lookup before any handler
            // runs — its own issue — so the empty list is not a row here.)
```

Side 2 — the rule the crate says it follows for an absent required member, `src/service.rs:99-124`:
```
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. KafkaConnect models its own client-input error for
        // exactly that case, so name it rather than the framework's generic
        // `ValidationException`
...
                Some(Refused {
                    code: "BadRequestException".to_string(),
```
and what is rendered instead, `src/service.rs:148-152`:
```
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
```

Why the route is rejected (cited, not filed against): `vendor/smithy-macros/src/lib.rs:599-612` collects every `@required` + `@httpQuery` member as `RequiredBinding::Query(..)` and `:4060-4066` attaches it to the route; `vendor/smithy-schema-server/src/rest_xml.rs:368` — "a candidate route is **rejected** when any of its [`RequiredBinding`]s is absent from the request" — implemented at `rest_xml.rs:561-573` (`if !present { return None; }`); `vendor/smithy-server-runtime/src/multi_service_router.rs:214-224` then renders `routing_error_response`.

The handler that never runs: `src/ops/untag_resource.rs:159` (`async fn handle`), which would otherwise answer through `state.remove_tags(..)` at `src/ops/untag_resource.rs:196`.

Test coverage: `tests/all/worker_configuration_delete_and_tags.rs` covers every other removal shape (absent keys, several of four, emptied, re-attached, interleaved) but — by its own comment above — has NO row for a request without `tagKeys`, so nothing pins the current 501.

## Steps to Reproduce

1. Create a worker configuration: POST /v1/worker-configurations with {"name":"wc-untag-route","propertiesFileContent":"QUJD"}; note workerConfigurationArn.
2. Send DELETE /v1/tags/<percent-encoded workerConfigurationArn> with NO query string at all.
3. Observe HTTP 501: the route is rejected because `RequiredBinding::Query("tagKeys")` is absent, so `UntagResource::handle` never runs.
4. Expected per the model and per `src/service.rs:99-124`: HTTP 400 with `__type` `BadRequestException` naming the absent required member.
5. Contrast with DELETE /v1/tags/<arn>?tagKeys=absent-one, which routes and answers 200.
