// SPDX-License-Identifier: CC0-1.0

//! The routes BOTH Amazon Lex model-building API versions bind, read off the two
//! generated route tables rather than off a reading of the two models.
//!
//! This service and Lex Models V2 share the `lex` SigV4 signing name, so each claims a
//! request only when its OWN model binds that request's method and URI
//! (`service_common::lex::claims_request`, which narrows
//! `service_common::claims_request` with the family's shared-segment split). A handful of
//! routes are bound by BOTH models, and for those the route cannot decide: the family's
//! split does — a segment one member owns, or the label shape on the one route shape they
//! share.
//!
//! That split is the one place a fact about two models is stated in code, so it can drift
//! when either model changes. This suite is what makes the drift loud: for each row it
//! asks BOTH services' generated protocol route tables whether they bind the request
//! (`service_common::addresses_modeled_route`, the same reader the claim uses) and then
//! asks each service's own `Service::can_handle` — the shipped claim, not a recomposition
//! of it — so that two properties are pinned on the emulator's one endpoint AND on the V1
//! endpoint host: NO request ever has two claimants (the `500 AmbiguousService` the Lex
//! family's routing issue was filed for), and a route either model binds always has one.
//! A row's expected per-service binding is asserted too, so a probe that stops addressing
//! the route it was written for fails instead of silently passing.
//!
//! The rows span the class: the routes both models bind, and — for each first path segment
//! the two models share (`bots`, `imports`, `exports`, `builtins`, `tags`) — routes only
//! one of them binds, including the pairs that differ only in method, only in depth, or
//! only in one literal segment, plus two paths around the shared tag routes that neither
//! model binds (an omitted `@required` query member, and an ARN carrying a raw `/`).

use smithy_server_runtime::Service;
use std::sync::Arc;

use service_com_amazonaws_lexmodelbuildingservice_awsdeepsensemodelbuildingservice::EmulatedService as V1Service;
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::EmulatedService as V2Service;

/// A SigV4 credential scope naming `lex`, the signing name both versions share.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260102/us-east-1/lex/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// A tagging ARN with no `/` in it, so the whole ARN is ONE path segment.
const ARN: &str = "arn:aws:lex:us-east-1:000000000000:bot:MyBot";

/// The one endpoint a client points at the emulator with, naming no endpoint prefix.
const EMULATOR_HOST: &str = "localhost:9297";

/// The V1 model-building service's own endpoint host.
const V1_MODELS_HOST: &str = "models.lex.us-east-1.amazonaws.com";

