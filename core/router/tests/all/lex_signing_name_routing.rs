// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the SigV4 signing name `lex`, shared by FOUR emulated
//! services.
//!
//! `aws.auth#sigv4` names `lex` on every Lex-family model while each one serves its own
//! endpoint (`smithy_query(query="service_info")` on the four models):
//!
//! | service | endpoint prefix | API version | protocol |
//! |---|---|---|---|
//! | Lex Models V2 (model building) | `models-v2-lex` | 2020-08-07 | restJson1 |
//! | Lex Runtime V2 | `runtime-v2-lex` | 2020-08-07 | restJson1 |
//! | Lex Model Building Service (V1) | `models.lex` | 2017-04-19 | restJson1 |
//! | Lex Runtime Service (V1) | `runtime.lex` | 2016-11-28 | restJson1 |
//!
//! Four endpoint prefixes, ONE signing name — a shared signing name across a service
//! family and its API versions is normal AWS and explicitly not an error condition. The
//! router resolves a request's routing key from the SigV4 credential scope first, so all
//! four see the key `lex`: a service that claims on the signing name ALONE claims every
//! sibling's traffic too, the router finds four claimants and answers `500
//! AmbiguousService` for ALL of it. That is a code no AWS model declares, and a 5xx where
//! every modelled failure of these operations but `InternalServerException` is a 4xx, so
//! an SDK retries to its ceiling a request that can never succeed — which is exactly what
//! made Lex V2 model building unreachable: `CreateBot`, the first call of every Lex check,
//! never reached a handler. AWS separates the four by endpoint host, which an emulator
//! cannot use: a client configured with one local `endpoint_url` sends the same `Host` for
//! all four.
//!
//! Each one therefore also requires the request to address its OWN modelled wire surface:
//! the first URI path segment its model declares, and — where more than one of them
//! publishes that segment (`bots`, `builtins`, `exports`, `imports`, `tags`) — the whole
//! modelled route, method and URI pattern, which is what distinguishes `restJson1` APIs on
//! the wire. `service_common::lex` states that split once for all four, including the one
//! route shape two of them bind IDENTICALLY (`DELETE /bots/{…}`, `DeleteBot` in both
//! control planes), which is split by the shape of its path label.
//!
//! This suite pins that: the claim table (who claims what), the property that the four
//! models' path spaces overlap ONLY where that rule declares a split, that a request to
//! each member reaches that member's own service through the whole router — never the
//! router's `AmbiguousService` (500, several claimants) or `UnknownService` (404, none) —
//! and that a V2 bot created through the whole router reads back through it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// The two refusals the ROUTER itself produces, neither of which is an AWS error code: a
/// request that reaches exactly one service can answer neither.
const ROUTER_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// The Smithy shape ids of the family members, as `identity().shape_id()` reports them.
const MODELS_V2: &str = "com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2";
const RUNTIME_V2: &str = "com.amazonaws.lexruntimev2#AWSDeepSenseRunTimeServiceApi2_0";
const MODELS_V1: &str = "com.amazonaws.lexmodelbuildingservice#AWSDeepSenseModelBuildingService";
const RUNTIME_V1: &str = "com.amazonaws.lexruntimeservice#AWSDeepSenseRunTimeService";

/// An IAM role ARN satisfying the V2 model's `RoleArn` shape
/// (len 32..2048, pattern `^arn:aws:iam::[0-9]{12}:role/.*$`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/lex-routing-check";

/// A SigV4 `Authorization` header whose credential scope names `lex`. The signature is
/// never checked: the router here runs with request authentication off.
fn authorization() -> String {
    "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/lex/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
        .to_string()
}

