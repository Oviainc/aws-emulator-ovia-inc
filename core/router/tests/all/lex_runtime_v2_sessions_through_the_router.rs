// SPDX-License-Identifier: CC0-1.0

//! The Lex Runtime V2 SESSION operations, driven through the whole four-service `lex`
//! family rather than against that one crate on its own.
//!
//! `aws.auth#sigv4` names `lex` on all four Lex-family models while each serves its own
//! endpoint (`smithy_query(query="service_info")` gives `models-v2-lex`, `runtime-v2-lex`,
//! `models.lex` and `runtime.lex` against that ONE signing name). The router resolves a
//! request's routing key from the SigV4 credential scope first, so all four see the key
//! `lex`: while each claimed on the signing name ALONE, every Lex request had four
//! claimants and the router — refusing to serve a request by an arbitrary one — answered
//! `500 AmbiguousService`, a code no AWS model carries. Lex Runtime V2 was therefore
//! unreachable: no `RecognizeText`, `RecognizeUtterance`, `PutSession`, `GetSession` or
//! `DeleteSession` request was ever served, and because the refusal is a 5xx an SDK
//! retried it to its ceiling.
//!
//! RULE: a request signed under a signing name several services share is claimed by the
//! one whose OWN model binds the request's route (method + URI pattern, which is what
//! distinguishes `restJson1` APIs on the wire) and is served by it — so the runtime's
//! session operations reach the runtime even though the control planes publish the same
//! first path segment, and no call can be answered by the router's `AmbiguousService`
//! (several claimants) or `UnknownService` (none).
//!
//! `lex_signing_name_routing.rs` pins the claim table itself, and
//! `lex_models_v2_runtime_v2_cross_service.rs` the build-then-talk sequence. What this
//! suite adds is the SESSION half of the data plane end to end through the whole family —
//! `PutSession` seeding the dialog state, `GetSession` reporting it, `RecognizeText`
//! advancing it without losing it, `DeleteSession` removing it and `GetSession` then
//! answering the modelled `404 ResourceNotFoundException`. That is what the
//! `lex-runtime-v2` behavioral checks drive, and every one of those calls was the
//! ambiguity refusal before: a single-service suite cannot see it, because with one
//! service mounted there is no second claimant to be ambiguous with.
//!
//! The rows span what the rule generalises over rather than one recorded request: three
//! bots (so three minted `botId`s), three locales, session ids at both ends of
//! `SessionId`'s own `@length(2, 100)` and over the punctuation its
//! `@pattern(^[0-9a-zA-Z._:-]+$)` admits, session-attribute maps of zero, one and three
//! entries, and zero, one and two `RecognizeText` turns before the state is re-read
//! (`smithy_query(query="operation", operation="PutSession,RecognizeText,GetSession,
//! DeleteSession")` on the runtime model).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so the harness, the
//! service set and the dispatch helper are the ones `lex_signing_name_routing.rs` owns,
//! read through `pub(crate)` rather than copied.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::lex_models_v2_runtime_v2_cross_service::author_and_build_locale;
use crate::lex_signing_name_routing::{
    build_router, claimants, dispatch, lex_family, Answer, MODELS_V2, ROLE_ARN, ROUTER_REFUSALS,
    RUNTIME_V2,
};

/// The alias a client talks to a draft bot through — the one `CreateBot` provisions with
/// the bot, and the `botAliasId` label the runtime's own URIs carry.
const TEST_ALIAS: &str = "TSTALIASID";

/// One conversation: a bot the control plane mints, and the session the runtime holds for
/// it.
struct Run {
    case: &'static str,
    /// `Name`: `@length(1, 100)`, `@pattern(^([0-9a-zA-Z][_-]?){1,100}$)`.
    bot_name: &'static str,
    /// The locale the session is in use in, as `localeId` (`@length(min: 1)`) carries it.
    locale: &'static str,
    /// The conversation's `sessionId`.
    session: String,
    /// `sessionState.sessionAttributes`, which `PutSession` seeds and every later read
    /// must report unchanged.
    attributes: Vec<(&'static str, &'static str)>,
    /// How many `RecognizeText` turns are taken before the state is re-read.
    turns: usize,
}