/// The request parts a claim is decided on.
fn parts_for(method: &str, path: &str, host: &str) -> http::request::Parts {
    http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", AUTHORIZATION)
        .body(())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

#[test]
fn the_shared_route_predicate_is_the_intersection_of_the_two_generated_route_tables() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let v1 = V1Service::new(Arc::clone(&engine));
    let v2 = V2Service::new(engine);

    // `GET`/`POST /tags/{arn}` carry no query binding; `UntagResource` binds a REQUIRED
    // `tagKeys` query member in BOTH models, so its probe carries one — without it
    // neither model's route is a legal target for the request.
    let tags_path = format!("/tags/{ARN}");
    let untag_path = format!("/tags/{ARN}?tagKeys=Owner");
    // A `{resourceArn}` label is NOT greedy in either model, so an ARN whose resource
    // part carries a RAW `/` (the Lex V2 spelling, `…:bot/MYBOT`, which an SDK sends
    // percent-encoded as one segment) is one segment too many for both route tables —
    // neither binds it, so the tie-break must not fire for it either.
    let slashed_arn_path = "/tags/arn:aws:lex:us-east-1:000000000000:bot/MyBot";

    // (case, method, path, bound by V1, bound by V2)
    let rows: [(&str, &str, &str, bool, bool); 22] = [
        // The six routes both models bind: `DeleteBot`/`DeleteBot`,
        // `StartImport`/`ListImports`, `GetImport`/`DescribeImport` and the three
        // tagging operations.
        ("DeleteBot, bound twice", "DELETE", "/bots/MyBot", true, true),
        ("import start vs import list", "POST", "/imports", true, true),
        ("GetImport vs DescribeImport", "GET", "/imports/IMPORT1234", true, true),
        ("ListTagsForResource", "GET", tags_path.as_str(), true, true),
        ("TagResource", "POST", tags_path.as_str(), true, true),
        ("UntagResource", "DELETE", untag_path.as_str(), true, true),
        // Paths around the shared tag routes that neither model binds, so the
        // "exactly one claimant" invariant below must hold for them as "no tie to
        // break" rather than as a shared route.
        (
            "UntagResource without its required tagKeys query",
            "DELETE",
            tags_path.as_str(),
            false,
            false,
        ),
        (
            "a tags path whose ARN carries a raw slash",
            "GET",
            slashed_arn_path,
            false,
            false,
        ),
        // Routes only ONE version binds, under the first segments they share.
        ("V2 CreateBot", "PUT", "/bots", false, true),
        ("V2 ListBots", "POST", "/bots", false, true),
        ("V1 GetBots", "GET", "/bots", true, false),
        ("V2 DescribeBot", "GET", "/bots/ABCDEFGHIJ", false, true),
        ("V2 UpdateBot", "PUT", "/bots/ABCDEFGHIJ", false, true),
        ("V1 GetBot", "GET", "/bots/MyBot/versions/$LATEST", true, false),
        ("V1 PutBot", "PUT", "/bots/MyBot/versions/$LATEST", true, false),
        (
            "V1 DeleteBotVersion vs the V2 spelling",
            "DELETE",
            "/bots/MyBot/versions/1",
            true,
            false,
        ),
        (
            "V2 DeleteBotVersion",
            "DELETE",
            "/bots/ABCDEFGHIJ/botversions/1",
            false,
            true,
        ),
        ("V1 GetBotAliases", "GET", "/bots/MyBot/aliases", true, false),
        (
            "V1 DeleteUtterances, one segment deeper than V2's",
            "DELETE",
            "/bots/MyBot/utterances/user-1",
            true,
            false,
        ),
        (
            "V2 DeleteUtterances",
            "DELETE",
            "/bots/ABCDEFGHIJ/utterances",
            false,
            true,
        ),
        ("V1 GetBuiltinIntents", "GET", "/builtins/intents", true, false),
        (
            "V2 ListBuiltInIntents",
            "POST",
            "/builtins/locales/en_US/intents",
            false,
            true,
        ),
    ];

    // The one fact about the HOST this suite needs, asserted once: the two hosts the rows
    // are driven on really are two different endpoints — the V1 model-building endpoint
    // prefix (`awsService.endpointPrefix` = `models.lex`, from
    // `smithy_query(query="service_info")` on this crate's model) and a host naming no
    // prefix at all — so the per-row comparison of the claims on each of them is a
    // comparison of two endpoints and not of one.
    assert!(
        V1_MODELS_HOST.starts_with("models.lex."),
        "{V1_MODELS_HOST} must address the V1 model-building endpoint prefix"
    );
    assert!(
        !EMULATOR_HOST.contains("lex"),
        "{EMULATOR_HOST}, the one endpoint a client points at the emulator with, names no \
         endpoint prefix"
    );

    for (case, method, path, by_v1, by_v2) in rows {
        // Which model BINDS the route does not depend on the endpoint, so it is read
        // once, off the one host a client pointed at the emulator sends.
        let parts = parts_for(method, path, EMULATOR_HOST);
        let v1_binds = service_common::addresses_modeled_route(&parts, v1.definition());
        let v2_binds = service_common::addresses_modeled_route(&parts, v2.definition());
        assert_eq!(
            (v1_binds, v2_binds),
            (by_v1, by_v2),
            "case {case:?}: {method} {path} expected (bound by V1, bound by V2) == \
             ({by_v1}, {by_v2}), got ({v1_binds}, {v2_binds})"
        );

        // The SHIPPED claim rule, on each endpoint: no request may have TWO claimants
        // (the `500 AmbiguousService` the Lex routing issue was filed for), and a route
        // either model binds must have one. Asked of each service's own `can_handle`
        // rather than recomposed here, so a change to either crate's claim composition is
        // caught.
        //
        // Two converses are deliberately NOT asserted. A segment the family's split hands
        // to an OWNER (`/imports`, `/tags/{arn}`) is claimed by that owner even for a
        // request its route table does not bind — `UntagResource` without its `@required
        // tagKeys` query, say — so the owner answers its own protocol error instead of the
        // router's `404 UnknownService`. And the claim does not depend on the endpoint:
        // this family splits the routes two members bind identically by the shape of the
        // label in the path, so addressing the V1 endpoint host changes nothing, which the
        // comparison after the loop pins.
        let mut claims: Vec<(bool, bool)> = Vec::new();
        for host in [EMULATOR_HOST, V1_MODELS_HOST] {
            let addressed = parts_for(method, path, host);
            let v1_claims = v1.can_handle(&addressed);
            let v2_claims = v2.can_handle(&addressed);
            let claimants = usize::from(v1_claims) + usize::from(v2_claims);
            assert!(
                claimants <= 1,
                "case {case:?}: {method} {path} on {host:?} has TWO claimants, which the \
                 router answers `500 AmbiguousService`; got V1 {v1_claims}, V2 \
                 {v2_claims}"
            );
            assert!(
                !(v1_binds || v2_binds) || claimants == 1,
                "case {case:?}: {method} {path} on {host:?} is bound by a model but no \
                 service claims it, so it reaches the router's 404; got V1 {v1_claims}, \
                 V2 {v2_claims}"
            );
            claims.push((v1_claims, v2_claims));
        }
        let (on_emulator_host, on_v1_host) = (claims[0], claims[1]);
        assert_eq!(
            on_emulator_host, on_v1_host,
            "case {case:?}: {method} {path} is claimed differently on the two endpoint \
             hosts, but this family's split reads the path and not the host"
        );
    }
}
