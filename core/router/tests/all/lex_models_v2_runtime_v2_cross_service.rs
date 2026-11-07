// SPDX-License-Identifier: CC0-1.0

//! The CROSS-SERVICE Lex V2 sequence: Lex Models V2 builds a bot, then Lex Runtime V2
//! converses with that same bot — both signed `lex`, both addressed at ONE emulator
//! endpoint, and both inside the ONE URI path space (`bots`) their models share.
//!
//! RULE: when two services that share a SigV4 signing name are addressed through one
//! endpoint, each call is served by the service whose OWN model binds its route, whichever
//! service minted the resource the URI names — so the control-plane routes of a bot reach
//! the control plane, the data-plane routes of that SAME `botId` reach the data plane, each
//! answers out of its own state, and no call can be answered by the router's
//! `AmbiguousService` (several claimants) or `UnknownService` (none).
//!
//! `lex_signing_name_routing.rs` pins the claim table itself — who claims which route, and
//! how the five shared first segments are split. This suite pins the SEQUENCE the
//! cross-service behavioral checks drive
//! (`lex-models-v2__lex-runtime-v2.recognize-text-*`), which no single-service suite can
//! see: `CreateBot`, the locale/slot-type/intent/build calls on the control plane, and
//! then `RecognizeText` and `GetSession` on the runtime, all against one engine and one
//! `botId`. Every one of those calls was answered `500 AmbiguousService` while each Lex
//! crate claimed on the signing name alone, so the checks died on their first call and
//! nothing after it was ever served.
//!
//! The rows span what the rule generalises over rather than one recorded request: three
//! bots (so three minted `botId`s), three locales, and session ids at both ends of
//! `SessionId`'s own bounds (`@length(2, 100)`, `@pattern(^[0-9a-zA-Z._:-]+$)`, from
//! `smithy_query(query="operation", operation="RecognizeText")` on the runtime model).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so the harness,
//! the service set and the dispatch helper are the ones `lex_signing_name_routing.rs`
//! owns, read through `pub(crate)` rather than copied.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::lex_signing_name_routing::{
    build_router, claimants, dispatch, lex_family, Answer, MODELS_V2, ROLE_ARN, ROUTER_REFUSALS,
    RUNTIME_V2,
};

/// The bot version every Lex V2 build sequence works on: the model binds
/// `@pattern(^DRAFT$)` for the `botVersion` label of `CreateBotLocale`, `CreateSlotType`,
/// `CreateIntent` and `BuildBotLocale`
/// (`smithy_query(query="operation", operation="CreateBotLocale")`).
const DRAFT: &str = "DRAFT";

/// The alias a client talks to a draft bot through — the one `CreateBot` provisions with
/// the bot, and the `botAliasId` label the runtime's own URIs carry.
const TEST_ALIAS: &str = "TSTALIASID";

/// One cross-service run: a bot built on the control plane and then talked to on the
/// runtime.
struct Run {
    case: &'static str,
    /// `Name`: `@length(1, 100)`, `@pattern(^([0-9a-zA-Z][_-]?){1,100}$)`.
    bot_name: &'static str,
    /// The locale the bot's draft locale is built in, as `localeId` carries it.
    locale: &'static str,
    /// The conversation's `sessionId`.
    session: String,
}

/// The runs: three bots, three locales, and session ids spanning `SessionId`'s bounds and
/// the punctuation its pattern admits.
fn runs() -> Vec<Run> {
    vec![
        Run {
            case: "en_US with an ordinary session id",
            bot_name: "CrossServiceOne",
            locale: "en_US",
            session: "cross-service-session-1".to_string(),
        },
        Run {
            case: "de_DE with the shortest session id SessionId allows",
            bot_name: "CrossServiceTwo",
            locale: "de_DE",
            // `@length(min: 2)`.
            session: "ab".to_string(),
        },
        Run {
            case: "ja_JP with the longest session id SessionId allows, and its punctuation",
            bot_name: "CrossServiceThree",
            locale: "ja_JP",
            // `@length(max: 100)`, built out of the characters
            // `^[0-9a-zA-Z._:-]+$` admits.
            session: format!("{}{}", "a.b_c:d-1".repeat(11), "0"),
        },
    ]
}

/// One call of the sequence: its route, and the ONE emulated service whose model binds it.
struct Step {
    case: String,
    method: &'static str,
    path: String,
    body: String,
    /// The shape id of the service that must claim it — and the only one.
    claimant: &'static str,
}