/// The runs: three bots, three locales, session ids spanning `SessionId`'s bounds and the
/// punctuation its pattern admits, attribute maps of three sizes, and three turn counts.
fn runs() -> Vec<Run> {
    vec![
        Run {
            case: "en_US, no attributes, no turn taken",
            bot_name: "RouterSessionOne",
            locale: "en_US",
            session: "router-session-1".to_string(),
            attributes: vec![],
            turns: 0,
        },
        Run {
            case: "de_DE, one attribute, one turn, shortest session id SessionId allows",
            bot_name: "RouterSessionTwo",
            locale: "de_DE",
            // `@length(min: 2)`.
            session: "ab".to_string(),
            attributes: vec![("cart", "2")],
            turns: 1,
        },
        Run {
            case: "ja_JP, three attributes, two turns, longest session id SessionId allows",
            bot_name: "RouterSessionThree",
            locale: "ja_JP",
            // `@length(max: 100)`, built from the characters `^[0-9a-zA-Z._:-]+$` admits.
            session: format!("{}{}", "a.b_c:d-1".repeat(11), "0"),
            attributes: vec![("cart", "2"), ("floor", ""), ("name", "Zoë")],
            turns: 2,
        },
    ]
}

/// `sessionState.sessionAttributes` as the wire carries it.
fn attribute_map(attributes: &[(&str, &str)]) -> Value {
    Value::Object(
        attributes
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect(),
    )
}

/// The session URI the runtime's own operations hang off.
fn session_path(bot_id: &str, locale: &str, session: &str) -> String {
    format!("/bots/{bot_id}/botAliases/{TEST_ALIAS}/botLocales/{locale}/sessions/{session}")
}

/// Assert that `answer` came from a service rather than from one of the router's own
/// refusals — the whole point of the claim narrowing this suite covers.
fn assert_served(case: &str, step: &str, answer: &Answer) {
    assert!(
        !ROUTER_REFUSALS.contains(&answer.error_code.as_str()),
        "case {case:?}: {step} must be served by exactly one of the four services sharing \
         the `lex` signing name, got HTTP {} {:?}: {}",
        answer.status,
        answer.error_code,
        answer.raw_body
    );
}

/// Assert that exactly one mounted service claims `method path`, and that it is `expected`.
fn assert_sole_claimant(
    case: &str,
    step: &str,
    family: &[Box<dyn smithy_server_runtime::Service>],
    method: &str,
    path: &str,
    expected: &str,
) {
    let claiming = claimants(family, method, path);
    assert_eq!(
        claiming,
        vec![expected.to_string()],
        "case {case:?}: {step} ({method} {path}) must be claimed by {expected} alone, got \
         {claiming:?}"
    );
}

