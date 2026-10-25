// SPDX-License-Identifier: CC0-1.0

//! How a request's `filterValues` entry is read: the quotes that enclose a value
//! DELIMIT it, so what a filter expression compares against a stored attribute is the
//! text between them.
//!
//! `filterValues` is documented as "the parameter name (in matching case) as a key and
//! the filter value(s) as the corresponding value. Separate multiple values for one
//! parameter with a comma", and the request format encloses a string value in double
//! quotes (written escaped in JSON) while leaving a numeric value bare. The rule
//! generalizes over every operation that binds a filter and over every form a value
//! can take, so the suites below drive it as a TABLE whose rows differ in how many
//! values an entry carries, whether they are quoted, how they are spaced, whether a
//! value carries the separator itself, and whether the clause compares strings or
//! numbers — and over all three routes plus a promotion's own `filterValues`.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Action, Campaign, Item, RecipeType, REASON_PROMOTED_ITEM,
};

use crate::support::{
    entries, ids, Emulator, ACTION_RECOMMENDATIONS, COLUMN_GENRE, COLUMN_YEAR,
    PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// The `PERSONALIZED_RANKING` campaign the ranking rows re-rank against.
const CAMPAIGN_RANKING: &str =
    "arn:aws:personalize:us-east-1:123456789012:campaign/quoting-ranking";
/// The `USER_PERSONALIZATION` campaign the `GetRecommendations` rows read.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:123456789012:campaign/quoting-user";
/// The `PERSONALIZED_ACTIONS` campaign the `GetActionRecommendations` rows read.
const CAMPAIGN_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:123456789012:campaign/quoting-actions";

/// `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const FILTER_INCLUDE_GENRE: &str =
    "arn:aws:personalize:us-east-1:123456789012:filter/quoting-include-genre";
/// `EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const FILTER_EXCLUDE_GENRE: &str =
    "arn:aws:personalize:us-east-1:123456789012:filter/quoting-exclude-genre";
/// `INCLUDE ItemID WHERE Items.YEAR >= $YEAR`.
const FILTER_FROM_YEAR: &str =
    "arn:aws:personalize:us-east-1:123456789012:filter/quoting-from-year";
/// `INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)`.
const FILTER_INCLUDE_CHANNEL: &str =
    "arn:aws:personalize:us-east-1:123456789012:filter/quoting-include-channel";
/// `EXCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)`.
const FILTER_EXCLUDE_CHANNEL: &str =
    "arn:aws:personalize:us-east-1:123456789012:filter/quoting-exclude-channel";

/// The `GENRE` values the seeded Items catalog cycles through.
///
/// Test data, chosen so the rows below can bind every form a value takes: the third
/// entry carries the value SEPARATOR inside the value, and the fourth is a
/// multi-valued categorical attribute (`|` is the separator a Personalize Items CSV
/// uses within one column, which the filter module documents).
const GENRES: [&str; 5] = [
    "Comedy",
    "Drama",
    "Science Fiction, Fantasy",
    "Comedy|Documentary",
    "Action",
];

/// The `CHANNEL` values the seeded Actions catalog cycles through.
const CHANNELS: [&str; 2] = ["App", "Email"];

/// The first `YEAR` the seeded catalog carries; the column rises by one per entry, so
/// a `>= $YEAR` clause selects a known suffix of the catalog.
const FIRST_YEAR: i64 = 1990;

/// How many entries the seeded Items catalog holds.
const CATALOG_SIZE: usize = 15;
/// How many entries the seeded Actions catalog holds.
const ACTION_CATALOG_SIZE: usize = 8;

/// The Items catalog these suites filter.
fn catalog() -> Vec<Item> {
    (0..CATALOG_SIZE)
        .map(|index| {
            Item::new(format!("item-{index}"), (CATALOG_SIZE - index) as f64)
                .with_attribute(COLUMN_GENRE, GENRES[index % GENRES.len()])
                .with_attribute(COLUMN_YEAR, (FIRST_YEAR + index as i64).to_string())
        })
        .collect()
}

/// The Actions catalog these suites filter.
fn action_catalog() -> Vec<Action> {
    (0..ACTION_CATALOG_SIZE)
        .map(|index| {
            Action::new(format!("action-{index}"), (ACTION_CATALOG_SIZE - index) as f64)
                .with_attribute("CHANNEL", CHANNELS[index % CHANNELS.len()])
        })
        .collect()
}

/// Whether a stored entry's `GENRE` column holds `wanted` — the attribute may carry
/// several values separated by `|`, and `IN` matches when any of them does.
fn holds_genre(item: &Item, wanted: &str) -> bool {
    item.attributes
        .get(COLUMN_GENRE)
        .is_some_and(|stored| stored.split('|').any(|held| held == wanted))
}

/// The stored `YEAR` of one entry, as a number.
fn year(item: &Item) -> i64 {
    item.attributes
        .get(COLUMN_YEAR)
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

/// Whether a stored action's `CHANNEL` column holds `wanted`.
fn on_channel(action: &Action, wanted: &str) -> bool {
    action
        .attributes
        .get("CHANNEL")
        .is_some_and(|stored| stored == wanted)
}

/// An emulator holding the campaigns and the placeholder filters these suites bind.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    let campaigns = [
        (CAMPAIGN_RANKING, RecipeType::PersonalizedRanking),
        (CAMPAIGN_USER, RecipeType::UserPersonalization),
    ];
    for (arn, recipe_type) in campaigns {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new(arn, "quoting", recipe_type, catalog()),
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(CAMPAIGN_ACTIONS, "quoting-actions", action_catalog()),
        ),
        "the campaign seam must register the actions campaign"
    );

    let filters = [
        (
            FILTER_INCLUDE_GENRE,
            "INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)",
        ),
        (
            FILTER_EXCLUDE_GENRE,
            "EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)",
        ),
        (FILTER_FROM_YEAR, "INCLUDE ItemID WHERE Items.YEAR >= $YEAR"),
        (
            FILTER_INCLUDE_CHANNEL,
            "INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)",
        ),
        (
            FILTER_EXCLUDE_CHANNEL,
            "EXCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)",
        ),
    ];
    for (arn, expression) in filters {
        assert_eq!(
            registry::register_filter(engine, arn, expression),
            Ok(true),
            "case {arn}: the filter seam must accept {expression:?}"
        );
    }
    emulator
}

