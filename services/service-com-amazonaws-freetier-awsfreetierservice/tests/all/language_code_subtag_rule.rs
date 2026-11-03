// SPDX-License-Identifier: CC0-1.0

//! The crate's one statement of when two `LanguageCode` values name the SAME
//! language (`crate::state::primary_subtag`), driven over the wire through an
//! operation from each of its call sites.
//!
//! `LocalizedText::in_language` resolves a requested code against the stored
//! translations with that computation, and both activity readers reach it:
//! `GetAccountActivity` for the `title` and `description` it reports,
//! `ListAccountActivities` for the `title`. So the grouping asserted here is
//! READ FROM the shared computation rather than restated: over the model's
//! whole `LanguageCode` value set and the whole catalog, two codes the shared
//! rule puts in different languages are answered with different text by BOTH
//! readers, and both readers answer one code with one text because they resolve
//! one stored row by one rule.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::ops::types::LANGUAGE_CODE_VALUES;
use service_com_amazonaws_freetier_awsfreetierservice::state::primary_subtag;
use smithy_server_runtime::MultiServiceRouter;

use crate::free_tier::{harness, ok, text, ACTIVITIES_PAGE_SIZE_CEILING};

/// The largest page `ListAccountActivities` serves
/// ([`ACTIVITIES_PAGE_SIZE_CEILING`]), so this asks for the whole catalog in one
/// call.
const MAX_RESULTS_CEILING: i32 = ACTIVITIES_PAGE_SIZE_CEILING;

/// One of this service's activity readers — the two call sites of the shared
/// rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reader {
    /// `GetAccountActivity`, which reports the detail view.
    Detail,
    /// `ListAccountActivities`, which reports one summary per activity.
    List,
}

impl Reader {
    /// Both readers, so every rule below is asserted at each call site.
    const ALL: [Self; 2] = [Self::Detail, Self::List];

    /// The members the model documents this reader returns translated
    /// (`GetAccountActivityOutput$title`/`$description`,
    /// `ActivitySummary$title`).
    fn translated_members(self) -> &'static [&'static str] {
        match self {
            Self::Detail => &["title", "description"],
            Self::List => &["title"],
        }
    }
}

/// Every activity the catalog holds in one language, keyed by activity id.
async fn summaries(router: &MultiServiceRouter, language_code: &str) -> BTreeMap<String, Value> {
    let payload = ok(
        router,
        "ListAccountActivities",
        &json!({"maxResults": MAX_RESULTS_CEILING, "languageCode": language_code}),
    )
    .await;
    payload["activities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {language_code}: activities must be a list, got {payload}"))
        .iter()
        .map(|activity| (text(activity, "activityId"), activity.clone()))
        .collect()
}

/// The detail view of one activity in one language.
async fn detail(router: &MultiServiceRouter, activity_id: &str, language_code: &str) -> Value {
    ok(
        router,
        "GetAccountActivity",
        &json!({"activityId": activity_id, "languageCode": language_code}),
    )
    .await
}

/// The modelled codes paired once each (`code` before `other` in the value
/// set), so a rule about a pair is asserted exactly once.
fn code_pairs() -> Vec<(&'static str, &'static str)> {
    let mut pairs = Vec::new();
    for (index, code) in LANGUAGE_CODE_VALUES.iter().enumerate() {
        for other in &LANGUAGE_CODE_VALUES[index + 1..] {
            pairs.push((*code, *other));
        }
    }
    pairs
}

/// Both activity readers resolve a `languageCode` by the crate's one subtag
/// rule: two codes the rule puts in different languages are answered with
/// different translated text, and two codes answered with the same text
/// therefore name one language.
///
/// The table spans the model's whole `LanguageCode` value set
/// (`ops::types::LANGUAGE_CODE_VALUES`, generated from the Smithy enum),
/// every activity in the catalog, and both readers, so the grouping is asserted
/// against the shared computation at each of its call sites rather than against
/// a copy of it.
#[tokio::test]
async fn both_activity_readers_group_language_codes_by_one_subtag_rule() {
    let router = harness().0;

    // reader -> code -> activity id -> the translated members that reader
    // reports, gathered over the wire once per (code, activity).
    let mut reported: BTreeMap<Reader, BTreeMap<&str, BTreeMap<String, Vec<String>>>> =
        BTreeMap::new();
    for code in LANGUAGE_CODE_VALUES {
        for (id, summary) in summaries(&router, code).await {
            let view = detail(&router, &id, code).await;

            // Both readers resolve the SAME stored row, so the member they
            // both report carries one text in the language asked for.
            assert_eq!(
                text(&summary, "title"),
                text(&view, "title"),
                "case {id}/{code}: both readers resolve one localized row, so the summary's \
                 title must be the detail view's title, got {summary} against {view}"
            );

            for reader in Reader::ALL {
                let payload = match reader {
                    Reader::Detail => &view,
                    Reader::List => &summary,
                };
                let values: Vec<String> = reader
                    .translated_members()
                    .iter()
                    .map(|member| text(payload, member))
                    .collect();
                reported
                    .entry(reader)
                    .or_default()
                    .entry(code)
                    .or_default()
                    .insert(id.clone(), values);
            }
        }
    }

    let ids: Vec<String> = reported[&Reader::Detail][LANGUAGE_CODE_VALUES[0]]
        .keys()
        .cloned()
        .collect();
    assert!(
        !ids.is_empty(),
        "case catalog: the account must hold activities to read in each language"
    );

    for (reader, per_code) in &reported {
        for id in &ids {
            for (code, other) in code_pairs() {
                if primary_subtag(code) == primary_subtag(other) {
                    continue;
                }
                let for_code = &per_code[code][id];
                let for_other = &per_code[other][id];
                assert_ne!(
                    for_code, for_other,
                    "case {reader:?} {id}/{code} vs {other}: two codes naming different languages \
                     must not be answered with the same text, got {for_code:?} for both"
                );
            }
        }
    }
}