/// RULE (above), over the whole table: with all four `lex`-signing services mounted at one
/// endpoint, every session call is claimed by the Lex Runtime V2 service alone and served
/// by it, so the dialog state `PutSession` seeds is reported by `GetSession`, survives each
/// `RecognizeText` turn, and is gone — the modelled `404 ResourceNotFoundException` — once
/// `DeleteSession` has removed it.
#[tokio::test]
async fn a_runtime_v2_session_is_seeded_advanced_read_and_deleted_through_the_whole_router() {
    for run in runs() {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = build_router(&engine);
        let family = lex_family(&engine);
        let case = run.case;

        // ---- the control plane mints the bot the conversation is about --------------
        assert_sole_claimant(case, "CreateBot", &family, "PUT", "/bots", MODELS_V2);
        let created = dispatch(
            &router,
            "PUT",
            "/bots",
            &json!({
                "botName": run.bot_name,
                "roleArn": ROLE_ARN,
                "dataPrivacy": { "childDirected": false },
                "idleSessionTTLInSeconds": 300,
                "botType": "Bot",
            })
            .to_string(),
        )
        .await;
        assert_served(case, "CreateBot", &created);
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

        let path = session_path(&bot_id, run.locale, &run.session);

        // ---- and the locale the conversation is held in, built ---------------------
        //
        // The runtime serves a call only against the model a build produced, so a bot can
        // only be conversed with once the locale is built: the authoring and the build are
        // the cross-service suite's, reused rather than copied.
        author_and_build_locale(&router, &bot_id, run.locale).await;

        // ---- every session route is the runtime's, and only the runtime's -----------
        //
        // The three methods the runtime binds on the session URI itself, plus the turn
        // below it: the two control planes publish the same `bots` first segment, so it is
        // the whole route that decides.
        for (step, method) in [
            ("PutSession", "POST"),
            ("GetSession", "GET"),
            ("DeleteSession", "DELETE"),
        ] {
            assert_sole_claimant(case, step, &family, method, &path, RUNTIME_V2);
        }
        let text_path = format!("{path}/text");
        assert_sole_claimant(case, "RecognizeText", &family, "POST", &text_path, RUNTIME_V2);

        // ---- PutSession seeds the dialog state -------------------------------------
        let attributes = attribute_map(&run.attributes);
        let put = dispatch(
            &router,
            "POST",
            &path,
            &json!({
                "sessionState": {
                    "dialogAction": { "type": "Delegate" },
                    "sessionAttributes": attributes.clone(),
                },
            })
            .to_string(),
        )
        .await;
        assert_served(case, "PutSession", &put);
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutSession must answer the 200 its model declares, got {} {:?}: {}",
            put.status, put.error_code, put.raw_body
        );

        // ---- GetSession reports what it seeded --------------------------------------
        let got = dispatch(&router, "GET", &path, "").await;
        assert_served(case, "GetSession after PutSession", &got);
        assert_eq!(
            got.status, 200,
            "case {case:?}: GetSession must report the session PutSession created, got {} \
             {:?}: {}",
            got.status, got.error_code, got.raw_body
        );
        assert_eq!(
            got.body.get("sessionId").and_then(Value::as_str),
            Some(run.session.as_str()),
            "case {case:?}: GetSession must answer for the session that was addressed, got {}",
            got.raw_body
        );
        assert_eq!(
            got.body.pointer("/sessionState/sessionAttributes").cloned(),
            Some(attributes.clone()),
            "case {case:?}: GetSession must report the attributes PutSession seeded, got {}",
            got.raw_body
        );
        assert_eq!(
            got.body
                .get("interpretations")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0),
            "case {case:?}: no utterance has been recognized yet, got {}",
            got.raw_body
        );

        // ---- each turn is served by the runtime too ---------------------------------
        for turn in 1..=run.turns {
            let spoken = dispatch(
                &router,
                "POST",
                &text_path,
                &json!({ "text": format!("turn {turn}") }).to_string(),
            )
            .await;
            let step = format!("RecognizeText turn {turn}");
            assert_served(case, &step, &spoken);
            assert_eq!(
                spoken.status, 200,
                "case {case:?}: {step} must answer the 200 its model declares, got {} {:?}: {}",
                spoken.status, spoken.error_code, spoken.raw_body
            );
            assert_eq!(
                spoken.body.get("sessionId").and_then(Value::as_str),
                Some(run.session.as_str()),
                "case {case:?}: {step} must answer for the session it addressed, got {}",
                spoken.raw_body
            );
            // `interpretations` is an output member only the runtime model declares, so its
            // presence is what says the data plane — not a control plane sharing the `bots`
            // space — served the turn.
            assert!(
                spoken
                    .body
                    .get("interpretations")
                    .is_some_and(Value::is_array),
                "case {case:?}: {step} must report the turn's interpretations, got {}",
                spoken.raw_body
            );
        }

        // ---- the state PutSession seeded survives every turn ------------------------
        let after = dispatch(&router, "GET", &path, "").await;
        assert_served(case, "GetSession after the turns", &after);
        assert_eq!(
            after.status, 200,
            "case {case:?}: GetSession must still report the session after {} turn(s), got \
             {} {:?}: {}",
            run.turns, after.status, after.error_code, after.raw_body
        );
        assert_eq!(
            after.body.pointer("/sessionState/sessionAttributes").cloned(),
            Some(attributes),
            "case {case:?}: the attributes PutSession seeded must survive {} turn(s), got {}",
            run.turns,
            after.raw_body
        );
        let interpreted = after
            .body
            .get("interpretations")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_else(|| panic!("case {case:?}: no interpretations in {}", after.raw_body));
        assert_eq!(
            interpreted > 0,
            run.turns > 0,
            "case {case:?}: after {} turn(s) the session must report {} interpretation(s), \
             got {interpreted}",
            run.turns,
            if run.turns > 0 { "some" } else { "no" }
        );

        // ---- DeleteSession removes it, and the key is then a modelled 404 -----------
        let deleted = dispatch(&router, "DELETE", &path, "").await;
        assert_served(case, "DeleteSession", &deleted);
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteSession must answer the 200 its model declares, got {} \
             {:?}: {}",
            deleted.status, deleted.error_code, deleted.raw_body
        );
        assert_eq!(
            deleted.body.get("botId").and_then(Value::as_str),
            Some(bot_id.as_str()),
            "case {case:?}: DeleteSession must answer for the bot the control plane minted, \
             got {}",
            deleted.raw_body
        );

        let gone = dispatch(&router, "GET", &path, "").await;
        assert_served(case, "GetSession after DeleteSession", &gone);
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a deleted session must be the modelled 404, got {} {:?}: {}",
            gone.status, gone.error_code, gone.raw_body
        );
        assert_eq!(
            gone.error_code, "ResourceNotFoundException",
            "case {case:?}: a deleted session must be refused with the error its model \
             declares, got {} {:?}: {}",
            gone.status, gone.error_code, gone.raw_body
        );
    }
}