/// The predicate a row's filter, bound to that row's `filterValues`, means over a
/// stored catalog entry.
type Retains = fn(&Item) -> bool;

/// One row: its name, the filter it names, the `filterValues` it sends, and what that
/// combination selects.
type Row = (&'static str, &'static str, Value, Retains);

/// Every form of `filterValues` entry the rule covers, with the subset each selects.
///
/// The JSON string the rows send is what a client writes after JSON escaping is
/// undone: `"\"Comedy\""` on the wire arrives as `"Comedy"`, quote characters
/// included.
fn rows() -> Vec<Row> {
    vec![
        (
            "one quoted value",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Comedy\"" }),
            |item| holds_genre(item, "Comedy"),
        ),
        (
            "two quoted values separated by a comma",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Comedy\",\"Drama\"" }),
            |item| holds_genre(item, "Comedy") || holds_genre(item, "Drama"),
        ),
        (
            "quoted values spaced around the separator",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": " \"Drama\" , \"Action\" " }),
            |item| holds_genre(item, "Drama") || holds_genre(item, "Action"),
        ),
        (
            "a quoted value carrying the separator itself",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Science Fiction, Fantasy\"" }),
            |item| holds_genre(item, "Science Fiction, Fantasy"),
        ),
        (
            "a quoted value naming one of several values the attribute holds",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Documentary\"" }),
            |item| holds_genre(item, "Documentary"),
        ),
        (
            "a quoted value no stored entry carries",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Opera\"" }),
            |_| false,
        ),
        (
            "an undelimited value",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "Comedy" }),
            |item| holds_genre(item, "Comedy"),
        ),
        (
            "a quoted value whose JSON escaping was left in",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\\\"Comedy\\\"" }),
            |item| holds_genre(item, "Comedy"),
        ),
        (
            "EXCLUDE with one quoted value",
            FILTER_EXCLUDE_GENRE,
            json!({ "GENRE": "\"Comedy\"" }),
            |item| !holds_genre(item, "Comedy"),
        ),
        (
            "EXCLUDE with three quoted values",
            FILTER_EXCLUDE_GENRE,
            json!({ "GENRE": "\"Comedy\",\"Drama\",\"Action\"" }),
            |item| {
                !holds_genre(item, "Comedy")
                    && !holds_genre(item, "Drama")
                    && !holds_genre(item, "Action")
            },
        ),
        (
            "a bare number in a comparison",
            FILTER_FROM_YEAR,
            json!({ "YEAR": "1995" }),
            |item| year(item) >= 1995,
        ),
        (
            "a quoted number in a comparison",
            FILTER_FROM_YEAR,
            json!({ "YEAR": "\"1995\"" }),
            |item| year(item) >= 1995,
        ),
    ]
}

#[tokio::test]
async fn a_ranking_keeps_exactly_the_entries_its_bound_filter_values_name() {
    let emulator = seeded();
    let stored = catalog();
    // Every id the catalog holds, so the ranking shows exactly what the filter removed.
    let input_list: Vec<String> = stored.iter().map(|item| item.item_id.clone()).collect();

    for (case, filter_arn, filter_values, retains) in rows() {
        let expected: Vec<String> = stored
            .iter()
            .filter(|item| retains(item))
            .map(|item| item.item_id.clone())
            .collect();
        assert!(
            expected.len() < stored.len(),
            "case {case}: the row must select a strict subset of the catalog, got \
             {expected:?}"
        );

        let response = emulator
            .ok(
                PERSONALIZE_RANKING,
                json!({
                    "campaignArn": CAMPAIGN_RANKING,
                    "userId": "user-1",
                    "inputList": input_list,
                    "filterArn": filter_arn,
                    "filterValues": filter_values,
                }),
            )
            .await;

        let returned = ids(&response, "personalizedRanking", "itemId");
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_returned, sorted_expected,
            "case {case}: the ranking must hold exactly the entries the bound filter \
             values name, got {returned:?}"
        );
    }
}