/// The control-plane calls the checks make between `CreateBot` and the first utterance,
/// on the bot `bot_id` in locale `locale`.
///
/// Routes from `smithy_query(query="operation", operation="CreateBotLocale,CreateSlotType,
/// CreateIntent,BuildBotLocale,DescribeBotLocale")` on the Lex Models V2 model. Their
/// HTTP answers are not asserted here: several are still scaffolded 501s with their own
/// stub issues, and what this suite is about is that each one reaches the Lex Models V2
/// service rather than the router's refusal.
fn build_steps(bot_id: &str, locale: &str) -> Vec<Step> {
    let locales = format!("/bots/{bot_id}/botversions/{DRAFT}/botlocales");
    vec![
        Step {
            case: format!("CreateBotLocale {locale}"),
            method: "PUT",
            path: locales.clone(),
            body: json!({ "localeId": locale, "nluIntentConfidenceThreshold": 0.4 }).to_string(),
        claimant: MODELS_V2,
        },
        Step {
            case: format!("CreateSlotType in {locale}"),
            method: "PUT",
            path: format!("{locales}/{locale}/slottypes"),
            body: json!({
                "slotTypeName": "PizzaSize",
                "slotTypeValues": [{
                    "sampleValue": { "value": "large" },
                    "synonyms": [{ "value": "big" }],
                }],
                "valueSelectionSetting": {
                    "resolutionStrategy": "TopResolution",
                },
            })
            .to_string(),
            claimant: MODELS_V2,
        },
        Step {
            case: format!("CreateIntent in {locale}"),
            method: "PUT",
            path: format!("{locales}/{locale}/intents"),
            body: json!({
                "intentName": "OrderPizza",
                "sampleUtterances": [{ "utterance": "I want a {PizzaSize} pizza" }],
            })
            .to_string(),
            claimant: MODELS_V2,
        },
        Step {
            case: format!("BuildBotLocale {locale}"),
            method: "POST",
            path: format!("{locales}/{locale}"),
            body: String::new(),
            claimant: MODELS_V2,
        },
        Step {
            case: format!("DescribeBotLocale {locale}"),
            method: "GET",
            path: format!("{locales}/{locale}"),
            body: String::new(),
            claimant: MODELS_V2,
        },
    ]
}

/// The session URI the runtime's own operations hang off, for the same bot.
fn session_path(bot_id: &str, locale: &str, session: &str) -> String {
    format!("/bots/{bot_id}/botAliases/{TEST_ALIAS}/botLocales/{locale}/sessions/{session}")
}

/// Assert that exactly one mounted service claims `step`, and that it is the one whose
/// model binds the route.
fn assert_sole_claimant(family: &[Box<dyn smithy_server_runtime::Service>], step: &Step) {
    let claiming = claimants(family, step.method, &step.path);
    assert_eq!(
        claiming,
        vec![step.claimant.to_string()],
        "case {:?}: {} {} must be claimed by {} alone, got {claiming:?}",
        step.case,
        step.method,
        step.path,
        step.claimant
    );
}

