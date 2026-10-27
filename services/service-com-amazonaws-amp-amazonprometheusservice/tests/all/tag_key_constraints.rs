// SPDX-License-Identifier: CC0-1.0

//! `TagKey`'s modelled constraints, on the two request paths that carry a tag key and
//! that `tests/all/map_key_constraints.rs` does not reach.
//!
//! The rule: wherever a request carries a tag KEY, a key violating the `TagKey` shape's
//! modelled `@length(min: 1, max: 128)` or `@pattern` is refused HTTP 400
//! `ValidationException` — naming the member the key arrived in and the requirement it
//! failed — and nothing is created, tagged or untagged.
//!
//! `tests/all/map_key_constraints.rs` asserts that over the `TagMap` members of
//! `CreateWorkspace`, `CreateAnomalyDetector`, `CreateRuleGroupsNamespace` and
//! `TagResource`. What is left is
//!
//! * `CreateScraper$tags` — the fifth and last operation whose input carries a `TagMap`
//!   (`smithy_query(query="operation", operation="CreateScraper")`), and
//! * `UntagResource$tagKeys` — the only member where a tag key arrives as a LIST element
//!   rather than as a map key, so the constraints are the request boundary's to apply
//!   rather than the handler's.
//!
//! The rows also span character classes the other suite does not: ASCII punctuation
//! outside the pattern's list, a currency symbol, an emoji (both Unicode symbols, which
//! are neither letters, separators nor numbers) and a tab (a control character, which is
//! not a separator).
//!
//! The harness (`Amp`, `Reply`, `text`, `tags_of`) is the one
//! `tests/all/workspace_lifecycle.rs` owns, and the refusal assertion, the wire map and
//! the requirement wordings are `tests/all/map_key_constraints.rs`'s, so this suite adds
//! no second set of rules about either.

use serde_json::{json, Value};

use super::map_key_constraints::{
    assert_member_refusal, tag_sinks, wire_map, MIN_LENGTH_REQUIREMENT, TAG_KEY_MAX_LENGTH,
    TAG_KEY_MAX_LENGTH_REQUIREMENT, TAG_KEY_PATTERN_REQUIREMENT,
};
use super::scraper_creation::{eks_source_json, workspace_destination_json};
use super::scraper_update::scrape_configuration_json;
use super::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    reserved_tag_key_requirement, VALIDATION_REASON,
};

/// One row: the case name, the tag key submitted, and the requirement the refusal must
/// report.
type KeyCase<'a> = (&'a str, &'a str, &'a str);

