// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListSlotTypes`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the emulator
//! serves on its socket, so the restJson1 routing, the modelled constraints and the
//! error envelopes are exercised exactly as a client sees them. The harness and the
//! bot/locale/slot-type setup are the other suites', reused rather than copied.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * the listing is built from the slot types stored in the bot, bot version and locale
//!   the URI names — and from no others — and each entry reports the stored definition,
//!   including the category the definition puts the slot type in;
//! * the listing is ordered by the attribute the request sorts by, in the order it asks
//!   for, and paged: a page holds no more than `maxResults` entries and `nextToken`
//!   resumes the same total order without losing or repeating an entry;
//! * a filter keeps the entries whose filtered field satisfies it, for every modelled
//!   filter name and operator;
//! * a sort attribute, sort order, filter name, filter operator, page size or token the
//!   model or this operation does not accept is refused.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::bot_lifecycle::{call, emulator, error_code, member, text};
use crate::slot_type::{create_slot_type, host_locale, slot_type_body, slot_types_uri};

/// The grammar source a slot type whose values live outside the bot points at.
fn grammar_setting() -> Value {
    json!({
        "grammarSlotTypeSetting": {
            "source": {
                "s3BucketName": "lex-grammar-bucket",
                "s3ObjectKey": "grammars/city.grxml",
            },
        },
    })
}

/// `ListSlotTypes` is a POST on `CreateSlotType`'s URI; `body` carries the sort, filter
/// and paging members.
async fn list(
    router: &MultiServiceRouter,
    bot_id: &str,
    locale_id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &slot_types_uri(bot_id, "DRAFT", locale_id),
        Some(body),
    )
    .await
}

/// The `slotTypeName`s of a listing, in the order the response put them.
fn listed_names(response: &Value) -> Vec<String> {
    member(response, "slotTypeSummaries")
        .as_array()
        .map(|entries| entries.iter().map(|entry| text(entry, "slotTypeName")).collect())
        .unwrap_or_default()
}

/// One summary of a listing, by slot type name.
fn summary_of(response: &Value, name: &str) -> Value {
    member(response, "slotTypeSummaries")
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| text(entry, "slotTypeName") == name)
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// The slot types every listing assertion is run over: rows differ in what the
/// definition carries, which is what decides the category the summary reports.
fn listed_slot_types(sub_slot_type_id: &str) -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "a slot type enumerating its own values",
            slot_type_body(
                "City",
                json!({
                    "description": "where the pizza goes",
                    "slotTypeValues": [{ "sampleValue": { "value": "Berlin" } }],
                }),
            ),
            state::SLOT_TYPE_CATEGORY_CUSTOM,
        ),
        (
            "a slot type extending the built-in parent signature",
            slot_type_body(
                "Reservation_Code",
                json!({ "parentSlotTypeSignature": state::SUPPORTED_PARENT_SLOT_TYPE_SIGNATURE }),
            ),
            state::SLOT_TYPE_CATEGORY_EXTENDED,
        ),
        (
            "a slot type whose values come from a grammar outside the bot",
            slot_type_body(
                "GrammarCity",
                json!({ "externalSourceSetting": grammar_setting() }),
            ),
            state::SLOT_TYPE_CATEGORY_EXTERNAL_GRAMMAR,
        ),
        (
            "a slot type composed of sub slot types",
            slot_type_body(
                "Composite_Address",
                json!({
                    "compositeSlotTypeSetting": {
                        "subSlots": [{ "name": "town", "slotTypeId": sub_slot_type_id }],
                    },
                }),
            ),
            state::SLOT_TYPE_CATEGORY_COMPOSITE,
        ),
    ]
}

/// A bot with one locale holding [`listed_slot_types`], and the names of those slot
/// types in the order they were created.
async fn stocked_locale(
    router: &MultiServiceRouter,
    bot_name: &str,
    locale_id: &str,
) -> (String, Vec<String>) {
    let bot_id = host_locale(router, bot_name, locale_id).await;
    let sub = create_slot_type(
        router,
        &bot_id,
        locale_id,
        &slot_type_body("Town", json!({})),
    )
    .await;

    let mut created = vec!["Town".to_string()];
    for (case, body, expected_category) in listed_slot_types(&sub) {
        create_slot_type(router, &bot_id, locale_id, &body).await;
        let (status, response) = list(router, &bot_id, locale_id, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListSlotTypes to answer 200, got {status} {response:?}"
        );
        let name = text(&body, "slotTypeName");
        let summary = summary_of(&response, &name);
        assert_eq!(
            text(&summary, "slotTypeCategory"),
            expected_category,
            "case {case:?}: expected category {expected_category}, got {summary:?}"
        );
        assert_eq!(
            member(&summary, "description"),
            member(&body, "description"),
            "case {case:?}: the summary must report the stored description, got {summary:?}"
        );
        assert_eq!(
            member(&summary, "parentSlotTypeSignature"),
            member(&body, "parentSlotTypeSignature"),
            "case {case:?}: the summary must report the stored parent signature, got {summary:?}"
        );
        assert!(
            member(&summary, "lastUpdatedDateTime").as_f64().is_some(),
            "case {case:?}: the summary must report when the slot type was last updated, got {summary:?}"
        );
        created.push(name);
    }
    (bot_id, created)
}

