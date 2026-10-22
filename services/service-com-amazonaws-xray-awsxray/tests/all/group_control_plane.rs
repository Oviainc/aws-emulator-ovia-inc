// SPDX-License-Identifier: CC0-1.0

//! The X-Ray group control plane, over the wire: the reserved `Default` group,
//! `FilterExpression` validation on `CreateGroup` / `UpdateGroup`, and the code
//! and message each class of refusal carries.
//!
//! Every case drives the same front door the production router does — a
//! `MultiServiceRouter` with this service mounted, addressed by the `xray`
//! endpoint host, speaking restJson1 — so the assertions are about HTTP
//! responses, not about internal state.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along
//! what the rule generalises over:
//!
//! * every account and region holds a group named `Default` that nobody
//!   created: `GetGroups` lists it, `GetGroup` resolves it by either selector,
//!   its ARN is stable across reads, and it can be neither created nor deleted;
//! * a group belongs to the region it was created in: the same name is free in
//!   every other region, another region's ARN resolves to nothing, and a delete
//!   addresses the record the lookup resolved rather than a name, so it leaves
//!   every other region's group intact;
//! * `CreateGroup` and `UpdateGroup` PARSE the `FilterExpression` and refuse,
//!   with `InvalidRequestException` and before any write, one that does not
//!   parse, one addressing a field X-Ray does not index, and one that is empty
//!   or omitted;
//! * a violated MODELLED constraint (`@length`, `@range`, `@required`) is
//!   refused by the request layer as `ValidationException`, in AWS's
//!   "<N> validation error(s) detected: …" envelope — not as any operation's
//!   own modelled error;
//! * a group-name collision is worded from the request (`<name> already
//!   exists`) and a lookup that matches nothing carries the fixed text `Group
//!   not found`.

use serde_json::{json, Value};

use super::trace_ingest_and_groups::{array, assert_group_arn_shape, Answer, Xray};

/// The reserved group name every account and region holds.
const DEFAULT_GROUP: &str = "Default";

/// `CreateGroup` with a name and a filter expression.
async fn create_group(xray: &Xray, name: &str, filter: &str) -> Answer {
    xray.call(
        "/CreateGroup",
        &json!({"GroupName": name, "FilterExpression": filter}),
    )
    .await
}

/// `GetGroup` by `GroupName` — the reader a write is round-tripped through.
async fn get_group_by_name(xray: &Xray, name: &str) -> Answer {
    xray.call("/GetGroup", &json!({"GroupName": name})).await
}

/// `GetGroups`, first page.
async fn get_groups(xray: &Xray) -> Answer {
    xray.call("/Groups", &json!({})).await
}

/// A refusal's message, whichever casing the envelope used: a framework refusal
/// renders `message`, a modelled error shape renders `Message`.
fn message(answer: &Answer) -> Option<&str> {
    answer
        .get("message")
        .as_str()
        .or_else(|| answer.get("Message").as_str())
}

/// The `Groups` entry named `name`, if the listing carries one.
fn listed<'a>(answer: &'a Answer, name: &str) -> Option<&'a Value> {
    array(&answer.body, "Groups")
        .iter()
        .find(|group| group.get("GroupName").and_then(Value::as_str) == Some(name))
}

/// A string member of a JSON object, or `None`.
fn string<'a>(value: &'a Value, member: &str) -> Option<&'a str> {
    value.get(member).and_then(Value::as_str)
}

// =============================================================================
// 1. The reserved account-level `Default` group
// =============================================================================

