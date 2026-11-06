// SPDX-License-Identifier: CC0-1.0

//! What a `CreateCallAnalyticsCategory` is refused for when the Region it is served in
//! already holds as many Call Analytics categories as this emulator keeps for one,
//! end to end over the wire.
//!
//! The rules these tests pin, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. The number of categories one Region holds is BOUNDED
//!    (`state::MAX_CALL_ANALYTICS_CATEGORIES`): while a Region is at that ceiling every
//!    further create is refused with the modelled `LimitExceededException`, whatever the
//!    request's name, rules, input type or tags, and the refusal names the ceiling.
//! 2. A refused create STORES NOTHING: the name it asked for is not readable afterwards
//!    and the Region reports exactly the categories it held before.
//! 3. The ceiling counts what the Region HOLDS, not the creates it has served, so a
//!    delete frees exactly one slot: the next create is served and the category it
//!    stored reads back, and the one after it is refused again.
//! 4. The ceiling is the Region's own: a Region at it does not stop another Region
//!    serving its first create, even of a name the full Region already holds.
//!
//! The rows differ along what the rules generalise over: the form of the category name
//! (the shortest the member's `@length(1..200)` / `@pattern(^[0-9a-zA-Z._-]+$)` allows,
//! every allowed punctuation character, a name at the 200-character ceiling), which
//! member of the `Rule` union the rules use, how many rules there are, whether the
//! request names an `InputType`, and whether it carries tags.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`) and the category suite's rule builders
//! (`crate::call_analytics_categories`) rather than standing up a second emulator or
//! spelling a second set of rules.

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::state::{
    DEFAULT_REGION, MAX_CALL_ANALYTICS_CATEGORIES,
};

use crate::call_analytics_categories::{
    interruption_rule, non_talk_time_rule, sentiment_rule, transcript_rule,
};
use crate::call_analytics_rule_negate::as_recorded;
use crate::transcription_jobs::Transcribe;

/// The modelled client error this service answers for a category the Region a request
/// is served in does not hold, and the status its awsJson1_1 wire protocol puts on
/// every client error — including the modelled 429 of `LimitExceededException` (both
/// pinned by `crate::client_error_wire_status`).
const ABSENT_CODE: &str = "BadRequestException";
/// See [`ABSENT_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;
/// The modelled refusal a create past a count ceiling carries.
const QUOTA_CODE: &str = "LimitExceededException";

/// The Region the tables fill to the ceiling: the one an unsigned request is served in
/// too.
const HOME: &str = DEFAULT_REGION;
/// A Region no table creates a filler category in.
const ELSEWHERE: &str = "ap-southeast-3";

/// The largest page `ListCallAnalyticsCategories` may be asked for: `MaxResults` is
/// modelled `@range(min: 1, max: 100)`
/// (`smithy_query(query="operation", operation="ListCallAnalyticsCategories")`).
const MAX_PAGE: usize = 100;

/// The name of the `index`-th category a table fills a Region with. Zero-padded so the
/// store's own name order (which the listing pages in) is the index order.
fn filler_name(index: usize) -> String {
    format!("quota-{index:03}")
}

/// The `index`-th filler create, cycling through all four members of the `Rule` union
/// and through naming and not naming an `InputType`, so no row of a table depends on
/// the full Region having been filled with one shape of category.
fn filler_request(index: usize) -> Value {
    let rules = match index % 4 {
        0 => vec![non_talk_time_rule(1_000 * index as i64)],
        1 => vec![interruption_rule(500)],
        2 => vec![transcript_rule(&format!("phrase {index}"))],
        _ => vec![sentiment_rule("NEGATIVE"), non_talk_time_rule(0)],
    };
    let mut request = json!({"CategoryName": filler_name(index), "Rules": rules});
    if index.is_multiple_of(2) {
        request["InputType"] = json!("REAL_TIME");
    }
    request
}

/// Fill `region` to the ceiling, asserting every create of the way there is served.
async fn fill_to_the_ceiling(transcribe: &Transcribe, region: &str) {
    for index in 0..MAX_CALL_ANALYTICS_CATEGORIES {
        transcribe
            .call_in_region(
                "CreateCallAnalyticsCategory",
                filler_request(index),
                Some(region),
            )
            .await
            .ok(&format!(
                "the category {} of {MAX_CALL_ANALYTICS_CATEGORIES} in {region}",
                index + 1
            ));
    }
}