/// Every emulated service whose SigV4 signing name is `lex`, over one engine, as
/// `core/router/src/lib.rs` mounts them.
fn lex_family(engine: &Arc<engine::InMemoryEngine>) -> Vec<Box<dyn Service>> {
    vec![
        Box::new(
            service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_lexruntimev2_awsdeepsenseruntimeserviceapi2_0::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_lexmodelbuildingservice_awsdeepsensemodelbuildingservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_lexruntimeservice_awsdeepsenseruntimeservice::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
    ]
}

/// The whole family mounted in a router, over the engine the caller keeps a handle on.
fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lexruntimev2_awsdeepsenseruntimeserviceapi2_0::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lexmodelbuildingservice_awsdeepsensemodelbuildingservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lexruntimeservice_awsdeepsenseruntimeservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// Which mounted services claim a `lex`-signed request for `method path`.
///
/// The `Host` is the emulator's own, as a client configured with one local `endpoint_url`
/// sends it: nothing in it names a Lex endpoint prefix, which is precisely why the claim
/// cannot be made on the host.
fn claimants(family: &[Box<dyn Service>], method: &str, path: &str) -> Vec<String> {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("authorization", authorization())
        .body(Bytes::new())
        .expect("build the request");
    let (parts, _body) = request.into_parts();
    family
        .iter()
        .filter(|service| service.can_handle(&parts))
        .map(|service| service.definition().identity().shape_id().to_string())
        .collect()
}

/// One row: a request under the shared signing name, and every service that may claim it.
struct Row {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    claimants: Vec<&'static str>,
}

/// RULE: a request signed under the shared `lex` signing name is claimed by the one family
/// member whose modelled wire surface it addresses — its own first URI path segment, and,
/// where more than one member publishes that segment, the member whose own model binds the
/// request's route (or, for a route two models bind identically, the member whose modelled
/// label shape admits the identifier in the path).
#[test]
fn each_service_sharing_the_lex_signing_name_claims_only_its_own_surface() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let family = lex_family(&engine);

    // Rows differ along the wire surface they address: each member's own surface, both
    // sides of every path space two members share (`bots`, `builtins`, `exports`,
    // `imports`, `tags`), the method that separates two routes on one path, the label
    // shape that separates the one identical route, near misses on a segment, the root
    // path, and a path no member binds.
    let rows = [
        Row {
            // The issue's own call. Nothing but the route tells it from the V1 control
            // plane's `/bots` space.
            case: "CreateBot on the V2 control plane",
            method: "PUT",
            path: "/bots",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "ListBots on the V2 control plane, the same path under POST",
            method: "POST",
            path: "/bots",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "GetBots on the V1 control plane, the same path under GET",
            method: "GET",
            path: "/bots",
            claimants: vec![MODELS_V1],
        },
        Row {
            case: "DescribeBot on the V2 control plane",
            method: "GET",
            path: "/bots/BOT1234567",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "UpdateBot on the V2 control plane",
            method: "PUT",
            path: "/bots/BOT1234567",
            claimants: vec![MODELS_V2],
        },
        Row {
            // The one route shape BOTH control planes bind: `DeleteBot`. A label shaped
            // like a V2 `Id` (ten `[0-9a-zA-Z]`) is a V2 bot id.
            case: "DeleteBot on a V2 bot id",
            method: "DELETE",
            path: "/bots/BOT1234567",
            claimants: vec![MODELS_V2],
        },
        Row {
            // …and a label that is not one is a V1 bot name.
            case: "DeleteBot on a V1 bot name",
            method: "DELETE",
            path: "/bots/DoomedBot",
            claimants: vec![MODELS_V1],
        },
        Row {
            case: "PutBot on the V1 control plane",
            method: "PUT",
            path: "/bots/OrderFlowers/versions/$LATEST",
            claimants: vec![MODELS_V1],
        },
        Row {
            case: "GetBot on the V1 control plane",
            method: "GET",
            path: "/bots/OrderFlowers/versions/$LATEST",
            claimants: vec![MODELS_V1],
        },
        Row {
            // `botversions` against the V1 model's `versions`: exact segment equality,
            // never a prefix test.
            case: "CreateBotVersion on the V2 control plane",
            method: "PUT",
            path: "/bots/BOT1234567/botversions",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "CreateBotVersion on the V1 control plane",
            method: "POST",
            path: "/bots/OrderFlowers/versions",
            claimants: vec![MODELS_V1],
        },
        Row {
            case: "CreateBotLocale on the V2 control plane",
            method: "PUT",
            path: "/bots/BOT1234567/botversions/DRAFT/botlocales",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "BuildBotLocale on the V2 control plane",
            method: "POST",
            path: "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "DeleteBotLocale on the V2 control plane",
            method: "DELETE",
            path: "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "CreateSlotType on the V2 control plane",
            method: "PUT",
            path: "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US/slottypes",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "CreateSlot on the V2 control plane",
            method: "PUT",
            path: "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US/intents/INTENT1234/slots",
            claimants: vec![MODELS_V2],
        },
        Row {
            // The V2 runtime's own space is inside `bots` too, and its literals are
            // camelCase where the V2 control plane's are lower-case: `botAliases` is a
            // different segment from `botaliases` under exact, case-sensitive equality.
            case: "RecognizeText on the V2 runtime",
            method: "POST",
            path: "/bots/BOT1234567/botAliases/TSTALIASID/botLocales/en_US/sessions/s-1/text",
            claimants: vec![RUNTIME_V2],
        },
        Row {
            case: "GetSession on the V2 runtime",
            method: "GET",
            path: "/bots/BOT1234567/botAliases/TSTALIASID/botLocales/en_US/sessions/s-1",
            claimants: vec![RUNTIME_V2],
        },
        Row {
            case: "DeleteSession on the V2 runtime, the same path under DELETE",
            method: "DELETE",
            path: "/bots/BOT1234567/botAliases/TSTALIASID/botLocales/en_US/sessions/s-1",
            claimants: vec![RUNTIME_V2],
        },
        Row {
            // The V1 runtime binds everything under `bot`, which no sibling publishes.
            case: "PostText on the V1 runtime",
            method: "POST",
            path: "/bot/OrderFlowers/alias/PROD/user/user-1/text",
            claimants: vec![RUNTIME_V1],
        },
        Row {
            case: "GetSession on the V1 runtime",
            method: "GET",
            path: "/bot/OrderFlowers/alias/PROD/user/user-1/session",
            claimants: vec![RUNTIME_V1],
        },
        Row {
            // Both control planes bind the identical three tagging routes and nothing
            // else under `tags`, so no part of the request tells them apart and the V2
            // control plane is the declared owner of the whole segment.
            case: "ListTagsForResource, on the tagging space both control planes bind",
            method: "GET",
            path: "/tags/arn%3Aaws%3Alex%3Aus-east-1%3A000000000000%3Abot%2FBOT1234567",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "TagResource on that same space",
            method: "POST",
            path: "/tags/arn%3Aaws%3Alex%3Aus-east-1%3A000000000000%3Abot%2FBOT1234567",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "UntagResource on that same space",
            method: "DELETE",
            path: "/tags/arn%3Aaws%3Alex%3Aus-east-1%3A000000000000%3Abot%2FBOT1234567",
            claimants: vec![MODELS_V2],
        },
        Row {
            // Every V1 import route is identical to a V2 one, so the V2 control plane
            // owns the segment.
            case: "the import listing both control planes bind as POST /imports",
            method: "POST",
            path: "/imports",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "the import read both control planes bind",
            method: "GET",
            path: "/imports/IMPORT1234",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "StartImport, which only the V2 control plane binds as PUT /imports",
            method: "PUT",
            path: "/imports",
            claimants: vec![MODELS_V2],
        },
        Row {
            // `exports` is shared but the routes under it are disjoint, so the route
            // decides rather than an owner. GetExport's four `@required` `@httpQuery`
            // members are part of what names it, exactly as they are when the router
            // picks one: an SDK always sends them.
            case: "GetExport, the V1 control plane's only exports route",
            method: "GET",
            path: "/exports?name=OrderFlowers&version=1&resourceType=BOT\
                   &exportType=LEX",
            claimants: vec![MODELS_V1],
        },
        Row {
            case: "CreateExport on the V2 control plane",
            method: "PUT",
            path: "/exports",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "DescribeExport on the V2 control plane",
            method: "GET",
            path: "/exports/EXPORT1234",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "GetBuiltinIntents on the V1 control plane",
            method: "GET",
            path: "/builtins/intents",
            claimants: vec![MODELS_V1],
        },
        Row {
            case: "ListBuiltInIntents on the V2 control plane",
            method: "POST",
            path: "/builtins/locales/en_US/intents",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "GetIntents, on a segment only the V1 control plane publishes",
            method: "GET",
            path: "/intents",
            claimants: vec![MODELS_V1],
        },
        Row {
            case: "GetSlotTypes, likewise",
            method: "GET",
            path: "/slottypes",
            claimants: vec![MODELS_V1],
        },
        Row {
            case: "GetMigrations, likewise",
            method: "GET",
            path: "/migrations",
            claimants: vec![MODELS_V1],
        },
        Row {
            case: "CreateResourcePolicy, on a segment only the V2 control plane publishes",
            method: "POST",
            path: "/policy/arn%3Aaws%3Alex%3Aus-east-1%3A000000000000%3Abot%2FBOT1234567",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "ListTestSets, likewise",
            method: "POST",
            path: "/testsets",
            claimants: vec![MODELS_V2],
        },
        Row {
            case: "a near miss on a shared segment",
            method: "PUT",
            path: "/botsextra",
            claimants: vec![],
        },
        Row {
            case: "a shared segment in the wrong case",
            method: "PUT",
            path: "/Bots",
            claimants: vec![],
        },
        Row {
            case: "the V1 runtime's segment pluralised is the V2 space, with no route",
            method: "POST",
            path: "/bots/OrderFlowers/alias/PROD/user/user-1/text",
            claimants: vec![],
        },
        Row {
            case: "a path inside the shared bots space that no model binds",
            method: "POST",
            path: "/bots/BOT1234567/no-such-operation",
            claimants: vec![],
        },
        Row {
            case: "a path no lex model binds",
            method: "POST",
            path: "/not-a-lex-path",
            claimants: vec![],
        },
        Row {
            // No lex model is RPC, so none of them posts to the root path.
            case: "the root path",
            method: "POST",
            path: "/",
            claimants: vec![],
        },
    ];

    for row in rows {
        let actual = claimants(&family, row.method, row.path);
        assert_eq!(
            actual, row.claimants,
            "case {:?}: expected {:?} to claim {} {}, got {actual:?}",
            row.case, row.claimants, row.method, row.path
        );
    }
}