#[tokio::test]
async fn the_default_group_exists_without_being_created_and_cannot_be_removed() {
    let xray = Xray::new();

    // A FRESH emulator already holds it, as a fresh account does.
    let fresh = get_groups(&xray).await;
    assert_eq!(fresh.status, 200, "GetGroups must succeed, got {fresh:?}");
    let entry = listed(&fresh, DEFAULT_GROUP).unwrap_or_else(|| {
        panic!("a fresh emulator must list the reserved {DEFAULT_GROUP} group, got {fresh:?}")
    });
    let arn = entry
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // The same ARN shape every group carries, checked by the sibling suite's
    // helper rather than a second copy of it.
    assert_group_arn_shape(
        "the reserved group on a fresh emulator",
        DEFAULT_GROUP,
        &arn,
    );

    // It is readable by either selector, and the ARN is stable across reads —
    // listing it, reading it by name and reading it by ARN all answer the same.
    for (label, body) in [
        ("by GroupName", json!({"GroupName": DEFAULT_GROUP})),
        ("by GroupARN", json!({"GroupARN": arn})),
    ] {
        for attempt in 1..=2 {
            let read = xray.call("/GetGroup", &body).await;
            assert_eq!(
                read.status, 200,
                "case {label:?} attempt {attempt}: GetGroup must succeed, got {read:?}"
            );
            assert_eq!(
                string(read.get("Group"), "GroupARN"),
                Some(arn.as_str()),
                "case {label:?} attempt {attempt}: the ARN must be stable across reads, got {read:?}"
            );
            assert_eq!(
                string(read.get("Group"), "GroupName"),
                Some(DEFAULT_GROUP),
                "case {label:?} attempt {attempt}: expected the {DEFAULT_GROUP} group, got {read:?}"
            );
        }
    }

    // The reserved name cannot be taken, and the group cannot be deleted by
    // either selector.
    let taken = create_group(&xray, DEFAULT_GROUP, "ok").await;
    assert_eq!(
        (taken.status, taken.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "the reserved name must not be creatable, got {taken:?}"
    );
    assert_eq!(
        message(&taken),
        Some(format!("{DEFAULT_GROUP} already exists").as_str()),
        "case {DEFAULT_GROUP:?}: expected the collision wording, got {taken:?}"
    );

    for (label, body) in [
        ("by GroupName", json!({"GroupName": DEFAULT_GROUP})),
        ("by GroupARN", json!({"GroupARN": arn})),
    ] {
        let refused = xray.call("/DeleteGroup", &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: deleting the reserved group must be refused, got {refused:?}"
        );
        let still_there = get_group_by_name(&xray, DEFAULT_GROUP).await;
        assert_eq!(
            still_there.status, 200,
            "case {label:?}: the reserved group must survive the refused delete, got {still_there:?}"
        );
    }

    // Updating it is allowed — it is a group, and the first update is what
    // gives it a stored record — but the ARN it is read with does not change,
    // and it is still listed exactly once.
    let updated = xray
        .call(
            "/UpdateGroup",
            &json!({
                "GroupName": DEFAULT_GROUP,
                "InsightsConfiguration": {"InsightsEnabled": true, "NotificationsEnabled": false},
            }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "the reserved group must be updatable, got {updated:?}"
    );
    assert_eq!(
        string(updated.get("Group"), "GroupARN"),
        Some(arn.as_str()),
        "updating the reserved group must not change its ARN, got {updated:?}"
    );
    let after_update = get_groups(&xray).await;
    let occurrences = array(&after_update.body, "Groups")
        .iter()
        .filter(|group| string(group, "GroupName") == Some(DEFAULT_GROUP))
        .count();
    assert_eq!(
        occurrences, 1,
        "the reserved group must be listed exactly once, got {after_update:?}"
    );
    let still_reserved = xray
        .call("/DeleteGroup", &json!({"GroupName": DEFAULT_GROUP}))
        .await;
    assert_eq!(
        (still_reserved.status, still_reserved.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "an updated reserved group is still not deletable, got {still_reserved:?}"
    );

    // And it keeps its place in the listing beside created groups.
    let created = create_group(&xray, "probe", "fault = true").await;
    assert_eq!(
        created.status, 200,
        "creating a group beside the reserved one must succeed, got {created:?}"
    );
    let both = get_groups(&xray).await;
    for name in [DEFAULT_GROUP, "probe"] {
        assert!(
            listed(&both, name).is_some(),
            "GetGroups must list {name:?}, got {both:?}"
        );
    }
}

// =============================================================================
// 2. A group lives in ONE region
// =============================================================================

#[tokio::test]
async fn a_group_belongs_to_the_region_it_was_created_in() {
    // An X-Ray group is a regional resource: its ARN carries the region, and
    // the same name in another region is another group. Rows span the regions a
    // caller can sign for, including the one the service falls back to.
    let regions = ["us-east-1", "us-west-2", "eu-central-1"];
    let xray = Xray::new();

    let mut arns = Vec::new();
    for region in regions {
        let created = xray
            .call_in_region(
                region,
                "/CreateGroup",
                &json!({"GroupName": "shared-name", "FilterExpression": "ok"}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: the same name is free in each region, got {created:?}"
        );
        let arn = string(created.get("Group"), "GroupARN")
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.contains(&format!(":{region}:")),
            "case {region:?}: the minted ARN must carry the region, got {arn:?}"
        );
        arns.push((region, arn));
    }

    // Each region reads its own record, and no other region's ARN resolves
    // there.
    for (region, arn) in &arns {
        let by_arn = xray
            .call_in_region(region, "/GetGroup", &json!({"GroupARN": arn}))
            .await;
        assert_eq!(
            (by_arn.status, string(by_arn.get("Group"), "GroupARN")),
            (200, Some(arn.as_str())),
            "case {region:?}: a region must read its own group by ARN, got {by_arn:?}"
        );
        for (other_region, other_arn) in &arns {
            if other_region == region {
                continue;
            }
            let foreign = xray
                .call_in_region(region, "/GetGroup", &json!({"GroupARN": other_arn}))
                .await;
            assert_eq!(
                (foreign.status, message(&foreign)),
                (400, Some("Group not found")),
                "case {region:?}: {other_region:?}'s ARN must not resolve here, got {foreign:?}"
            );
        }
    }

    // Deleting in one region leaves every other region's group intact — the
    // removal addresses the record the lookup resolved, not a name.
    let (deleted_region, _) = arns[1];
    let deleted = xray
        .call_in_region(
            deleted_region,
            "/DeleteGroup",
            &json!({"GroupName": "shared-name"}),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case {deleted_region:?}: DeleteGroup must succeed, got {deleted:?}"
    );
    for (region, arn) in &arns {
        let read = xray
            .call_in_region(region, "/GetGroup", &json!({"GroupName": "shared-name"}))
            .await;
        if *region == deleted_region {
            assert_eq!(
                (read.status, message(&read)),
                (400, Some("Group not found")),
                "case {region:?}: the deleted group must be gone, got {read:?}"
            );
            continue;
        }
        assert_eq!(
            (read.status, string(read.get("Group"), "GroupARN")),
            (200, Some(arn.as_str())),
            "case {region:?}: a delete elsewhere must leave this group intact, got {read:?}"
        );
    }

    // And each region's listing carries its own group and its own reserved
    // `Default` group, nobody else's.
    for (region, arn) in &arns {
        if *region == deleted_region {
            continue;
        }
        let listed_here = xray.call_in_region(region, "/Groups", &json!({})).await;
        let entries: Vec<(&str, &str)> = array(&listed_here.body, "Groups")
            .iter()
            .filter_map(|group| {
                Some((string(group, "GroupName")?, string(group, "GroupARN")?))
            })
            .collect();
        assert!(
            entries.contains(&("shared-name", arn.as_str())),
            "case {region:?}: the listing must carry this region's group, got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .all(|(_, listed_arn)| listed_arn.contains(&format!(":{region}:"))),
            "case {region:?}: every listed group must belong to this region, got {entries:?}"
        );
        assert!(
            entries.iter().any(|(name, _)| *name == DEFAULT_GROUP),
            "case {region:?}: the reserved group must be listed here too, got {entries:?}"
        );
    }
}

// =============================================================================
// 3. FilterExpression is parsed, and a refusal writes nothing
// =============================================================================

/// Expressions X-Ray's filter-expression language accepts. Rows differ along
/// what the grammar generalises over: the keyword's type (boolean, numeric,
/// string), the comparison operator, boolean composition and negation,
/// parenthesised grouping, a complex keyword with arguments and with a nested
/// block, a caller-chosen annotation key, and a root-cause path.
///
/// The keyword catalog these address is AWS's X-Ray "Using filter expressions"
/// reference — `annotation.<key>`, `edge(…)`, `http.*` and `rootcause.*` are
/// entries the issue did not cite.
const PARSES: &[(&str, &str)] = &[
    ("a bare boolean keyword", "ok"),
    ("a negated boolean keyword", "NOT fault"),
    ("a boolean keyword compared", "fault = true"),
    ("a boolean keyword compared to false", "error = false"),
    ("a numeric keyword", "responsetime > 2"),
    ("a numeric keyword, inclusive", "duration >= 0.5"),
    ("a numeric keyword, not equal", "http.status != 200"),
    ("a string keyword", "http.url = \"https://example.test/a\""),
    ("a string keyword with a method", "http.method = \"POST\""),
    ("a user keyword", "user = \"probe\""),
    ("an annotation key", "annotation.probe = \"edge\""),
    ("an annotation key compared numerically", "annotation.n > 10"),
    ("a string operator", "http.url CONTAINS \"/api\""),
    ("a complex keyword", "service(\"svc\")"),
    ("a complex keyword with a block", "service(\"svc\") { fault }"),
    ("an edge keyword", "edge(\"a\", \"b\")"),
    ("a labelled argument", "id(name: \"svc\", type: \"AWS::EC2::Instance\")"),
    ("a root-cause path", "rootcause.fault.service { name = \"svc\" }"),
    ("AND composition", "fault = true AND responsetime > 1"),
    ("OR composition", "ok OR error"),
    ("parenthesised grouping", "(ok OR error) AND duration < 3"),
    (
        "nested grouping and negation",
        "NOT (annotation.probe = \"edge\" OR (http.status >= 400 AND partial))",
    ),
];

/// Expressions X-Ray refuses. Rows differ along the way the expression is
/// wrong: unparseable prose, a doubled operator, a doubled keyword, an
/// unterminated string, unbalanced brackets, a dangling operator, a field the
/// service does not index (`metadata.<key>` is the ordinary case) and a value
/// that is not a value.
const REFUSED: &[(&str, &str)] = &[
    ("prose", "this is (not a valid filter"),
    ("a doubled comparison and dangling brackets", "annotation.marker === \"x\" AND AND ((("),
    ("an unterminated string", "service(\"unclosed"),
    ("an unclosed group", "(fault = true"),
    ("an unopened group", "fault = true)"),
    ("an unclosed block", "service(\"svc\") { fault"),
    ("a dangling AND", "fault = true AND"),
    ("a leading AND", "AND fault"),
    ("a comparison with no value", "responsetime >"),
    ("a comparison with no field", "> 2"),
    ("an unindexed metadata field", "metadata.probe_key = \"0bbb73411a\""),
    ("an unindexed metadata field, bare", "metadata.anything"),
    ("an unindexed field", "segment.name = \"svc\""),
    ("an unindexed field inside a block", "service(\"svc\") { metadata.k = 1 }"),
    ("an unknown keyword", "whatever = 1"),
    ("a stray character", "fault = true & ok"),
];

#[tokio::test]
async fn create_group_parses_the_filter_expression_and_stores_only_what_parses() {
    let xray = Xray::new();

    for (index, (label, expression)) in PARSES.iter().enumerate() {
        let name = format!("ok-{index:02}");
        let created = create_group(&xray, &name, expression).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: {expression:?} must be accepted, got {created:?}"
        );
        // Round-tripped through the implemented reader: the stored record, not
        // the request, is what answers.
        let read = get_group_by_name(&xray, &name).await;
        assert_eq!(
            string(read.get("Group"), "FilterExpression"),
            Some(*expression),
            "case {label:?}: the stored expression must read back, got {read:?}"
        );
    }

    for (index, (label, expression)) in REFUSED.iter().enumerate() {
        let name = format!("bad-{index:02}");
        let refused = create_group(&xray, &name, expression).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: {expression:?} must be refused, got {refused:?}"
        );
        assert!(
            message(&refused).is_some_and(|text| !text.is_empty()),
            "case {label:?}: a refusal must say what was wrong, got {refused:?}"
        );
        // Nothing was written: the name is still free, and the group does not
        // read back.
        let missing = get_group_by_name(&xray, &name).await;
        assert_eq!(
            (missing.status, message(&missing)),
            (400, Some("Group not found")),
            "case {label:?}: a refused CreateGroup must store nothing, got {missing:?}"
        );
    }
}

#[tokio::test]
async fn the_filter_expression_parser_holds_its_declared_bounds() {
    use service_com_amazonaws_xray_awsxray::state::filter_expression::{MAX_DEPTH, MAX_LENGTH};

    let xray = Xray::new();

    // An expression exactly at each declared bound is accepted; one character
    // or one level past it is refused, so neither guard can be dropped without
    // failing here. Padding with trailing whitespace is what makes the length
    // row land exactly on the bound.
    let long = {
        let mut expression = "ok".to_string();
        while expression.len() + " OR ok".len() <= MAX_LENGTH {
            expression.push_str(" OR ok");
        }
        while expression.len() < MAX_LENGTH {
            expression.push(' ');
        }
        expression
    };
    let deep = format!("{}ok{}", "(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));

    for (label, name, expression) in [
        ("an expression at the length bound", "at-length", long.clone()),
        ("an expression at the depth bound", "at-depth", deep.clone()),
    ] {
        let created = create_group(&xray, name, &expression).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: an expression at the bound must be accepted, got {created:?}"
        );
    }

    for (label, name, expression) in [
        (
            "an expression one character past the length bound",
            "past-length",
            format!("{long} "),
        ),
        (
            "an expression one level past the depth bound",
            "past-depth",
            format!("{}ok{}", "(".repeat(MAX_DEPTH + 1), ")".repeat(MAX_DEPTH + 1)),
        ),
    ] {
        let refused = create_group(&xray, name, &expression).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: an expression past the bound must be refused, got {refused:?}"
        );
        let missing = get_group_by_name(&xray, name).await;
        assert_eq!(
            (missing.status, message(&missing)),
            (400, Some("Group not found")),
            "case {label:?}: a refused CreateGroup must store nothing, got {missing:?}"
        );
    }
}