/// Wait for the `Built` status the submitted build owes the locale, and answer what
/// `DescribeBotLocale` then reports.
///
/// The runtime interprets an utterance against the model a build produced, so a
/// conversation can only be held once the locale is `Built` — the status AWS documents as
/// the one where "the bot is ready for use and can be tested using any utterance". The
/// control plane materialises it at the instant the clock makes it due, which a read is
/// what observes.
pub(crate) async fn wait_until_built(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale: &str,
) -> Value {
    let uri = format!("/bots/{bot_id}/botversions/{DRAFT}/botlocales/{locale}");
    let mut described = Value::Null;
    for _ in 0..50 {
        let answer = dispatch(router, "GET", &uri, "").await;
        described = answer.body;
        if described.get("botLocaleStatus").and_then(Value::as_str) == Some("Built") {
            return described;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the locale {locale} of bot {bot_id} must reach Built, last saw {described:?}");
}

/// Author a locale with one intent carrying a sample utterance, build it, and wait for the
/// `Built` the build owes — everything the runtime needs before a turn can be served.
pub(crate) async fn author_and_build_locale(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale: &str,
) {
    for step in build_steps(bot_id, locale) {
        let answer = dispatch(router, step.method, &step.path, &step.body).await;
        assert_not_a_router_refusal(&step.case, &answer);
    }
    wait_until_built(router, bot_id, locale).await;
}

/// Assert that `answer` came from a service rather than from the router's own refusals.
pub(crate) fn assert_not_a_router_refusal(case: &str, answer: &Answer) {
    assert!(
        !ROUTER_REFUSALS.contains(&answer.error_code.as_str()),
        "case {case:?}: the call must be served by exactly one of the services sharing the \
         `lex` signing name, got HTTP {} {:?}: {}",
        answer.status,
        answer.error_code,
        answer.raw_body
    );
}

/// RULE: every call of the build-then-talk sequence is claimed by exactly one service —
/// the one whose own model binds its route — and served by it, so a bot the control plane
/// minted can be conversed with on the runtime through the same endpoint and signing name.
#[tokio::test]
async fn the_cross_service_build_then_talk_sequence_reaches_one_service_per_call() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);
    let family = lex_family(&engine);

    for run in runs() {
        let case = run.case;

        // ---- the control plane mints the bot ---------------------------------------
        let create_bot = json!({
            "botName": run.bot_name,
            "roleArn": ROLE_ARN,
            "dataPrivacy": { "childDirected": false },
            "idleSessionTTLInSeconds": 300,
            "botType": "Bot",
        })
        .to_string();
        let create_step = Step {
            case: format!("{case}: CreateBot"),
            method: "PUT",
            path: "/bots".to_string(),
            body: create_bot,
            claimant: MODELS_V2,
        };
        assert_sole_claimant(&family, &create_step);
        let created = dispatch(&router, "PUT", &create_step.path, &create_step.body).await;
        assert_not_a_router_refusal(&create_step.case, &created);
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

        // ---- the control plane's own build calls, on that bot ----------------------
        for step in build_steps(&bot_id, run.locale) {
            assert_sole_claimant(&family, &step);
            let answer = dispatch(&router, step.method, &step.path, &step.body).await;
            assert_not_a_router_refusal(&step.case, &answer);
        }
        // The runtime serves a turn against the model the build produced, so the
        // conversation below waits for the locale to reach `Built`.
        wait_until_built(&router, &bot_id, run.locale).await;

        // ---- the runtime talks to the SAME bot ------------------------------------
        let session = session_path(&bot_id, run.locale, &run.session);
        let recognize = Step {
            case: format!("{case}: RecognizeText"),
            method: "POST",
            path: format!("{session}/text"),
            body: json!({ "text": "I want a big pizza" }).to_string(),
            claimant: RUNTIME_V2,
        };
        assert_sole_claimant(&family, &recognize);
        let spoken = dispatch(&router, "POST", &recognize.path, &recognize.body).await;
        assert_not_a_router_refusal(&recognize.case, &spoken);
        assert_eq!(
            spoken.status, 200,
            "case {case:?}: RecognizeText must answer the 200 its model declares, got {} \
             {:?}: {}",
            spoken.status, spoken.error_code, spoken.raw_body
        );
        // `interpretations` and `sessionState` are the runtime's own output members: only
        // the Lex Runtime V2 service can answer with them, so their presence is what says
        // the data plane — not a control plane sharing the `bots` space — served the turn.
        assert!(
            spoken.body.get("interpretations").is_some_and(Value::is_array),
            "case {case:?}: RecognizeText must report the turn's interpretations, got {}",
            spoken.raw_body
        );
        assert_eq!(
            spoken.body.get("sessionId").and_then(Value::as_str),
            Some(run.session.as_str()),
            "case {case:?}: RecognizeText must answer for the session it addressed, got {}",
            spoken.raw_body
        );

        // ---- and the runtime's own state reads back through the runtime -----------
        let read_session = Step {
            case: format!("{case}: GetSession"),
            method: "GET",
            path: session.clone(),
            body: String::new(),
            claimant: RUNTIME_V2,
        };
        assert_sole_claimant(&family, &read_session);
        let fetched = dispatch(&router, "GET", &read_session.path, "").await;
        assert_not_a_router_refusal(&read_session.case, &fetched);
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetSession must report the session the turn started, got {} \
             {:?}: {}",
            fetched.status, fetched.error_code, fetched.raw_body
        );
        assert_eq!(
            fetched.body.get("sessionId").and_then(Value::as_str),
            Some(run.session.as_str()),
            "case {case:?}: GetSession must report the session that was addressed, got {}",
            fetched.raw_body
        );
        assert!(
            fetched.body.get("sessionState").is_some(),
            "case {case:?}: GetSession must report the dialog state the turn persisted, got {}",
            fetched.raw_body
        );

        // ---- the control plane's record of the bot is untouched by the conversation -
        let reread = dispatch(&router, "GET", &format!("/bots/{bot_id}"), "").await;
        assert_not_a_router_refusal(&format!("{case}: DescribeBot after the turn"), &reread);
        assert_eq!(
            reread.status, 200,
            "case {case:?}: DescribeBot must still answer 200 after the conversation, got \
             {} {:?}: {}",
            reread.status, reread.error_code, reread.raw_body
        );
        assert_eq!(
            reread.body.get("botName").and_then(Value::as_str),
            Some(run.bot_name),
            "case {case:?}: the runtime's traffic must not change the control plane's \
             record of the bot, got {}",
            reread.raw_body
        );
    }
}