/// The first URI path segments that MORE THAN ONE of the four models publishes, as
/// `service_common::lex`'s `SHARED_SEGMENTS` declares them. Restated here because a router
/// test target reaches only the crates the router depends on: this assertion is what
/// catches a model whose path space starts overlapping a sibling's somewhere the family
/// rule does not split, which would put two claimants back on that path.
const DECLARED_SHARED_SEGMENTS: &[&str] = &["bots", "builtins", "exports", "imports", "tags"];

/// RULE (the property the claim table rests on): the four models' first-path-segment
/// spaces overlap only where the family rule declares a split, and none of them publishes
/// an EMPTY space — an empty one claims every path and swallows all three peers.
#[test]
fn the_four_path_spaces_overlap_only_where_the_family_rule_splits_them() {
    // (member, its generated ROUTE_PATH_SEGMENTS)
    let members: [(&str, &[&str]); 4] = [
        (
            MODELS_V2,
            service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::ROUTE_PATH_SEGMENTS,
        ),
        (
            RUNTIME_V2,
            service_com_amazonaws_lexruntimev2_awsdeepsenseruntimeserviceapi2_0::ROUTE_PATH_SEGMENTS,
        ),
        (
            MODELS_V1,
            service_com_amazonaws_lexmodelbuildingservice_awsdeepsensemodelbuildingservice::ROUTE_PATH_SEGMENTS,
        ),
        (
            RUNTIME_V1,
            service_com_amazonaws_lexruntimeservice_awsdeepsenseruntimeservice::ROUTE_PATH_SEGMENTS,
        ),
    ];

    for (member, segments) in members {
        assert!(
            !segments.is_empty(),
            "case {member:?}: every lex model is restJson1 and binds every operation to a \
             URI, so it must publish a non-empty path space, got {segments:?}"
        );
    }

    for (index, (member, segments)) in members.iter().enumerate() {
        for (peer, peer_segments) in members.iter().skip(index + 1) {
            for segment in segments.iter() {
                if !peer_segments.contains(segment) {
                    continue;
                }
                assert!(
                    DECLARED_SHARED_SEGMENTS.contains(segment),
                    "case {member:?} vs {peer:?}: both publish the first segment \
                     {segment:?}, which the family rule does not split — every request \
                     under it has two claimants and is refused 500 AmbiguousService"
                );
            }
        }
    }

    // And the other direction: a segment the rule declares shared must really be
    // published by two or more of the four, or the split narrows a space that has one
    // owner anyway and makes part of it unreachable.
    for segment in DECLARED_SHARED_SEGMENTS {
        let publishers: Vec<&str> = members
            .iter()
            .filter(|(_, segments)| segments.contains(segment))
            .map(|(member, _)| *member)
            .collect();
        assert!(
            publishers.len() >= 2,
            "case {segment:?}: the family rule splits this segment, so two or more models \
             must publish it, got {publishers:?}"
        );
    }
}