#[tokio::test]
async fn an_empty_or_omitted_filter_expression_is_refused_alike() {
    // "Filter Expression is empty" is X-Ray's own wording for both forms.
    for (label, body) in [
        (
            "an explicitly empty FilterExpression",
            json!({"GroupName": "empty-filter", "FilterExpression": ""}),
        ),
        (
            "a whitespace-only FilterExpression",
            json!({"GroupName": "empty-filter", "FilterExpression": "   "}),
        ),
        (
            "an omitted FilterExpression",
            json!({"GroupName": "empty-filter"}),
        ),
    ] {
        let xray = Xray::new();
        let refused = xray.call("/CreateGroup", &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: expected 400 InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            Some("Filter Expression is empty"),
            "case {label:?}: expected X-Ray's empty-filter wording, got {refused:?}"
        );
        let missing = get_group_by_name(&xray, "empty-filter").await;
        assert_eq!(
            (missing.status, message(&missing)),
            (400, Some("Group not found")),
            "case {label:?}: no group may have been created, got {missing:?}"
        );
    }
}

#[tokio::test]
async fn update_group_parses_the_filter_expression_and_keeps_the_prior_one_on_refusal() {
    let xray = Xray::new();
    let prior = "annotation.probe = \"edge\"";
    let created = create_group(&xray, "updatable", prior).await;
    assert_eq!(created.status, 200, "seeding must succeed, got {created:?}");
    let arn = created
        .get("Group")
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Every refused expression leaves the stored one exactly as it was.
    for (label, expression) in REFUSED.iter().chain(std::iter::once(&("an empty expression", ""))) {
        let refused = xray
            .call(
                "/UpdateGroup",
                &json!({"GroupName": "updatable", "FilterExpression": expression}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: {expression:?} must be refused, got {refused:?}"
        );
        let read = get_group_by_name(&xray, "updatable").await;
        assert_eq!(
            string(read.get("Group"), "FilterExpression"),
            Some(prior),
            "case {label:?}: the prior expression must survive, got {read:?}"
        );
    }

    // An expression that parses replaces it, by either selector, and the ARN
    // minted at creation does not change.
    for (index, (label, expression)) in PARSES.iter().enumerate() {
        let body = if index % 2 == 0 {
            json!({"GroupName": "updatable", "FilterExpression": expression})
        } else {
            json!({"GroupARN": arn, "FilterExpression": expression})
        };
        let updated = xray.call("/UpdateGroup", &body).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: {expression:?} must be accepted, got {updated:?}"
        );
        assert_eq!(
            string(updated.get("Group"), "GroupARN"),
            Some(arn.as_str()),
            "case {label:?}: UpdateGroup must not re-mint the ARN, got {updated:?}"
        );
        let read = get_group_by_name(&xray, "updatable").await;
        assert_eq!(
            string(read.get("Group"), "FilterExpression"),
            Some(*expression),
            "case {label:?}: the update must be persisted, got {read:?}"
        );
    }

    // An omitted FilterExpression updates the other members and leaves the
    // stored expression alone.
    let last = PARSES.last().expect("the table is not empty").1;
    let insights = xray
        .call(
            "/UpdateGroup",
            &json!({
                "GroupName": "updatable",
                "InsightsConfiguration": {"InsightsEnabled": true, "NotificationsEnabled": false},
            }),
        )
        .await;
    assert_eq!(
        insights.status, 200,
        "an update of the insights configuration alone must succeed, got {insights:?}"
    );
    let read = get_group_by_name(&xray, "updatable").await;
    assert_eq!(
        string(read.get("Group"), "FilterExpression"),
        Some(last),
        "an omitted FilterExpression must leave the stored one alone, got {read:?}"
    );
    assert_eq!(
        read.get("Group")
            .get("InsightsConfiguration")
            .and_then(|value| value.get("InsightsEnabled"))
            .and_then(Value::as_bool),
        Some(true),
        "the supplied insights configuration must be persisted, got {read:?}"
    );

    // Updating a group that does not exist carries the fixed not-found text.
    let absent = xray
        .call(
            "/UpdateGroup",
            &json!({"GroupName": "absent", "FilterExpression": "ok"}),
        )
        .await;
    assert_eq!(
        (absent.status, message(&absent)),
        (400, Some("Group not found")),
        "updating an unknown group must be refused as not found, got {absent:?}"
    );
}

