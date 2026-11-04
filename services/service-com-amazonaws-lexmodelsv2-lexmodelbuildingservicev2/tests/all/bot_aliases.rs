// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListBotAliases`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so the restJson1 routing, the modelled constraints and the
//! error envelopes are exercised exactly as a client sees them. The harness and the bot
//! setup are the other suites', reused rather than copied.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a listing is built from the aliases the bot the URI names holds, and every bot
//!   holds the alias Amazon Lex keeps for testing it from the moment it is created, so
//!   the listing of a bot that was only created already reports that alias — read back
//!   off the stored record, the same values on every call;
//! * the listing is scoped to that bot: an alias of another bot is not in it, a bot id
//!   nothing backs lists nothing, and an alias does not outlive its bot;
//! * the listing is ordered and paged: a page holds no more than `maxResults` entries
//!   and `nextToken` resumes the same total order without losing or repeating an entry;
//! * a page size or token the model or this operation does not accept is refused.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, create_bot, create_body, emulator, error_code, member, seconds, text};

/// `ListBotAliases`'s URI: `POST /bots/{botId}/botaliases`.
fn aliases_uri(bot_id: &str) -> String {
    format!("/bots/{bot_id}/botaliases")
}

/// List one bot's aliases; `body` carries the paging members.
async fn list(router: &MultiServiceRouter, bot_id: &str, body: Value) -> (u16, Value) {
    call(router, "POST", &aliases_uri(bot_id), Some(body)).await
}

/// The summaries of a listing, in the order the response put them.
fn summaries(response: &Value) -> Vec<Value> {
    member(response, "botAliasSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `botAliasId`s of a listing, in the order the response put them.
fn listed_ids(response: &Value) -> Vec<String> {
    summaries(response)
        .iter()
        .map(|entry| text(entry, "botAliasId"))
        .collect()
}

/// The bots every listing assertion is run over: rows differ in which optional members
/// the bot was created with, in its type, in the session TTL and in the shape of the
/// name — none of which changes the alias the bot is created with.
fn bot_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members",
            create_body("AliasMinimalBot", 300, json!({})),
        ),
        (
            "a bot carrying tags for its test alias",
            create_body(
                "AliasTaggedBot",
                1_800,
                json!({
                    "description": "a bot whose test alias was tagged as it was created",
                    "botTags": { "team": "voice" },
                    "testBotAliasTags": { "env": "test" },
                }),
            ),
        ),
        (
            "a bot network",
            create_body(
                "AliasNetworkBot",
                600,
                json!({
                    "botType": "BotNetwork",
                    "botMembers": [{
                        "botMemberId": "ABCDEFGHIJ",
                        "botMemberName": "MemberOne",
                        "botMemberAliasId": state::TEST_BOT_ALIAS_ID,
                        "botMemberAliasName": state::TEST_BOT_ALIAS_NAME,
                        "botMemberVersion": state::DRAFT_BOT_VERSION,
                    }],
                }),
            ),
        ),
        (
            "a name using the separators the modelled pattern allows",
            create_body("Alias_Bot-Name_2", 120, json!({})),
        ),
    ]
}

/// Assert `summary` is the alias Amazon Lex keeps for testing the bot, naming `case` on
/// every failure.
fn assert_test_alias(case: &str, summary: &Value) {
    assert_eq!(
        text(summary, "botAliasId"),
        state::TEST_BOT_ALIAS_ID,
        "case {case:?}: expected the test alias id, got {summary:?}"
    );
    assert_eq!(
        text(summary, "botAliasName"),
        state::TEST_BOT_ALIAS_NAME,
        "case {case:?}: expected the test alias name, got {summary:?}"
    );
    assert_eq!(
        text(summary, "botVersion"),
        state::DRAFT_BOT_VERSION,
        "case {case:?}: the test alias points at the bot's working copy, got {summary:?}"
    );
    assert_eq!(
        text(summary, "botAliasStatus"),
        state::BOT_ALIAS_STATUS_AVAILABLE,
        "case {case:?}: expected the alias to be ready for use, got {summary:?}"
    );
    assert!(
        seconds(summary, "creationDateTime").is_finite(),
        "case {case:?}: the summary must report when the alias was created, got {summary:?}"
    );
    assert!(
        seconds(summary, "lastUpdatedDateTime").is_finite(),
        "case {case:?}: the summary must report when the alias was last updated, got {summary:?}"
    );
}

