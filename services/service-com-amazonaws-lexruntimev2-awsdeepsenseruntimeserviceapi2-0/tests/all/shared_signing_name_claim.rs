// SPDX-License-Identifier: CC0-1.0

//! Which requests this service claims, when its SigV4 signing name alone cannot tell it
//! from its three siblings.
//!
//! `lex` is declared as the `aws.auth#sigv4` name by FOUR emulated services — this
//! runtime, Lex Models V2, and the V1 model-building and runtime services — each serving
//! its own endpoint. The router resolves a request's routing key from the SigV4
//! credential scope first, so all four see `lex`, and a service that claims on the
//! signing name alone claims its siblings' traffic too: the router then has four
//! claimants for every Lex request and refuses all of them with `500 AmbiguousService`.
//!
//! The rule under test is the claim this crate's `can_handle` applies: a request is this
//! service's when the signing name matches AND THIS service's model binds the request's
//! method and URI. The table below spans the class — every method this model binds, the
//! siblings' routes under the same first path segment, a sibling's spelling of a nested
//! collection, a method no Lex model binds on this service's own path, and another
//! service's signing name — rather than the one recorded request.
//!
//! The complementary half (that each row IS answered by its owner once all four services
//! are mounted) is asserted through the whole router in
//! `core/router/tests/all/lex_signing_name_routing.rs`.

use smithy_server_runtime::Service;
use std::sync::Arc;

use service_com_amazonaws_lexruntimev2_awsdeepsenseruntimeserviceapi2_0::EmulatedService;

/// A SigV4 credential scope naming `lex`, the signing name all four Lex services share.
const LEX_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260102/us-east-1/lex/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// The same scope naming another service entirely, which this service must never claim.
const OTHER_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260102/us-east-1/sts/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// A session address inside this model's `@httpLabel` constraints.
const SESSION: &str = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/sess-1";

/// The request parts the `can_handle` seam is handed.
fn parts(method: &str, path: &str, authorization: &str) -> http::request::Parts {
    http::Request::builder()
        .method(method)
        .uri(format!("https://localhost:9297{path}"))
        .header("host", "localhost:9297")
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

#[test]
fn this_runtime_claims_its_own_modelled_routes_and_no_sibling_lex_route() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    // (case, method, path, authorization, claimed)
    //
    // The routes this model binds come from
    // `smithy_query(query="digest", model_path="…lexruntimev2….json")`: every operation
    // is bound under
    // `/bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}`,
    // with `/text` and `/utterance` below it. The sibling rows come from the same query
    // on the other three Lex models.
    let rows: [(&str, &str, &str, &str, bool); 15] = [
        ("its own PutSession route", "POST", SESSION, LEX_AUTHORIZATION, true),
        ("its own GetSession route", "GET", SESSION, LEX_AUTHORIZATION, true),
        ("its own DeleteSession route", "DELETE", SESSION, LEX_AUTHORIZATION, true),
        (
            "its own RecognizeText route",
            "POST",
            "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/sess-1/text",
            LEX_AUTHORIZATION,
            true,
        ),
        (
            "its own RecognizeUtterance route",
            "POST",
            "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/sess-1/utterance",
            LEX_AUTHORIZATION,
            true,
        ),
        // A method no Lex model binds on this service's own path: nothing to serve, so
        // the claim is not this service's to make.
        ("a method this model does not bind", "PUT", SESSION, LEX_AUTHORIZATION, false),
        // The Lex Models V2 control plane's routes, all under the same first segment.
        ("the V2 CreateBot route", "PUT", "/bots", LEX_AUTHORIZATION, false),
        ("the V2 ListBots route", "POST", "/bots", LEX_AUTHORIZATION, false),
        (
            "the V2 DescribeBot route",
            "GET",
            "/bots/ABCDEFGHIJ",
            LEX_AUTHORIZATION,
            false,
        ),
        (
            "the V2 DescribeIntent route",
            "GET",
            "/bots/ABCDEFGHIJ/botversions/DRAFT/botlocales/en_US/intents/INTENT1234",
            LEX_AUTHORIZATION,
            false,
        ),
        // The V1 model-building service's routes.
        ("the V1 GetBots route", "GET", "/bots", LEX_AUTHORIZATION, false),
        (
            "the V1 GetBot route",
            "GET",
            "/bots/MyBot/versions/$LATEST",
            LEX_AUTHORIZATION,
            false,
        ),
        // The V1 runtime service's route: the singular `/bot` space.
        (
            "the V1 runtime PostText route",
            "POST",
            "/bot/MyBot/alias/prod/user/user-1/text",
            LEX_AUTHORIZATION,
            false,
        ),
        // This service's own shape of path, spelled the way the V2 model spells its
        // nested collections. Literal segments are matched exactly, as HTTP path
        // semantics require, so this is NOT this service's route.
        (
            "the V2 spelling of the alias collection",
            "POST",
            "/bots/ABCDEFGHIJ/botaliases/TSTALIASID/botlocales/en_US/sessions/sess-1/text",
            LEX_AUTHORIZATION,
            false,
        ),
        // Its own route, signed for another service: the signing name still has to match.
        ("its own route, signed for another service", "GET", SESSION, OTHER_AUTHORIZATION, false),
    ];

    for (case, method, path, authorization, expected) in rows {
        let actual = service.can_handle(&parts(method, path, authorization));
        assert_eq!(
            actual, expected,
            "case {case:?}: {method} {path} expected claimed == {expected}, got {actual}"
        );
    }
}