/// One answer from the whole router, reduced to what the rows assert on.
struct Answer {
    status: u16,
    error_code: String,
    body: Value,
    raw_body: String,
}

/// Dispatch one `lex`-signed request through the whole router.
async fn dispatch(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw_body = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_str::<Value>(&raw_body).unwrap_or(Value::Null);
    // restJson1 names the error shape in `x-amzn-errortype`, and the router's own refusals
    // set `__type` in the body; read both so `AmbiguousService` and `UnknownService` are
    // visible whichever way they arrive.
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_code,
        body,
        raw_body,
    }
}

/// One request to dispatch through the whole router. Every one is signed for `lex`, which
/// is the point.
struct Call<'a> {
    case: &'a str,
    method: &'a str,
    path: &'a str,
    body: &'a str,
}

/// RULE: with the whole family mounted, a request to any member reaches THAT member's own
/// service, so the answer is that service's own — never the router's `AmbiguousService`
/// refusal (two or more claimants) and never its `UnknownService` one (none).
#[tokio::test]
async fn every_member_is_reachable_through_the_whole_router() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // Rows differ along the member they address and the route that names it, spanning the
    // operations the behavioral checks call first on each.
    let create_bot = json!({
        "botName": "RouterReachable",
        "roleArn": ROLE_ARN,
        "dataPrivacy": { "childDirected": false },
        "idleSessionTTLInSeconds": 300,
    })
    .to_string();
    let calls: [Call<'_>; 8] = [
        Call {
            case: "CreateBot on the V2 control plane",
            method: "PUT",
            path: "/bots",
            body: &create_bot,
        },
        Call {
            case: "ListBots on the V2 control plane",
            method: "POST",
            path: "/bots",
            body: "{}",
        },
        Call {
            case: "DescribeBot on the V2 control plane, for an id no bot carries",
            method: "GET",
            path: "/bots/0000000000",
            body: "",
        },
        Call {
            case: "GetBots on the V1 control plane",
            method: "GET",
            path: "/bots",
            body: "",
        },
        Call {
            case: "PutBot on the V1 control plane",
            method: "PUT",
            path: "/bots/RouterReachableOne/versions/$LATEST",
            body: r#"{"locale":"en-US","childDirected":false,"processBehavior":"SAVE"}"#,
        },
        Call {
            case: "GetSession on the V2 runtime, for a bot alias no bot carries",
            method: "GET",
            path: "/bots/0000000000/botAliases/TSTALIASID/botLocales/en_US/sessions/s-1",
            body: "",
        },
        Call {
            case: "RecognizeText on the V2 runtime, for a bot alias no bot carries",
            method: "POST",
            path: "/bots/0000000000/botAliases/TSTALIASID/botLocales/en_US/sessions/s-1/text",
            body: r#"{"text":"hello"}"#,
        },
        Call {
            case: "GetSession on the V1 runtime, for a bot nothing created",
            method: "GET",
            path: "/bot/OrderFlowers/alias/PROD/user/user-1/session",
            body: "",
        },
    ];

    for call in calls {
        let answer = dispatch(&router, call.method, call.path, call.body).await;
        assert!(
            !ROUTER_REFUSALS.contains(&answer.error_code.as_str()),
            "case {:?}: {} {} must reach exactly one of the four services sharing the \
             `lex` signing name, got HTTP {} {:?}: {}",
            call.case,
            call.method,
            call.path,
            answer.status,
            answer.error_code,
            answer.raw_body
        );
    }
}