#[tokio::test]
async fn a_listing_reports_the_slot_types_stored_in_the_locale_the_uri_names() {
    let router = emulator();
    let (bot_id, created) = stocked_locale(&router, "ListingBot", "en_US").await;

    let (status, response) = list(&router, &bot_id, "en_US", json!({})).await;
    assert_eq!(
        status, 200,
        "expected ListSlotTypes to answer 200, got {status} {response:?}"
    );
    assert_eq!(
        text(&response, "botId"),
        bot_id,
        "the listing must echo the bot it was scoped to, got {response:?}"
    );
    assert_eq!(
        text(&response, "botVersion"),
        "DRAFT",
        "the listing must echo the bot version it was scoped to, got {response:?}"
    );
    assert_eq!(
        text(&response, "localeId"),
        "en_US",
        "the listing must echo the locale it was scoped to, got {response:?}"
    );
    assert_eq!(
        listed_names(&response).into_iter().collect::<BTreeSet<_>>(),
        created.iter().cloned().collect::<BTreeSet<_>>(),
        "the listing must report every stored slot type of the locale, got {response:?}"
    );
    assert_eq!(
        member(&response, "nextToken"),
        Value::Null,
        "a listing that fits one page carries no token, got {response:?}"
    );

    // Each entry's id reads back through DescribeSlotType, so a listed id is a stored
    // slot type and not an invention of the listing.
    let summaries = member(&response, "slotTypeSummaries");
    for entry in summaries.as_array().expect("the listing is an array") {
        let slot_type_id = text(entry, "slotTypeId");
        let (status, described) = call(
            &router,
            "GET",
            &format!("{}/{slot_type_id}", slot_types_uri(&bot_id, "DRAFT", "en_US")),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "a listed slotTypeId must read back, got {status} {described:?}"
        );
        assert_eq!(
            text(&described, "slotTypeName"),
            text(entry, "slotTypeName"),
            "the listed name must be the stored one, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_holds_no_slot_type_of_another_bot_version_or_locale() {
    let router = emulator();
    let (bot_id, created) = stocked_locale(&router, "ScopedListingBot", "en_US").await;
    let (other_bot, _) = stocked_locale(&router, "OtherListingBot", "en_US").await;

    // A second locale on the same bot, holding a slot type of its own.
    let (status, response) = call(
        &router,
        "PUT",
        &crate::bot_locale::locales_uri(&bot_id, "DRAFT"),
        Some(crate::bot_locale::locale_body("de_DE", 0.4, json!({}))),
    )
    .await;
    assert_eq!(
        status, 202,
        "expected CreateBotLocale to answer 202, got {status} {response:?}"
    );
    create_slot_type(
        &router,
        &bot_id,
        "de_DE",
        &slot_type_body("Stadt", json!({})),
    )
    .await;

    let (_, response) = list(&router, &bot_id, "en_US", json!({})).await;
    let names = listed_names(&response);
    assert!(
        !names.contains(&"Stadt".to_string()),
        "a slot type of another locale must not be listed, got {names:?}"
    );
    assert_eq!(
        names.len(),
        created.len(),
        "the locale holds {} slot types, got {names:?}",
        created.len()
    );

    let (_, response) = list(&router, &bot_id, "de_DE", json!({})).await;
    assert_eq!(
        listed_names(&response),
        vec!["Stadt".to_string()],
        "the second locale holds only its own slot type, got {response:?}"
    );

    // A bot version nothing was created under lists nothing, and so does the other
    // bot's locale — with its own records only.
    let (status, response) = list(&router, &bot_id, "de_DE", json!({ "maxResults": 1 })).await;
    assert_eq!(status, 200, "expected 200, got {status} {response:?}");
    let (_, response) = list(&router, &other_bot, "en_US", json!({})).await;
    assert!(
        !listed_names(&response).contains(&"Stadt".to_string()),
        "the other bot lists only its own slot types, got {response:?}"
    );
}

#[tokio::test]
async fn a_listing_is_ordered_by_the_attribute_the_request_sorts_by() {
    let router = emulator();
    let (bot_id, created) = stocked_locale(&router, "SortedListingBot", "en_US").await;

    // The creation order is also the lastUpdatedDateTime order, since each create
    // stamped the record when it ran.
    let by_creation = created.clone();
    let mut by_name = created.clone();
    by_name.sort();

    // (case, sortBy, the names the listing must report, in order)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "by name, ascending",
            json!({ "attribute": "SlotTypeName", "order": "Ascending" }),
            by_name.clone(),
        ),
        (
            "by name, descending",
            json!({ "attribute": "SlotTypeName", "order": "Descending" }),
            by_name.iter().rev().cloned().collect(),
        ),
        (
            "by last update, ascending",
            json!({ "attribute": "LastUpdatedDateTime", "order": "Ascending" }),
            by_creation.clone(),
        ),
        (
            "by last update, descending",
            json!({ "attribute": "LastUpdatedDateTime", "order": "Descending" }),
            by_creation.iter().rev().cloned().collect(),
        ),
    ];

    for (case, sort_by, expected) in &rows {
        let (status, response) = list(
            &router,
            &bot_id,
            "en_US",
            json!({ "sortBy": sort_by.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        assert_eq!(
            &listed_names(&response),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed_names(&response)
        );
    }
}

#[tokio::test]
async fn a_listing_is_paged_by_the_page_size_the_request_asks_for() {
    let router = emulator();
    let (bot_id, created) = stocked_locale(&router, "PagedListingBot", "en_US").await;
    let mut by_name = created.clone();
    by_name.sort();

    // Rows differ in the page size, including one page per entry and a size larger than
    // the listing.
    for page_size in [1_usize, 2, 3, created.len(), created.len() + 5] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({
                "maxResults": page_size,
                "sortBy": { "attribute": "SlotTypeName", "order": "Ascending" },
            });
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let (status, response) = list(&router, &bot_id, "en_US", body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {response:?}"
            );
            let names = listed_names(&response);
            assert!(
                names.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} entries, got {names:?}"
            );
            walked.extend(names);
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
            match member(&response, "nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, by_name,
            "case {case:?}: paging must walk the listing once, in order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_keeps_the_slot_types_whose_field_satisfies_it() {
    let router = emulator();
    let (bot_id, _) = stocked_locale(&router, "FilteredListingBot", "en_US").await;

    // (case, filter, the names the listing must report)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a name equal to one stored name",
            json!({ "name": "SlotTypeName", "values": ["City"], "operator": "EQ" }),
            vec!["City"],
        ),
        (
            "a name no slot type carries",
            json!({ "name": "SlotTypeName", "values": ["Nothing"], "operator": "EQ" }),
            vec![],
        ),
        (
            "a fragment several names contain",
            json!({ "name": "SlotTypeName", "values": ["City"], "operator": "CO" }),
            vec!["City", "GrammarCity"],
        ),
        (
            "the external source of a slot type whose values live outside the bot",
            json!({
                "name": "ExternalSourceType",
                "values": [state::EXTERNAL_SOURCE_TYPE_GRAMMAR],
                "operator": "EQ",
            }),
            vec!["GrammarCity"],
        ),
        (
            "a fragment of that external source",
            json!({ "name": "ExternalSourceType", "values": ["Grammar"], "operator": "CO" }),
            vec!["GrammarCity"],
        ),
    ];

    for (case, filter, expected) in &rows {
        let (status, response) = list(
            &router,
            &bot_id,
            "en_US",
            json!({
                "filters": [filter.clone()],
                "sortBy": { "attribute": "SlotTypeName", "order": "Ascending" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response:?}"
        );
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            listed_names(&response),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed_names(&response)
        );
    }
}

#[tokio::test]
async fn a_listing_request_outside_what_the_model_accepts_is_refused() {
    let router = emulator();
    let (bot_id, _) = stocked_locale(&router, "RefusedListingBot", "en_US").await;

    // Rows differ in which member is outside what the model or this operation accepts:
    // the sort attribute, the sort order, the filter name, the filter operator
    // (`NE` is a BOT filter operator, not a slot type one), the page size at each side
    // of the published range, and a token this operation never issued.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a sort attribute outside the enum",
            json!({ "sortBy": { "attribute": "SlotTypeId", "order": "Ascending" } }),
        ),
        (
            "a sort order outside the enum",
            json!({ "sortBy": { "attribute": "SlotTypeName", "order": "Sideways" } }),
        ),
        (
            "a filter name outside the enum",
            json!({
                "filters": [{ "name": "BotName", "values": ["City"], "operator": "EQ" }],
            }),
        ),
        (
            "a filter operator the slot type filter does not model",
            json!({
                "filters": [{ "name": "SlotTypeName", "values": ["City"], "operator": "NE" }],
            }),
        ),
        (
            "a page size below the published minimum",
            json!({ "maxResults": 0 }),
        ),
        (
            "a page size above the published maximum",
            json!({ "maxResults": 1_001 }),
        ),
        (
            "a token this operation never issued",
            json!({ "nextToken": "not-a-token" }),
        ),
    ];

    for (case, body) in &rows {
        let (status, response) = list(&router, &bot_id, "en_US", body.clone()).await;
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
}
