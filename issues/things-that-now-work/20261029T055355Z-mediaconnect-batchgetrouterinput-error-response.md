Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 12966d24554ed06eeaba7911ae25ff590cd06862

# A BatchGetRouterInput request with no `arns` query parameter answers 501, not the BadRequestException this crate says an absent required member gets (gap its own test comment admits)

_Filed: 2026-10-29T05:53:55Z_
_Operation: BatchGetRouterInput | Class: error-response_

> _Note: this issue was identified against the code base at commit `12966d24554ed06eeaba7911ae25ff590cd06862`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchGetRouterInputRequest$Arns` is `@required` and bound to the query string (`@httpQuery("arns")`), so the generated REST route for `GET /v1/routerInputs` carries `RequiredBinding::Query("arns")` and the shared route lookup REJECTS the route when the parameter is absent. A `GET /v1/routerInputs` with no query string therefore resolves to no operation at all: `BatchGetRouterInput::handle` never runs, `override_routing_error` returns `None`, and the router renders the restJson1 routing default — HTTP 501 NotImplemented.

That contradicts this crate's own stated rule twice over: (1) `src/service.rs` says a request the framework refuses before any handler runs — explicitly including "a required member absent" — is answered with `BadRequestException` (HTTP 400) in this service's `Validation failed:` envelope, and the crate's own envelope test asserts exactly that for an absent required member of a create; and (2) 501 tells the client that BatchGetRouterInput is not implemented, when it IS implemented and only the request was malformed. There is no legal wire form that satisfies the route either way: `Arns` is `length 1..20`, so an empty list is already illegal and omission is the only shape of "no ARNs".

The crate's own test comment ADMITS the route is unresolved and declines to cover it, so nothing pins the 501. The same defect holds, for the same reason, for the two sibling batch reads (`BatchGetRouterOutputRequest$Arns`, `BatchGetRouterNetworkInterfaceRequest$Arns` — both `@required` + `@httpQuery("arns")`): the fix belongs in the one `src/service.rs` seam the file documents for this case (`override_operation` / `override_routing_error`, both currently `None`), so one fix covers all three.

## Evidence

Model — `smithy_query(query="shape", shape="BatchGetRouterInputRequest")`:
```
"Arns": {"target": "RouterInputArnList", "httpQuery": "arns", "length": {"min": 1, "max": 20}, "required": true, "type": "list"}
```
and `BatchGetRouterInput` declares `{"shape": "BadRequestException", "errorKind": "client", "httpError": 400}`.

Side 1 — the crate's own test comment admits the gap, services/service-com-amazonaws-mediaconnect-mediaconnect/tests/all/router_input_batch_get.rs:411-415:
```
    // The lower end of the bound is the 1-ARN row: `Arns` is required and bound to the
    // query string, so a request carrying no `arns` parameter at all resolves no route in
    // the first place and never reaches this operation's bound.
    let count_cases = [
        ("the minimum", MIN_ARNS, true),
```
(the helper even carries the never-exercised empty-query branch, tests/all/router_input_batch_get.rs:74-78: `let uri = if query.is_empty() { "/v1/routerInputs".to_string() } else { ... }`).

Side 2 — the rule this service says it follows for an absent required member, src/service.rs:160-192:
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
and what is rendered instead, src/service.rs:198-210:
```
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
```

The crate's own envelope test pins the 400 for the body-bound twin of this case, tests/all/error_envelopes.rs:657-665 + 745-749:
```
        RefusalCase {
            case: "the required Name member absent from a create",
            at_the_boundary: true,
...
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
```

Why the route is rejected (cited, not filed against): vendor/smithy-macros/src/lib.rs:599-612 collects every `@required` + `@httpQuery` member as `RequiredBinding::Query(..)`; vendor/smithy-schema-server/src/rest_xml.rs:367-369 — "a candidate route is **rejected** when any of its [`RequiredBinding`]s is absent from the request" — implemented at rest_xml.rs:561-575 (`if !present { return None; }`), and rest_xml.rs:378-380 states this lookup is "Shared by [`RestXmlServerProtocol`] and [`crate::rest_json::RestJsonServerProtocol`]", which is this service's protocol (`restJson1`).

The handler that never runs: src/ops/batch_get_router_input.rs:154 (`async fn handle`), which would otherwise answer through `store.router_input_projection(arn, now)` at line 188.

Test coverage: tests/all/router_input_batch_get.rs covers 1 ARN, 20 ARNs, 21 ARNs (400), a malformed ARN (400) and unknown ARNs (`Errors` entries) — but, by its own comment above, has NO row for a request without `arns`, so nothing pins the current 501.

## Steps to Reproduce

1. Start the emulator with the mediaconnect service mounted.
2. Send `GET /v1/routerInputs` with NO query string at all (and no body).
3. Observe HTTP 501: the route carries `RequiredBinding::Query("arns")`, so the route lookup rejects it and `BatchGetRouterInput::handle` never runs.
4. Expected per the model and per src/service.rs:160-192: HTTP 400 with `__type` `BadRequestException` and a `Validation failed:` message naming the absent required member — the same answer `POST /v1/flows` with no `name` already gives (tests/all/error_envelopes.rs:657-665).
5. Contrast with `GET /v1/routerInputs?arns=arn:aws:mediaconnect:us-east-1:123456789012:routerInput:abcdefghijkl`, which routes and answers 200.
6. Repeat with `GET /v1/routerOutputs` and `GET /v1/routerNetworkInterfaces` (no query): both answer 501 for the same reason.