/// RULE (the complement): narrowing each claim to the member's own wire surface must not
/// leave some member claiming EVERY `lex`-signed path. A path none of the four binds is the
/// router's 404 `UnknownService` — one claimant too few, never the 500 that says several
/// claimed it.
#[tokio::test]
async fn a_lex_signed_request_on_a_path_no_member_binds_reaches_nobody() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    let rows: [Call<'_>; 3] = [
        Call {
            case: "a path no lex model binds",
            method: "POST",
            path: "/not-a-lex-path",
            body: "{}",
        },
        Call {
            // Inside a space three of the models share, the claim is the exact modelled
            // route, so a path none of them binds reaches nobody rather than all three.
            case: "a path inside the shared bots space that no model binds",
            method: "POST",
            path: "/bots/BOT1234567/no-such-operation",
            body: "{}",
        },
        Call {
            case: "the root path, which no restJson1 lex model binds",
            method: "POST",
            path: "/",
            body: "{}",
        },
    ];

    for call in rows {
        let answer = dispatch(&router, call.method, call.path, call.body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "UnknownService"),
            "case {:?}: expected the router's 404 UnknownService, got {} {:?} with {}",
            call.case,
            answer.status,
            answer.error_code,
            answer.raw_body
        );
    }
}

/// RULE: a V2 bot created through the whole router is readable back through it, and both
/// answers are the V2 control plane's own — the proof that `CreateBot`, the call every Lex
/// check starts with and the one the ambiguity refusal killed, reaches the Lex Models V2
/// handlers rather than a sibling or the router's refusal.
#[tokio::test]
async fn a_bot_created_through_the_whole_router_reads_back_through_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // Rows differ along what `CreateBot` generalises over — the optional members a bot
    // carries and the session TTL, at both published bounds (range 60..86400) — since the
    // routing rule must hold for every shape of the request, not for one recorded body.
    let rows: [(&str, Value); 4] = [
        (
            "the required members alone",
            json!({
                "botName": "RoutedPlainBot",
                "roleArn": ROLE_ARN,
                "dataPrivacy": { "childDirected": false },
                "idleSessionTTLInSeconds": 300,
            }),
        ),
        (
            "with a description and the lowest TTL the model allows",
            json!({
                "botName": "RoutedDescribedBot",
                "description": "created through the whole router",
                "roleArn": ROLE_ARN,
                "dataPrivacy": { "childDirected": false },
                "idleSessionTTLInSeconds": 60,
            }),
        ),
        (
            "with tags and the highest TTL the model allows",
            json!({
                "botName": "RoutedTaggedBot",
                "roleArn": ROLE_ARN,
                "dataPrivacy": { "childDirected": true },
                "idleSessionTTLInSeconds": 86_400,
                "botTags": { "team": "routing" },
                "testBotAliasTags": { "team": "routing" },
            }),
        ),
        (
            "with an explicit botType",
            json!({
                "botName": "RoutedTypedBot",
                "roleArn": ROLE_ARN,
                "dataPrivacy": { "childDirected": false },
                "idleSessionTTLInSeconds": 1_200,
                "botType": "Bot",
            }),
        ),
    ];

    for (case, body) in rows {
        let created = dispatch(&router, "PUT", "/bots", &body.to_string()).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: CreateBot must answer the 202 its model declares, got {} {:?}: {}",
            created.status, created.error_code, created.raw_body
        );
        let bot_id = created
            .body
            .get("botId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no botId in {}", created.raw_body))
            .to_string();
        // The modelled `Id` shape: @length(min: 10, max: 10), @pattern(^[0-9a-zA-Z]+$).
        assert!(
            bot_id.len() == 10 && bot_id.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "case {case:?}: botId {bot_id:?} must satisfy the modelled Id shape"
        );
        assert_eq!(
            created.body.get("botStatus").and_then(Value::as_str),
            Some("Creating"),
            "case {case:?}: a created bot starts in Creating, got {}",
            created.raw_body
        );

        let read = dispatch(&router, "GET", &format!("/bots/{bot_id}"), "").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: DescribeBot on {bot_id} must answer 200, got {} {:?}: {}",
            read.status, read.error_code, read.raw_body
        );
        assert_eq!(
            read.body.get("botId").and_then(Value::as_str),
            Some(bot_id.as_str()),
            "case {case:?}: the read-back bot is the created one, got {}",
            read.raw_body
        );
        assert_eq!(
            read.body.get("botName").and_then(Value::as_str),
            body.get("botName").and_then(Value::as_str),
            "case {case:?}: the read-back bot carries the name that was created, got {}",
            read.raw_body
        );
    }
}