/// Tag keys no `TagKey` the model admits can be, each with the clause it misses. The
/// character rows all satisfy both length bounds, so the pattern is the only clause at
/// fault; the two length rows use a key the pattern admits, so the bound is.
fn refused_keys(too_long: &str) -> Vec<KeyCase<'_>> {
    vec![
        (
            "an exclamation mark, which no class of the pattern admits",
            "bad!key",
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
        ("an ampersand", "team&ops", TAG_KEY_PATTERN_REQUIREMENT),
        (
            "parentheses",
            "cost(center)",
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
        (
            // A currency symbol is Unicode category Sc — a symbol, so neither a letter
            // (\p{L}), a separator (\p{Z}) nor a number (\p{N}).
            "a currency symbol",
            "\u{20ac}uro",
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
        (
            // An emoji is category So, a symbol for the same reason.
            "an emoji",
            "team\u{1f680}",
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
        (
            // A tab is a control character (Cc), which `\p{Z}` does not cover even
            // though a plain space (Zs) is admitted.
            "a tab, which is not a separator",
            "team\tops",
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "one past the published maximum length",
            too_long,
            TAG_KEY_MAX_LENGTH_REQUIREMENT,
        ),
    ]
}

/// A `CreateScraper` body: the three `@required` members plus the `tags` under test.
fn scraper_body(region: &str, workspace_arn: &str, tags: Value) -> Value {
    json!({
        "scrapeConfiguration": scrape_configuration_json("global: {}\n"),
        "source": eks_source_json(region, "tag-cluster", &["subnet-0123456789abcdef0"], &[]),
        "destination": workspace_destination_json(workspace_arn),
        "tags": tags,
    })
}

/// `POST /scrapers`.
async fn create_scraper(amp: &Amp, region: &str, body: &Value) -> Reply {
    amp.call("POST", "/scrapers", region, &body.to_string()).await
}

/// `GET /scrapers/{scraperId}`.
async fn describe_scraper(amp: &Amp, region: &str, scraper_id: &str) -> Reply {
    amp.call("GET", &format!("/scrapers/{scraper_id}"), region, "")
        .await
}

/// One workspace, created through `CreateWorkspace` — the ARN an `ampConfiguration`
/// destination names, and the resource the untag rows are seeded on.
async fn workspace_arn(amp: &Amp, region: &str) -> String {
    let created = amp
        .call("POST", "/workspaces", region, &json!({ "alias": "tags" }).to_string())
        .await;
    assert_eq!(
        created.status, 202,
        "case \"seed\": the workspace under test must be creatable, got {created:?}"
    );
    text(&created.body["arn"])
}

/// `CreateScraper$tags`: a key outside `TagKey`'s constraints is refused, and the store
/// holds no scraper afterwards.
#[tokio::test]
async fn a_scraper_create_carrying_a_tag_key_outside_the_key_shape_is_refused() {
    let amp = Amp::new();
    let region = "us-west-2";
    let destination = workspace_arn(&amp, region).await;
    let too_long = "t".repeat(TAG_KEY_MAX_LENGTH + 1);

    let mut cases = refused_keys(&too_long);
    cases.push((
        "the empty key, one below the published minimum length",
        "",
        MIN_LENGTH_REQUIREMENT,
    ));

    for (case, key, requirement) in cases {
        let body = scraper_body(region, &destination, wire_map(&[(key, "v")]));
        let reply = create_scraper(&amp, region, &body).await;
        assert_member_refusal(case, &reply, "tags", requirement);
        assert!(
            reply.body.get("scraperId").is_none(),
            "case {case:?}: a refused create must mint nothing, got {reply:?}"
        );
        assert_eq!(
            amp.store().lock().scrapers.len(),
            0,
            "case {case:?}: a refused create must persist no scraper, got {} held",
            amp.store().lock().scrapers.len()
        );
    }

    // A well-formed key beside a bad one does not rescue the map.
    let body = scraper_body(
        region,
        &destination,
        wire_map(&[("team", "obs"), ("bad!key", "v")]),
    );
    let reply = create_scraper(&amp, region, &body).await;
    assert_member_refusal(
        "a well-formed key beside a bad one",
        &reply,
        "tags",
        TAG_KEY_PATTERN_REQUIREMENT,
    );
}

/// The accepting half over the same member: every key form `TagKey` admits is stored and
/// read back through `DescribeScraper`, including a key at each published length bound.
#[tokio::test]
async fn every_tag_key_the_key_shape_admits_round_trips_through_a_scraper_create() {
    let amp = Amp::new();
    let region = "eu-west-1";
    let destination = workspace_arn(&amp, region).await;
    let at_max_length = "t".repeat(TAG_KEY_MAX_LENGTH);

    // (case, the tags the create carries)
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("a plain key", vec![("team", "obs")]),
        (
            "the punctuation the pattern lists",
            vec![("cost.center:eu/west=1+2-3@aws", "v")],
        ),
        ("a Unicode letter key", vec![("\u{43a}\u{43b}\u{44e}\u{447}", "v")]),
        ("a separator and a number", vec![("cost center 2", "v")]),
        (
            "a single character, the published minimum length",
            vec![("t", "v")],
        ),
        (
            "at the published maximum length",
            vec![(at_max_length.as_str(), "v")],
        ),
    ];

    for (case, tags) in cases {
        let body = scraper_body(region, &destination, wire_map(&tags));
        let created = create_scraper(&amp, region, &body).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: a key the key shape admits must be accepted, got {created:?}"
        );
        let scraper_id = text(&created.body["scraperId"]);
        let described = describe_scraper(&amp, region, &scraper_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the minted scraper must read back, got {described:?}"
        );
        assert_eq!(
            described.body["scraper"]["tags"],
            wire_map(&tags),
            "case {case:?}: describe must report the tags the service persisted, \
             got {described:?}"
        );
    }
}

/// `UntagResource$tagKeys`: the same constraints hold where a tag key arrives as a list
/// element, so a refused call removes nothing — the tag the resource already carries is
/// still there afterwards.
#[tokio::test]
async fn an_untag_naming_a_tag_key_outside_the_key_shape_is_refused() {
    let amp = Amp::new();
    let region = "ap-southeast-2";
    let resource_arn = workspace_arn(&amp, region).await;
    let too_long = "t".repeat(TAG_KEY_MAX_LENGTH + 1);

    let tagged = amp
        .tag(region, &resource_arn, &json!({ "team": "obs" }))
        .await;
    assert_eq!(
        tagged.status, 200,
        "case \"seed\": the resource must be taggable, got {tagged:?}"
    );

    for (case, key, requirement) in refused_keys(&too_long) {
        // The well-formed key is named FIRST, so a call that was not refused would have
        // removed it before reaching the key under test.
        let reply = amp.untag(region, &resource_arn, &["team", key]).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a tag key outside the key shape's constraints must be refused \
             400, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["reason"]),
            VALIDATION_REASON,
            "case {case:?}: `reason` is @required, got {reply:?}"
        );
        let fields = reply.body["fieldList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            fields.len(),
            1,
            "case {case:?}: expected exactly one refused member, got {reply:?}"
        );
        let name = text(&fields[0]["name"]);
        assert!(
            name == "tagKeys" || name.starts_with("tagKeys."),
            "case {case:?}: the refusal must name the tagKeys member, got {reply:?}"
        );
        assert_eq!(
            text(&fields[0]["message"]),
            requirement,
            "case {case:?}: expected the requirement the key failed, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["message"]),
            format!("Invalid {name}: {requirement}"),
            "case {case:?}: expected the message to name the member and its requirement, \
             got {reply:?}"
        );

        let listed = amp.list_tags(region, &resource_arn).await;
        assert_eq!(
            tags_of(&listed.body["tags"]),
            [("team".to_string(), "obs".to_string())]
                .into_iter()
                .collect(),
            "case {case:?}: a refused untag must remove nothing, got {listed:?}"
        );
    }

    // The accepting half: a key the shape admits is removed.
    let removed = amp.untag(region, &resource_arn, &["team"]).await;
    assert_eq!(
        removed.status, 200,
        "case \"a key the key shape admits\": the untag must be accepted, got {removed:?}"
    );
    let listed = amp.list_tags(region, &resource_arn).await;
    assert_eq!(
        listed.body["tags"],
        json!({}),
        "case \"a key the key shape admits\": the tag must be gone, got {listed:?}"
    );
}

