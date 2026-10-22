// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS — the `Service::can_handle` seam.
//!
//! Lex Models V2 signs as `lex`, and so does every other service in the Lex family: Lex
//! Runtime V2 (endpoint `runtime-v2-lex`), the V1 Lex Model Building Service
//! (`models.lex`) and the V1 Lex Runtime Service (`runtime.lex`) — four distinct wire
//! services across two API versions, one `aws.auth#sigv4` name, which is normal AWS and
//! not an error condition. The router resolves a request's routing key from the SigV4
//! credential scope first, so all four see the key `lex`; claiming on that key alone makes
//! each of them a claimant for every Lex request, the router finds four claimants and
//! answers `500 AmbiguousService` — a code no AWS model carries, retried to the SDK's
//! ceiling because it is a 5xx — for all of it, so nothing on Lex V2 model building is
//! reachable, starting with `CreateBot`.
//!
//! The claim is therefore the routing key AND the request's own modelled wire surface:
//! the first URI path segment, matched by exact equality against this service's
//! model-derived `ROUTE_PATH_SEGMENTS` (`OPERATIONS.md`: `/bots`, `/builtins`, `/exports`,
//! `/imports`, `/policy`, `/tags`, `/testsets`, `/testexecutions`, `/testsetdiscrepancy`,
//! `/testsetgenerations`, `/createuploadurl`), with the segments a sibling model also
//! publishes split by `service_common::lex` — by the whole modelled route (method + URI
//! pattern), by a declared owner where two models bind the identical routes, and, on the
//! one route shape two bind identically while both are implemented, by the modelled shape
//! of the path label.
//!
//! This suite pins that rule over the class of requests it generalises over: this model's
//! own first segments in their collection and instance shapes, a query string, the path
//! spaces it shares with each sibling, both sides of the `DELETE /bots/{…}` label split,
//! near-miss spellings, and requests that name another service or none.

use std::sync::Arc;

use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::EmulatedService;
use smithy_server_runtime::Service;

/// A SigV4 credential scope naming `signing_name`; the signature is never inspected by
/// the routing key.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The request parts `can_handle` is handed: the URI, and the headers the routing key is
/// read from. The host is the emulator's own loopback endpoint, as a client configured
/// with one local `endpoint_url` sends it — it names no Lex endpoint prefix, which is why
/// the claim cannot be made on the host.
fn parts(method: &str, path_and_query: &str, signing_name: Option<&str>) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://127.0.0.1:9297{path_and_query}"))
        .header("host", "127.0.0.1:9297");
    if let Some(signing_name) = signing_name {
        builder = builder.header("authorization", authorization(signing_name));
    }
    builder
        .body(())
        .expect("the request is well formed")
        .into_parts()
        .0
}

/// One claim case: the request, and whether THIS service must claim it.
struct Case {
    name: &'static str,
    method: &'static str,
    path_and_query: &'static str,
    signing_name: Option<&'static str>,
    claimed: bool,
}