/// Every category name `region` currently holds, read by paging the listing to its end.
async fn names_held(transcribe: &Transcribe, region: &str, case: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut request = json!({"MaxResults": MAX_PAGE});
        if let Some(token) = &cursor {
            request["NextToken"] = json!(token);
        }
        let reply = transcribe
            .call_in_region("ListCallAnalyticsCategories", request, Some(region))
            .await;
        let body = reply.ok(case).clone();
        let page = body
            .get("Categories")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a Categories list, got {body:?}")
            });
        for category in &page {
            names.push(
                category
                    .get("CategoryName")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a CategoryName, got {category:?}")
                    })
                    .to_string(),
            );
        }
        match body.get("NextToken").and_then(Value::as_str) {
            Some(token) => cursor = Some(token.to_string()),
            None => break,
        }
    }
    names
}

/// One create a table probes a full Region with.
struct Probe {
    case: &'static str,
    name: String,
    /// The rules this probe asks for, which a served probe must report back.
    rules: Vec<Value>,
    /// The `InputType` the probe sends, or `None` to send none at all.
    input_type: Option<&'static str>,
    /// The tags the probe sends, or `None` to send no `Tags` member.
    tags: Option<Vec<(&'static str, &'static str)>>,
}

impl Probe {
    /// This probe's `CreateCallAnalyticsCategory` request body.
    fn request(&self) -> Value {
        let mut request = json!({"CategoryName": self.name, "Rules": self.rules});
        if let Some(input_type) = self.input_type {
            request["InputType"] = json!(input_type);
        }
        if let Some(tags) = &self.tags {
            request["Tags"] = tags
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect();
        }
        request
    }
}

/// The creates a full Region is probed with, one row per axis the rules generalise
/// over: the form of the name, which member of the `Rule` union the rules use, how many
/// rules there are, whether an `InputType` is named, and whether tags are carried.
fn probes() -> Vec<Probe> {
    vec![
        Probe {
            case: "the shortest allowed name, one non-talk-time rule, no InputType",
            name: "p".to_string(),
            rules: vec![non_talk_time_rule(10_000)],
            input_type: None,
            tags: None,
        },
        Probe {
            case: "every allowed punctuation character, an interruption rule, POST_CALL",
            name: "p.a_s-t.9".to_string(),
            rules: vec![interruption_rule(4_000)],
            input_type: Some("POST_CALL"),
            tags: Some(vec![("team", "analytics")]),
        },
        Probe {
            case: "a name at the 200-character ceiling, a transcript rule, REAL_TIME",
            name: "q".repeat(200),
            rules: vec![transcript_rule("please hold")],
            input_type: Some("REAL_TIME"),
            tags: Some(vec![("team", "analytics"), ("env", "test")]),
        },
        Probe {
            case: "two rules of two union members, no InputType, one tag",
            name: "sentiment-and-silence".to_string(),
            rules: vec![sentiment_rule("POSITIVE"), non_talk_time_rule(0)],
            input_type: None,
            tags: Some(vec![("env", "")]),
        },
    ]
}

/// RULES 1 and 2: every create a Region at its ceiling is probed with is refused with
/// the modelled `LimitExceededException`, the refusal names the ceiling, and nothing is
/// stored — the probed name stays unreadable and the Region keeps exactly the categories
/// it held.
#[tokio::test]
async fn a_region_at_its_ceiling_refuses_every_further_create() {
    let transcribe = Transcribe::new();
    fill_to_the_ceiling(&transcribe, HOME).await;

    let before = names_held(&transcribe, HOME, "the full Region").await;
    assert_eq!(
        before.len(),
        MAX_CALL_ANALYTICS_CATEGORIES,
        "case {:?}: expected the Region to hold {MAX_CALL_ANALYTICS_CATEGORIES} \
         categories, got {}",
        "the full Region",
        before.len()
    );

    for probe in probes() {
        let case = probe.case;
        let refused = transcribe
            .call_in_region("CreateCallAnalyticsCategory", probe.request(), Some(HOME))
            .await;
        refused.refused(case, CLIENT_ERROR_STATUS, QUOTA_CODE);
        let message = refused.error_message(case);
        assert!(
            message.contains(&MAX_CALL_ANALYTICS_CATEGORIES.to_string()),
            "case {case:?}: the refusal must name the ceiling it was refused for, got \
             {message:?}"
        );

        // RULE 2: the refused create stored nothing — the name it asked for is not a
        // category this Region holds.
        transcribe
            .call_in_region(
                "GetCallAnalyticsCategory",
                json!({"CategoryName": probe.name}),
                Some(HOME),
            )
            .await
            .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
        let after = names_held(&transcribe, HOME, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must leave the Region's categories exactly \
             as they were, got {} of them",
            after.len()
        );
    }
}