#[tokio::test]
async fn recommendations_keep_exactly_the_entries_the_same_values_name() {
    let emulator = seeded();
    let stored = catalog();

    for (case, filter_arn, filter_values, retains) in rows() {
        let expected: Vec<String> = stored
            .iter()
            .filter(|item| retains(item))
            .map(|item| item.item_id.clone())
            .collect();

        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-2",
                    // More than the filter can leave, so the response size shows what
                    // the filter removed.
                    "numResults": CATALOG_SIZE,
                    "filterArn": filter_arn,
                    "filterValues": filter_values,
                }),
            )
            .await;

        let returned = ids(&response, "itemList", "itemId");
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_returned, sorted_expected,
            "case {case}: the recommendations must hold exactly the entries the bound \
             filter values name, got {returned:?}"
        );
    }
}

#[tokio::test]
async fn an_action_filter_binds_the_same_values_the_same_way() {
    let emulator = seeded();
    let stored = action_catalog();

    // The rows differ in the filter kind and in whether the value is delimited.
    let rows: [(&str, &str, Value, &str, bool); 4] = [
        (
            "INCLUDE with a quoted value",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"App\"" }),
            "App",
            true,
        ),
        (
            "INCLUDE with an undelimited value",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "App" }),
            "App",
            true,
        ),
        (
            "EXCLUDE with a quoted value",
            FILTER_EXCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"App\"" }),
            "App",
            false,
        ),
        (
            "EXCLUDE with a quoted value spaced around the separator",
            FILTER_EXCLUDE_CHANNEL,
            json!({ "CHANNEL": " \"Email\" " }),
            "Email",
            false,
        ),
    ];

    for (case, filter_arn, filter_values, channel, keeps_matches) in rows {
        let expected: Vec<String> = stored
            .iter()
            .filter(|action| on_channel(action, channel) == keeps_matches)
            .map(|action| action.action_id.clone())
            .collect();
        assert!(
            !expected.is_empty() && expected.len() < stored.len(),
            "case {case}: the row must select a strict, non-empty subset, got {expected:?}"
        );

        let response = emulator
            .ok(
                ACTION_RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_ACTIONS,
                    "userId": "user-3",
                    "numResults": ACTION_CATALOG_SIZE,
                    "filterArn": filter_arn,
                    "filterValues": filter_values,
                }),
            )
            .await;

        let returned = ids(&response, "actionList", "actionId");
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_returned, sorted_expected,
            "case {case}: the action list must hold exactly the actions the bound filter \
             values name, got {returned:?}"
        );
    }
}

#[tokio::test]
async fn a_promotions_own_filter_values_bind_the_same_way() {
    let emulator = seeded();
    let stored = catalog();

    // A promotion carries its own `filterValues`, documented in the same words, so the
    // promoted slots are the entries those values name. The rows differ in how many
    // values the entry carries and in whether they are delimited.
    let rows: [(&str, Value, Vec<&str>); 3] = [
        (
            "one quoted value",
            json!({ "GENRE": "\"Comedy\"" }),
            vec!["Comedy"],
        ),
        (
            "two quoted values",
            json!({ "GENRE": "\"Drama\",\"Action\"" }),
            vec!["Drama", "Action"],
        ),
        (
            "an undelimited value",
            json!({ "GENRE": "Comedy" }),
            vec!["Comedy"],
        ),
    ];

    for (case, filter_values, wanted) in rows {
        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-4",
                    "numResults": 10,
                    "promotions": [{
                        "name": "promo-quoting",
                        "percentPromotedItems": 50,
                        "filterArn": FILTER_INCLUDE_GENRE,
                        "filterValues": filter_values,
                    }],
                }),
            )
            .await;

        let returned = entries(&response, "itemList");
        let promoted: Vec<&Value> = returned
            .iter()
            .filter(|entry| entry.get("promotionName").is_some())
            .collect();
        assert_eq!(
            promoted.len(),
            5,
            "case {case}: half of ten slots must carry the promotion, got {promoted:?}"
        );
        for entry in &promoted {
            assert_eq!(
                entry.get("reason").and_then(Value::as_array),
                Some(&vec![json!(REASON_PROMOTED_ITEM)]),
                "case {case}: a promoted item must carry the documented reason, got {entry}"
            );
            let item_id = entry
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let item = stored
                .iter()
                .find(|item| item.item_id == item_id)
                .unwrap_or_else(|| panic!("case {case}: {item_id} is not in the stored catalog"));
            assert!(
                wanted.iter().any(|value| holds_genre(item, value)),
                "case {case}: a promoted item must satisfy the promotion's bound filter \
                 values {wanted:?}, got {item_id} with GENRE {:?}",
                item.attributes.get(COLUMN_GENRE)
            );
        }
    }
}