// =============================================================================
// 3. A violated MODELLED constraint is a ValidationException
// =============================================================================

/// A `SamplingRule` every member of which satisfies its modelled constraints,
/// so a row below violates exactly the one member it names.
fn sampling_rule() -> Value {
    json!({
        "RuleName": "probe",
        "ResourceARN": "*",
        "Priority": 1,
        "FixedRate": 0.5,
        "ReservoirSize": 0,
        "ServiceName": "*",
        "ServiceType": "*",
        "Host": "*",
        "HTTPMethod": "*",
        "URLPath": "*",
        "Version": 1,
    })
}

/// `sampling_rule()` with `member` replaced by `value`.
fn sampling_rule_with(member: &str, value: Value) -> Value {
    let mut rule = sampling_rule();
    rule[member] = value;
    json!({"SamplingRule": rule})
}

#[tokio::test]
async fn a_violated_modelled_constraint_is_refused_as_a_validation_exception() {
    let xray = Xray::new();

    // Rows span the constraint classes and both ends of every published bound:
    // `CreateGroup.GroupName` @length 1..32 (`smithy_query(query="shape",
    // shape="GroupName")`), `SamplingRule.Priority` @range 1..9999 and
    // `SamplingRule.FixedRate` @range 0..1 (`shape="SamplingRule"`), plus an
    // absent @required member.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "GroupName one character under the minimum length",
            "/CreateGroup",
            json!({"GroupName": "", "FilterExpression": "ok"}),
            "groupName",
        ),
        (
            "GroupName one character over the maximum length",
            "/CreateGroup",
            json!({"GroupName": "a".repeat(33), "FilterExpression": "ok"}),
            "groupName",
        ),
        (
            "a GroupName far over the maximum length",
            "/CreateGroup",
            json!({"GroupName": "a".repeat(37), "FilterExpression": "ok"}),
            "groupName",
        ),
        (
            "GroupName absent",
            "/CreateGroup",
            json!({"FilterExpression": "ok"}),
            "groupName",
        ),
        (
            "Priority one under the minimum",
            "/CreateSamplingRule",
            sampling_rule_with("Priority", json!(0)),
            "priority",
        ),
        (
            "Priority one over the maximum",
            "/CreateSamplingRule",
            sampling_rule_with("Priority", json!(10_000)),
            "priority",
        ),
        (
            "FixedRate under the minimum",
            "/CreateSamplingRule",
            sampling_rule_with("FixedRate", json!(-0.5)),
            "fixedRate",
        ),
        (
            "FixedRate over the maximum",
            "/CreateSamplingRule",
            sampling_rule_with("FixedRate", json!(2.0)),
            "fixedRate",
        ),
        (
            "ReservoirSize under the minimum",
            "/CreateSamplingRule",
            sampling_rule_with("ReservoirSize", json!(-1)),
            "reservoirSize",
        ),
        (
            "RuleName over the maximum length",
            "/CreateSamplingRule",
            sampling_rule_with("RuleName", json!("r".repeat(33))),
            "ruleName",
        ),
    ];

    for (label, path, body, member) in rows {
        let refused = xray.call(path, &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {label:?}: expected 400 ValidationException, got {refused:?}"
        );
        let message = message(&refused).unwrap_or_default();
        assert!(
            message.starts_with("1 validation error detected: "),
            "case {label:?}: expected AWS's validation envelope, got {message:?}"
        );
        assert!(
            message.contains(&format!("at '{member}'"))
                || message.contains(&format!(".{member}'")),
            "case {label:?}: the message must name {member:?}, got {message:?}"
        );
        assert!(
            message.contains("failed to satisfy constraint:"),
            "case {label:?}: the message must state the constraint, got {message:?}"
        );
    }

    // Each published bound, at the value the model allows, is NOT a constraint
    // refusal: `CreateGroup` succeeds, and `CreateSamplingRule` gets past the
    // request layer (it is scaffolded, so it answers 501, never a 400).
    for (label, name) in [
        ("the shortest GroupName the model allows", "a".to_string()),
        ("the longest GroupName the model allows", "a".repeat(32)),
    ] {
        let created = create_group(&xray, &name, "ok").await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: a name inside the bound must be accepted, got {created:?}"
        );
    }
    for (label, member, value) in [
        ("Priority at its minimum", "Priority", json!(1)),
        ("Priority at its maximum", "Priority", json!(9_999)),
        ("FixedRate at its minimum", "FixedRate", json!(0.0)),
        ("FixedRate at its maximum", "FixedRate", json!(1.0)),
    ] {
        let answer = xray
            .call("/CreateSamplingRule", &sampling_rule_with(member, value))
            .await;
        assert_ne!(
            answer.error_type.as_deref(),
            Some("ValidationException"),
            "case {label:?}: a value inside the bound must pass the request layer, got {answer:?}"
        );
    }
}