/// RULE 3: the ceiling counts the categories a Region HOLDS, so a delete frees exactly
/// one slot — the next create is served and its category reads back, and the one after
/// it is refused again. One row per probe, each run on its own freed slot.
#[tokio::test]
async fn a_delete_frees_exactly_one_slot() {
    for (index, probe) in probes().into_iter().enumerate() {
        let case = probe.case;
        // A fresh emulator per row: this row is about one Region going from its ceiling
        // to one below it and back.
        let transcribe = Transcribe::new();
        fill_to_the_ceiling(&transcribe, HOME).await;

        let freed = filler_name(index);
        transcribe
            .call_in_region(
                "DeleteCallAnalyticsCategory",
                json!({"CategoryName": freed}),
                Some(HOME),
            )
            .await
            .ok_status(case, 200);

        let created = transcribe
            .call_in_region("CreateCallAnalyticsCategory", probe.request(), Some(HOME))
            .await;
        created.ok(case);

        // The freed slot holds a REAL category: the create that took it is read back
        // through `GetCallAnalyticsCategory`, reporting the rules it was created with.
        let read = transcribe
            .call_in_region(
                "GetCallAnalyticsCategory",
                json!({"CategoryName": probe.name}),
                Some(HOME),
            )
            .await;
        let properties = read.ok(case)["CategoryProperties"].clone();
        assert_eq!(
            properties.get("Rules"),
            Some(&as_recorded(&Value::Array(probe.rules.clone()))),
            "case {case:?}: the category in the freed slot must report the rules it was \
             created with, got {properties:?}"
        );
        let held = names_held(&transcribe, HOME, case).await;
        assert_eq!(
            held.len(),
            MAX_CALL_ANALYTICS_CATEGORIES,
            "case {case:?}: expected the Region back at {MAX_CALL_ANALYTICS_CATEGORIES} \
             categories, got {}",
            held.len()
        );
        assert!(
            !held.contains(&freed),
            "case {case:?}: the deleted category must be gone, got {held:?}"
        );

        // Exactly ONE slot was freed: a second create is refused again.
        transcribe
            .call_in_region(
                "CreateCallAnalyticsCategory",
                json!({
                    "CategoryName": "one-too-many",
                    "Rules": [transcript_rule("one too many")]
                }),
                Some(HOME),
            )
            .await
            .refused(case, CLIENT_ERROR_STATUS, QUOTA_CODE);
    }
}

/// RULE 4: the ceiling is the Region's own — a Region holding nothing serves its first
/// create while another is at the ceiling, even for a name the full Region holds, and
/// the full Region is unchanged by it.
#[tokio::test]
async fn the_ceiling_counts_only_the_region_the_request_is_served_in() {
    let transcribe = Transcribe::new();
    fill_to_the_ceiling(&transcribe, HOME).await;

    for (index, probe) in probes().into_iter().enumerate() {
        let case = probe.case;
        // The name a filler holds in the full Region: creating it elsewhere is served,
        // because the ceiling the refusal above reports is per Region and so is the
        // uniqueness of a category name.
        let shared = filler_name(index);
        transcribe
            .call_in_region(
                "CreateCallAnalyticsCategory",
                json!({
                    "CategoryName": shared,
                    "Rules": probe.rules,
                    "InputType": "POST_CALL"
                }),
                Some(ELSEWHERE),
            )
            .await
            .ok(case);

        let read = transcribe
            .call_in_region(
                "GetCallAnalyticsCategory",
                json!({"CategoryName": shared}),
                Some(ELSEWHERE),
            )
            .await;
        assert_eq!(
            read.ok(case)["CategoryProperties"].get("CategoryName"),
            Some(&json!(shared)),
            "case {case:?}: {ELSEWHERE} must report the category it was asked to create, \
             got {:?}",
            read.body
        );
    }

    let held = names_held(&transcribe, HOME, "the full Region after the other Region's creates")
        .await;
    assert_eq!(
        held.len(),
        MAX_CALL_ANALYTICS_CATEGORIES,
        "case {:?}: expected the full Region still at {MAX_CALL_ANALYTICS_CATEGORIES} \
         categories, got {}",
        "the full Region after the other Region's creates",
        held.len()
    );
    transcribe
        .call_in_region(
            "CreateCallAnalyticsCategory",
            json!({
                "CategoryName": "still-one-too-many",
                "Rules": [transcript_rule("still one too many")]
            }),
            Some(HOME),
        )
        .await
        .refused(
            "the full Region after the other Region's creates",
            CLIENT_ERROR_STATUS,
            QUOTA_CODE,
        );
}
