Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# An UpdateConnector call that omits the required currentVersion query parameter answers 501 instead of the modelled BadRequestException

_Filed: 2026-10-29T04:01:06Z_
_Operation: UpdateConnector | Class: error-response_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConnectorRequest$currentVersion` is `@required` and bound to the query string (`@httpQuery("currentVersion")`). Because it is BOTH `@required` and an HTTP query binding, the generated REST route table marks it a `RequiredBinding::Query("currentVersion")`, and the shared REST route lookup REJECTS the route outright when the parameter is absent. `PUT /v1/connectors/<arn>` with a well-formed body but no `?currentVersion=` therefore resolves to NO operation at all: the handler in `src/ops/update_connector.rs` never runs, and the request falls into the router's "reached its service but named no modeled operation" path, which for a restJson1 service renders HTTP 501.

That contradicts this crate's own statement of what an absent required member answers. `src/service.rs::override_refusal` exists precisely so that "an absent required member, or a value outside a declared bound" is answered as the modelled `BadRequestException` (HTTP 400) rather than the framework's generic error — but that seam is only reached for a request that MATCHED an operation. A caller who forgets `currentVersion` is told the operation is not implemented (501), although `UpdateConnector` is fully implemented and the model declares `BadRequestException` (400, "Bad request due to incorrect input. Correct your request and then retry it.") for exactly this case.

The crate owns two seams documented for this situation and both currently return `None`: `override_operation` ("serve a request the model's routing key cannot name correctly") and `override_routing_error` ("how this service words a request that reached it but named no modeled operation").

## Evidence

Model — `smithy_query(query="operation", operation="UpdateConnector")`:
```
"currentVersion": {"target": "__string", "httpQuery": "currentVersion", "required": true}
```
and the operation's errors include `{"shape": "BadRequestException", "errorKind": "client", "httpError": 400}`.

The route table marks it a mandatory discriminator — `vendor/smithy-macros/src/lib.rs:599-612` (collected for every member that is both `@required` and `@httpQuery`):
```
        if !traits.contains_key("smithy.api#required") {
            continue;
        }
        if let Some(header) = traits.get("smithy.api#httpHeader") { ... }
        else if let Some(query) = traits.get("smithy.api#httpQuery") {
            let name = query.as_str().unwrap_or(member_name);
            bindings.push(quote! {
                ::aws_smithy_schema_server::RequiredBinding::Query(#name)
            });
        }
```
and `vendor/smithy-macros/src/lib.rs:4060-4066` attaches them to the route: `#route.with_required(&[#(#required_bindings),*])`.

The router then refuses the route — `vendor/smithy-schema-server/src/rest_xml.rs:368`:
```
/// So a candidate route is **rejected** when any of its [`RequiredBinding`]s is absent from the request,
```
implemented at `vendor/smithy-schema-server/src/rest_xml.rs:557-573`:
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

What the caller then gets — `vendor/smithy-server-runtime/src/multi_service_router.rs:214-224`: on an unresolved operation the router calls `svc.override_routing_error(&rt_req).unwrap_or_else(|| svc.definition().routing_error_response(&rt_req))`, and this crate's own `src/service.rs:148-150` states what that default renders:
```
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
```
with `src/service.rs:151-152` returning that default: `let _ = req;` / `None`.

The contradicting in-crate rule — `src/service.rs:99-124`:
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

The handler that never runs: `src/ops/update_connector.rs:166` (`async fn handle`), whose `currentVersion` check at `src/ops/update_connector.rs:257-265` can only be reached once the route has matched.

Test coverage: no test in `tests/all/` sends a PUT to `/v1/connectors/{arn}` without a `currentVersion` query parameter — `tests/all/connector_update.rs` always includes it — so nothing pins the current 501.

## Steps to Reproduce

1. Create a connector and wait for it to reach RUNNING (POST /v1/connectors, then poll GET /v1/connectors/{arn}).
2. Send the update WITHOUT the required query parameter: PUT /v1/connectors/<percent-encoded connectorArn> with body {"capacity":{"provisionedCapacity":{"mcuCount":1,"workerCount":2}}} and no `?currentVersion=`.
3. Observe HTTP 501: the route is rejected because `RequiredBinding::Query("currentVersion")` is absent, so `UpdateConnector::handle` never runs.
4. Expected per the model and per `src/service.rs:99-124`: HTTP 400 with `__type` `BadRequestException`.
5. Contrast with the same request carrying `?currentVersion=1`, which routes and is served normally.