const CASES: &[Case] = &[
    // One row per family of this model's own modelled routes, in both the collection and
    // the instance shape where it binds both.
    Case {
        name: "the issue's own call: PUT /bots",
        method: "PUT",
        path_and_query: "/bots",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "ListBots, the same path under POST",
        method: "POST",
        path_and_query: "/bots",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "DescribeBot on the instance path",
        method: "GET",
        path_and_query: "/bots/BOT1234567",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "UpdateBot on the instance path",
        method: "PUT",
        path_and_query: "/bots/BOT1234567",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "CreateBotVersion",
        method: "PUT",
        path_and_query: "/bots/BOT1234567/botversions",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "CreateBotLocale",
        method: "PUT",
        path_and_query: "/bots/BOT1234567/botversions/DRAFT/botlocales",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "BuildBotLocale",
        method: "POST",
        path_and_query: "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "DeleteBotLocale, the same path under DELETE",
        method: "DELETE",
        path_and_query: "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "CreateSlotType",
        method: "PUT",
        path_and_query: "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US/slottypes",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "CreateSlot",
        method: "PUT",
        path_and_query: "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US/intents/INTENT1234/slots",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "DeleteSlotType with a query string",
        method: "DELETE",
        path_and_query: "/bots/BOT1234567/botversions/DRAFT/botlocales/en_US/slottypes/SLOT123456\
                         ?skipResourceInUseCheck=true",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "CreateBotAlias, on the lower-case botaliases segment",
        method: "PUT",
        path_and_query: "/bots/BOT1234567/botaliases",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "CreateResourcePolicy, on a segment only this model publishes",
        method: "POST",
        path_and_query: "/policy/arn%3Aaws%3Alex%3Aus-east-1%3A000000000000%3Abot%2FBOT1234567",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "ListTestSets, likewise",
        method: "POST",
        path_and_query: "/testsets",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "CreateUploadUrl, likewise",
        method: "POST",
        path_and_query: "/createuploadurl",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "ListBuiltInIntents, inside the builtins space the V1 model shares",
        method: "POST",
        path_and_query: "/builtins/locales/en_US/intents",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "CreateExport, inside the exports space the V1 model shares",
        method: "PUT",
        path_and_query: "/exports",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "DescribeExport, likewise",
        method: "GET",
        path_and_query: "/exports/EXPORT1234",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        // Both control planes bind the identical import and tagging routes, and this
        // model is the declared owner of both spaces.
        name: "ListImports, on the import space both control planes bind",
        method: "POST",
        path_and_query: "/imports",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "ListTagsForResource, on the tagging space both control planes bind",
        method: "GET",
        path_and_query: "/tags/arn%3Aaws%3Alex%3Aus-east-1%3A000000000000%3Abot%2FBOT1234567",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        // The one route shape BOTH control planes bind, split by the label's modelled
        // shape: this model's `Id` is @length(min: 10, max: 10) @pattern(^[0-9a-zA-Z]+$).
        name: "DeleteBot on a label shaped like this model's botId",
        method: "DELETE",
        path_and_query: "/bots/BOT1234567",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "DeleteBot on a ten-digit botId",
        method: "DELETE",
        path_and_query: "/bots/0000000000",
        signing_name: Some("lex"),
        claimed: true,
    },
    Case {
        name: "DeleteBot on a V1 bot name, which is not a botId",
        method: "DELETE",
        path_and_query: "/bots/DoomedBot",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "DeleteBot on a ten-character V1 name carrying an underscore",
        method: "DELETE",
        path_and_query: "/bots/Order_Flow",
        signing_name: Some("lex"),
        claimed: false,
    },
    // Each sibling's own path space, which this service must NOT claim.
    Case {
        name: "the V2 runtime's RecognizeText, on its camelCase session path",
        method: "POST",
        path_and_query: "/bots/BOT1234567/botAliases/TSTALIASID/botLocales/en_US/sessions/s-1/text",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "the V2 runtime's GetSession",
        method: "GET",
        path_and_query: "/bots/BOT1234567/botAliases/TSTALIASID/botLocales/en_US/sessions/s-1",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "the V1 runtime's PostText, on the singular bot segment",
        method: "POST",
        path_and_query: "/bot/OrderFlowers/alias/PROD/user/user-1/text",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "the V1 control plane's GetBots, the /bots collection under GET",
        method: "GET",
        path_and_query: "/bots",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "the V1 control plane's PutBot",
        method: "PUT",
        path_and_query: "/bots/OrderFlowers/versions/$LATEST",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "the V1 control plane's CreateBotVersion, on its versions segment",
        method: "POST",
        path_and_query: "/bots/OrderFlowers/versions",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "the V1 control plane's GetBotAliases, on its aliases segment",
        method: "GET",
        path_and_query: "/bots/OrderFlowers/aliases",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "the V1 control plane's GetIntents, on a segment this model never binds",
        method: "GET",
        path_and_query: "/intents",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "the V1 control plane's GetBuiltinIntents, inside the shared builtins space",
        method: "GET",
        path_and_query: "/builtins/intents",
        signing_name: Some("lex"),
        claimed: false,
    },
    // Near misses and requests that name another service or none.
    Case {
        name: "a path inside the shared bots space that no lex model binds",
        method: "POST",
        path_and_query: "/bots/BOT1234567/no-such-operation",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "near miss /botsextra",
        method: "PUT",
        path_and_query: "/botsextra",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "near miss /Bots in the wrong case",
        method: "PUT",
        path_and_query: "/Bots",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        // Every operation of this model is bound to a URI, so it claims no root post.
        name: "the root path",
        method: "POST",
        path_and_query: "/",
        signing_name: Some("lex"),
        claimed: false,
    },
    Case {
        name: "another service's signing name on this service's path",
        method: "PUT",
        path_and_query: "/bots",
        signing_name: Some("s3"),
        claimed: false,
    },
    Case {
        // No credential scope and a loopback host name no service.
        name: "no routing signal at all",
        method: "PUT",
        path_and_query: "/bots",
        signing_name: None,
        claimed: false,
    },
];

#[test]
fn only_this_services_own_wire_surface_is_claimed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    for case in CASES {
        let parts = parts(case.method, case.path_and_query, case.signing_name);
        let claimed = service.can_handle(&parts);
        assert_eq!(
            claimed, case.claimed,
            "case {:?}: expected can_handle({} {}) to be {}, got {claimed}",
            case.name, case.method, case.path_and_query, case.claimed
        );
    }
}