// =============================================================================
// 4. Collision and not-found wording
// =============================================================================

#[tokio::test]
async fn a_name_collision_is_worded_from_the_request_and_a_miss_is_group_not_found() {
    let xray = Xray::new();

    // Rows differ in the name's length, character set and case — the collision
    // message is built from whichever name the request carried.
    for name in [
        "a",
        "probe",
        "Prod-Group_1.0",
        "abcdefghijklmnopqrstuvwxyz012345",
        DEFAULT_GROUP,
    ] {
        if name != DEFAULT_GROUP {
            let created = create_group(&xray, name, "ok").await;
            assert_eq!(
                created.status, 200,
                "case {name:?}: seeding must succeed, got {created:?}"
            );
        }
        let again = create_group(&xray, name, "ok").await;
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {name:?}: a duplicate name must be refused, got {again:?}"
        );
        assert_eq!(
            message(&again),
            Some(format!("{name} already exists").as_str()),
            "case {name:?}: expected \"<name> already exists\", got {again:?}"
        );
    }

    // A lookup that matches nothing carries the fixed text, whatever selector
    // was used and whether the group never existed or was deleted.
    let created = create_group(&xray, "transient", "ok").await;
    let transient_arn = created
        .get("Group")
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let deleted = xray
        .call("/DeleteGroup", &json!({"GroupName": "transient"}))
        .await;
    assert_eq!(deleted.status, 200, "DeleteGroup must succeed, got {deleted:?}");

    let misses: Vec<(&str, &str, Value)> = vec![
        (
            "GetGroup on a name that never existed",
            "/GetGroup",
            json!({"GroupName": "never"}),
        ),
        (
            "GetGroup on a deleted name",
            "/GetGroup",
            json!({"GroupName": "transient"}),
        ),
        (
            "GetGroup on a deleted group's ARN",
            "/GetGroup",
            json!({"GroupARN": transient_arn}),
        ),
        (
            "GetGroup on an ARN this service never minted",
            "/GetGroup",
            json!({"GroupARN": "arn:aws:xray:us-east-1:000000000000:group/never/0123456789ab"}),
        ),
        (
            "DeleteGroup on a deleted name",
            "/DeleteGroup",
            json!({"GroupName": "transient"}),
        ),
        (
            "UpdateGroup on a name that never existed",
            "/UpdateGroup",
            json!({"GroupName": "never", "FilterExpression": "ok"}),
        ),
    ];
    for (label, path, body) in misses {
        let refused = xray.call(path, &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: expected 400 InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            Some("Group not found"),
            "case {label:?}: expected the fixed not-found text, got {refused:?}"
        );
    }
}