/// The member's MODELLED constraints are decided before the rule this service documents
/// on top of them: a key that misses both `TagKey`'s pattern or length bound AND the
/// reserved `aws:` prefix rule is refused by the constraint — the refusal the request
/// boundary would have made — while a key that misses only the documented rule still
/// reports that rule. Asserted over every operation whose request carries a `TagMap`.
#[tokio::test]
async fn a_modelled_constraint_is_reported_before_the_reserved_prefix_rule() {
    let amp = Amp::new();
    let region = "us-east-1";
    let created = amp
        .call("POST", "/workspaces", region, &json!({ "alias": "tags" }).to_string())
        .await;
    assert_eq!(created.status, 202, "case \"seed\": {created:?}");
    let workspace_id = text(&created.body["workspaceId"]);
    let arn = text(&created.body["arn"]);
    wait_for_due_transition();

    // The reserved prefix plus 125 characters is 129 — one past the published maximum.
    let reserved_past_max = format!("aws:{}", "t".repeat(TAG_KEY_MAX_LENGTH - 3));
    let reserved_requirement = reserved_tag_key_requirement();

    let cases: Vec<KeyCase> = vec![
        (
            "the reserved prefix and a character no class of the pattern admits",
            "aws:bad!key",
            TAG_KEY_PATTERN_REQUIREMENT,
        ),
        (
            "the reserved prefix one past the published maximum length",
            reserved_past_max.as_str(),
            TAG_KEY_MAX_LENGTH_REQUIREMENT,
        ),
        (
            // The documented rule on its own is still the documented rule: this key
            // satisfies every constraint the shape declares.
            "the reserved prefix alone",
            "aws:created-by",
            reserved_requirement.as_str(),
        ),
    ];

    for (case, key, requirement) in cases {
        for sink in tag_sinks(&workspace_id, &arn) {
            let label = format!("{} with {case}", sink.operation);
            let body = (sink.body)(wire_map(&[(key, "v")]));
            let reply = amp
                .call(sink.method, &sink.path, region, &body.to_string())
                .await;
            assert_member_refusal(&label, &reply, "tags", requirement);
        }
        let label = format!("CreateScraper with {case}");
        let body = scraper_body(region, &arn, wire_map(&[(key, "v")]));
        let reply = create_scraper(&amp, region, &body).await;
        assert_member_refusal(&label, &reply, "tags", requirement);
    }

    let listed = amp.list_tags(region, &arn).await;
    assert_eq!(
        listed.body["tags"],
        json!({}),
        "case \"nothing tagged\": a refused call must persist nothing, got {listed:?}"
    );
}