/// RULE: inside the `bots` space the two V2 services share, what separates them is the
/// modelled route, and a URI segment is matched EXACTLY — as HTTP path semantics and the
/// framework's route matcher both do — so the control plane's lower-case `botversions` /
/// `botaliases` and the runtime's camel-case `botAliases` / `botLocales` / `sessions`
/// address different services under one and the same `botId`.
#[tokio::test]
async fn the_same_bot_id_splits_between_the_two_services_by_the_route_alone() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);
    let family = lex_family(&engine);

    // One bot, minted by the control plane, whose id then appears in both services' URIs.
    let created = dispatch(
        &router,
        "PUT",
        "/bots",
        &json!({
            "botName": "CrossServiceSplit",
            "roleArn": ROLE_ARN,
            "dataPrivacy": { "childDirected": false },
            "idleSessionTTLInSeconds": 60,
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the bot the rows address must be created, got {} {:?}: {}",
        created.status, created.error_code, created.raw_body
    );
    let bot_id = created
        .body
        .get("botId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no botId in {}", created.raw_body))
        .to_string();

    // Rows: the same bot addressed on both sides of the split, differing only in the
    // literal segments each model binds (and, for two of them, only in their CASE).
    let steps = vec![
        Step {
            case: "the control plane's bot versions".to_string(),
            method: "GET",
            path: format!("/bots/{bot_id}/botversions/{DRAFT}"),
            body: String::new(),
            claimant: MODELS_V2,
        },
        Step {
            case: "the control plane's lower-case botaliases".to_string(),
            method: "GET",
            path: format!("/bots/{bot_id}/botaliases/{TEST_ALIAS}"),
            body: String::new(),
            claimant: MODELS_V2,
        },
        Step {
            case: "the control plane's lower-case botlocales".to_string(),
            method: "GET",
            path: format!("/bots/{bot_id}/botversions/{DRAFT}/botlocales/en_US"),
            body: String::new(),
            claimant: MODELS_V2,
        },
        Step {
            case: "the runtime's camel-case session URI".to_string(),
            method: "GET",
            path: session_path(&bot_id, "en_US", "split-session"),
            body: String::new(),
            claimant: RUNTIME_V2,
        },
        Step {
            case: "the runtime's text turn".to_string(),
            method: "POST",
            path: format!("{}/text", session_path(&bot_id, "en_US", "split-session")),
            body: json!({ "text": "hello" }).to_string(),
            claimant: RUNTIME_V2,
        },
        Step {
            case: "the runtime's session delete".to_string(),
            method: "DELETE",
            path: session_path(&bot_id, "en_US", "split-session"),
            body: String::new(),
            claimant: RUNTIME_V2,
        },
    ];

    for step in &steps {
        assert_sole_claimant(&family, step);
        let answer = dispatch(&router, step.method, &step.path, &step.body).await;
        assert_not_a_router_refusal(&step.case, &answer);
    }

    // `RecognizeUtterance` is the one runtime route whose claim needs more than the
    // method and the URI: its `requestContentType` is a `@required`
    // `@httpHeader("Content-Type")` (`smithy_query(query="operation",
    // operation="RecognizeUtterance")`), so the route matches only on a request that
    // carries one — which is what a client, and `dispatch`, sends. It is therefore pinned
    // through the router rather than through the header-less claim probe.
    let utterance = format!(
        "{}/utterance",
        session_path(&bot_id, "en_US", "split-session")
    );
    let answer = dispatch(&router, "POST", &utterance, "").await;
    assert_not_a_router_refusal("the runtime's utterance turn", &answer);
}