/// RULE: on `DELETE /bots/{…}`, the route shape BOTH control planes bind, the claim follows
/// the label's own modelled shape — so each control plane's `DeleteBot` stays reachable
/// through the whole router, each answering its own modelled success.
#[tokio::test]
async fn both_control_planes_delete_their_own_bots_through_the_whole_router() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // A V2 bot, addressed by the ten-character id its own CreateBot minted.
    let created = dispatch(
        &router,
        "PUT",
        "/bots",
        &json!({
            "botName": "RoutedDoomedBot",
            "roleArn": ROLE_ARN,
            "dataPrivacy": { "childDirected": false },
            "idleSessionTTLInSeconds": 300,
        })
        .to_string(),
    )
    .await;
    let bot_id = created
        .body
        .get("botId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateBot must mint a botId, got {}", created.raw_body))
        .to_string();

    let deleted = dispatch(&router, "DELETE", &format!("/bots/{bot_id}"), "").await;
    assert_eq!(
        (deleted.status, deleted.body.get("botStatus").and_then(Value::as_str)),
        (202, Some("Deleting")),
        "the V2 DeleteBot must answer its own modelled 202 with the Deleting status, got {} {:?}: {}",
        deleted.status,
        deleted.error_code,
        deleted.raw_body
    );

    // A V1 bot, addressed by the NAME its own PutBot stored. The V1 model answers 204
    // with an empty body, which the V2 service could not produce for this path.
    let stored = dispatch(
        &router,
        "PUT",
        "/bots/RoutedDoomedOne/versions/$LATEST",
        r#"{"locale":"en-US","childDirected":false,"processBehavior":"SAVE"}"#,
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "the V1 PutBot must answer its own 200, got {} {:?}: {}",
        stored.status, stored.error_code, stored.raw_body
    );

    let deleted = dispatch(&router, "DELETE", "/bots/RoutedDoomedOne", "").await;
    assert_eq!(
        (deleted.status, deleted.raw_body.as_str()),
        (204, ""),
        "the V1 DeleteBot must answer its own modelled 204 with no body, got {} {:?}: {}",
        deleted.status,
        deleted.error_code,
        deleted.raw_body
    );
}