#[tokio::test]
async fn a_listing_reports_the_alias_every_created_bot_holds() {
    let router = emulator();

    for (case, request) in bot_cases() {
        let bot_id = create_bot(&router, &request).await;

        let (status, response) = list(&router, &bot_id, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListBotAliases to answer 200, got {status} {response:?}"
        );
        assert_eq!(
            text(&response, "botId"),
            bot_id,
            "case {case:?}: the listing must echo the bot it was scoped to, got {response:?}"
        );
        assert_eq!(
            member(&response, "nextToken"),
            Value::Null,
            "case {case:?}: a listing that fits one page carries no token, got {response:?}"
        );
        let listed = summaries(&response);
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: a bot that was only created holds the test alias alone, got {response:?}"
        );
        assert_test_alias(case, &listed[0]);

        // Reading the listing again reports the same stored record: the summary is read
        // back off state, not minted per call, so nothing a caller sees moves because it
        // looked.
        let (_, again) = list(&router, &bot_id, json!({})).await;
        assert_eq!(
            summaries(&again),
            listed,
            "case {case:?}: a second listing must report the same stored alias, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_holds_no_alias_of_another_bot() {
    let router = emulator();

    let mut bots: Vec<String> = Vec::new();
    for (case, request) in bot_cases() {
        let bot_id = create_bot(&router, &request).await;
        bots.push(bot_id.clone());

        let (status, response) = list(&router, &bot_id, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        for entry in summaries(&response) {
            assert_eq!(
                text(&entry, "botAliasId"),
                state::TEST_BOT_ALIAS_ID,
                "case {case:?}: the listing must hold only this bot's aliases, got {entry:?}"
            );
        }
    }
    assert_eq!(
        bots.iter().cloned().collect::<BTreeSet<_>>().len(),
        bots.len(),
        "each case must have created its own bot, got {bots:?}"
    );

    // A bot id nothing backs holds no alias; this operation models no not-found error for
    // it, so the listing is empty rather than refused.
    let (status, response) = list(&router, "ZZZZZZZZZZ", json!({})).await;
    assert_eq!(
        status, 200,
        "a bot id nothing backs must still answer 200, got {status} {response:?}"
    );
    assert_eq!(
        listed_ids(&response),
        Vec::<String>::new(),
        "a bot id nothing backs lists no alias, got {response:?}"
    );
    assert_eq!(
        text(&response, "botId"),
        "ZZZZZZZZZZ",
        "the listing must echo the bot it was scoped to, got {response:?}"
    );
}

#[tokio::test]
async fn an_alias_does_not_outlive_the_bot_it_belongs_to() {
    let router = emulator();

    // Rows differ in whether the delete asks to skip the in-use check, which is the only
    // member `DeleteBot` takes beyond the bot it names.
    for (case, skip) in [("with the in-use check", false), ("skipping it", true)] {
        let bot_id = create_bot(&router, &create_body(
            &format!("AliasDeleted{}", u8::from(skip)),
            300,
            json!({}),
        ))
        .await;
        let (_, response) = list(&router, &bot_id, json!({})).await;
        assert_eq!(
            listed_ids(&response),
            vec![state::TEST_BOT_ALIAS_ID.to_string()],
            "case {case:?}: the created bot must hold its test alias, got {response:?}"
        );

        let uri = format!("/bots/{bot_id}?skipResourceInUseCheck={skip}");
        let (status, deleted) = call(&router, "DELETE", &uri, None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected DeleteBot to answer 202, got {status} {deleted:?}"
        );

        let (status, response) = list(&router, &bot_id, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        assert_eq!(
            listed_ids(&response),
            Vec::<String>::new(),
            "case {case:?}: an alias must not outlive its bot, got {response:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_is_paged_by_the_page_size_the_request_asks_for() {
    let router = emulator();
    let bot_id = create_bot(&router, &create_body("AliasPagedBot", 300, json!({}))).await;
    let held = vec![state::TEST_BOT_ALIAS_ID.to_string()];

    // Rows differ in the page size: one page per entry, a size larger than the listing,
    // and both published bounds of the modelled range.
    for page_size in [1_usize, 2, 5, state::MAX_LIST_PAGE] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "maxResults": page_size });
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let (status, response) = list(&router, &bot_id, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {response:?}"
            );
            let ids = listed_ids(&response);
            assert!(
                ids.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} entries, got {ids:?}"
            );
            walked.extend(ids);
            pages += 1;
            assert!(
                pages <= held.len() + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            match member(&response, "nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, held,
            "case {case:?}: paging must walk the listing once, in order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_request_outside_what_the_model_accepts_is_refused() {
    let router = emulator();
    let bot_id = create_bot(&router, &create_body("AliasRefusedBot", 300, json!({}))).await;

    // Rows differ in which member is outside what the model or this operation accepts:
    // the page size one past each published bound of the modelled range 1..1000, and a
    // token this operation never issued.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a page size below the published minimum",
            json!({ "maxResults": 0 }),
        ),
        (
            "a page size above the published maximum",
            json!({ "maxResults": state::MAX_LIST_PAGE + 1 }),
        ),
        (
            "a token this operation never issued",
            json!({ "nextToken": "not-a-token" }),
        ),
    ];

    for (case, body) in &rows {
        let (status, response) = list(&router, &bot_id, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the request must be refused with 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }

    // Each published bound of the range is accepted, which is what makes the rows above a
    // refusal of what lies outside it rather than of the bound itself.
    for page_size in [1_usize, state::MAX_LIST_PAGE] {
        let (status, response) = list(&router, &bot_id, json!({ "maxResults": page_size })).await;
        assert_eq!(
            status, 200,
            "maxResults {page_size} is a published bound and must be accepted, got {status} {response:?}"
        );
    }
}
