// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeInsightDetails`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`], and the refusal assertion from [`super::insights`];
//! this suite adds no second copy of either.
//!
//! The rule under test: a detail read reports the detail fields persisted on the ONE
//! insight the entity the request names holds under the requested id — each field as
//! its stored name, type and value, in the order the insight holds them — rendering
//! each value with the markup the field carries when the request asks for HTML content
//! and as plain content otherwise; the entity is resolved exactly as a listing resolves
//! it, an id that entity holds no insight under names no insight to detail and is
//! refused, and nothing is written.
//!
//! So the assertions span a SET of inputs rather than one call: a table of fields
//! covering both values of `InsightFieldType`, a field with a marked-up rendering and
//! fields without one, an empty value and a long one; then the three ways a request can
//! answer `ShowHtmlContent`, every entity type the model admits with the account named
//! both ways, four ids no entity holds an insight under, five entities this account
//! holds nothing for, and an insight carrying no detail field at all.
//!
//! The insights themselves are written into the store directly (through
//! [`super::domain_lifecycle::Emulator::store`]): the service RAISES an insight and its
//! fields from what it observes of a domain, so no wire call leaves one. Everything the
//! rule is asserted ON is read back through the wire.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    INSIGHT_ENTITY_TYPE_VALUES, INSIGHT_FIELD_TYPE_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    InsightDetailField, InsightRecord,
};

use super::domain_lifecycle::{create_baseline, Answer, Emulator, PREFIX};
use super::insights::refused;

/// One row of the detail-field table: the field's name, its type, its plain value and
/// the marked-up rendering of that same content where the field has one.
struct Row {
    name: &'static str,
    field_type: &'static str,
    value: &'static str,
    html_value: Option<&'static str>,
}

/// A long text value, built rather than written out, so a row exercises content well
/// past the length of a one-line description.
fn long_text() -> String {
    "Nodes in this domain have been running hot for several hours. ".repeat(40)
}

/// The detail fields every case below is asserted over.
///
/// The rows differ along what the rule generalises over: what the field carries (a text
/// description or metric data — between them every value of `InsightFieldType`, the
/// catalog the model declares, as `ops::types`' `INSIGHT_FIELD_TYPE_VALUES`, asserted
/// below), whether the field has a marked-up rendering of its content as well as a
/// plain one, and how long that content is — nothing at all, one line, a numeric
/// datapoint set, and many lines.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "Summary",
            field_type: "text",
            value: "Shard count on the hot data nodes is above the recommended ceiling.",
            html_value: Some(
                "<p>Shard count on the hot data nodes is above the \
                 <b>recommended ceiling</b>.</p>",
            ),
        },
        Row {
            name: "Recommendation",
            field_type: "text",
            value: "Reduce the number of shards per data node, or add data nodes.",
            html_value: None,
        },
        Row {
            name: "JVMMemoryPressure",
            field_type: "metric",
            value: "{\"Unit\":\"Percent\",\"Datapoints\":[[0,72.5],[300,81.25]]}",
            html_value: None,
        },
        Row {
            name: "SearchableDocuments",
            field_type: "metric",
            value: "0",
            html_value: None,
        },
        Row {
            name: "Notes",
            field_type: "text",
            value: "",
            html_value: None,
        },
    ]
}

/// [`rows`] as the fields the store holds on an insight, in the order a detail read
/// must report them, with the long row appended so one field's content is far longer
/// than a line.
fn stored_fields() -> Vec<InsightDetailField> {
    let mut fields: Vec<InsightDetailField> = rows()
        .iter()
        .map(|row| InsightDetailField {
            name: row.name.to_string(),
            field_type: row.field_type.to_string(),
            value: row.value.to_string(),
            html_value: row.html_value.map(str::to_string),
        })
        .collect();
    fields.push(InsightDetailField {
        name: "Observation".to_string(),
        field_type: "text".to_string(),
        value: long_text(),
        html_value: Some(format!("<p>{}</p>", long_text())),
    });
    fields
}

/// The names of [`stored_fields`], in the order the insight holds them.
fn field_names() -> Vec<String> {
    stored_fields()
        .iter()
        .map(|field| field.name.clone())
        .collect()
}

/// The `GUID` an insight of this suite is addressed by: `InsightId` is
/// `@length(36, 36)` with pattern
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$`, so an
/// id is a hyphenated UUID whose last field is what tells it apart.
fn guid(suffix: &str) -> String {
    format!("4d2e7a91-6b3c-4f5d-9e8a-{suffix}")
}

/// The id of the insight the detailed view is read back from.
fn detailed_id() -> String {
    guid("000000000001")
}

/// An insight raised at `raised`, carrying `details`.
fn record(insight_id: &str, raised: SystemTime, details: Vec<InsightDetailField>) -> InsightRecord {
    InsightRecord {
        insight_id: insight_id.to_string(),
        display_name: "Shard count is above the recommended ceiling".to_string(),
        insight_type: "RECOMMENDATION".to_string(),
        priority: "HIGH".to_string(),
        status: "ACTIVE".to_string(),
        created_at: raised,
        updated_at: raised,
        is_experimental: false,
        details,
    }
}

/// Raise `record` on the domain `name`.
fn raise_on_domain(emulator: &Emulator, name: &str, record: InsightRecord) {
    let store = emulator.store();
    let mut guard = store.lock();
    let domain = guard
        .domains
        .get_mut(name)
        .unwrap_or_else(|| panic!("case raise_on_domain({name:?}): the domain must exist"));
    domain.insights.insert(record.insight_id.clone(), record);
}

/// A domain holding the whole [`stored_fields`] table on one insight.
async fn seeded(emulator: &Emulator, name: &str) -> String {
    create_baseline(emulator, name).await;
    let id = detailed_id();
    raise_on_domain(
        emulator,
        name,
        record(&id, SystemTime::now(), stored_fields()),
    );
    id
}

/// An `InsightEntity` naming the domain `name`.
fn domain_entity(name: &str) -> Value {
    json!({"Type": "DomainName", "Value": name})
}

/// `POST /2021-01-01/opensearch/insight-details` with `body`.
async fn describe(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/insight-details"),
            Some(body),
        )
        .await
}

/// The detailed view of `insight_id` as the entity `entity` holds it, with whatever
/// extra members `extra` carries merged in.
async fn describe_insight(
    emulator: &Emulator,
    entity: Value,
    insight_id: &str,
    extra: Value,
) -> Answer {
    let mut body = json!({"Entity": entity, "InsightId": insight_id});
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    describe(emulator, body).await
}

/// The `Fields` of an accepted detail read.
fn fields(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Fields"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Fields list, got {:?}", answer.body))
        .clone()
}

/// The `Name` of every field of a detailed view, in the order it reports them.
fn names(reported: &[Value]) -> Vec<String> {
    reported
        .iter()
        .map(|field| field["Name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The `Value` the detailed view reports for the field named `name`.
fn reported_value(reported: &[Value], name: &str, case: &str) -> String {
    reported
        .iter()
        .find(|field| field["Name"].as_str() == Some(name))
        .map(|field| {
            field["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string Value, got {field:?}"))
                .to_owned()
        })
        .unwrap_or_else(|| panic!("case {case}: expected the field {name:?} to be reported"))
}

/// `POST /2021-01-01/opensearch/insights` for `entity` — the listing the detail read
/// must leave exactly as it was.
async fn list(emulator: &Emulator, entity: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/insights"),
            Some(json!({"Entity": entity})),
        )
        .await
}

#[tokio::test]
async fn a_detail_read_reports_every_stored_field_of_the_insight_in_order() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-read").await;
    let entity = domain_entity("insight-detail-read");

    let answer = describe_insight(&emulator, entity.clone(), &id, json!({})).await;
    let reported = fields(&answer, "whole detailed view");
    assert_eq!(
        names(&reported),
        field_names(),
        "case whole detailed view: expected the fields in the order the insight holds them, got {:?}",
        names(&reported)
    );

    for stored in stored_fields() {
        let case = format!("InsightField({:?})", stored.name);
        // The type catalog these rows draw on is the model's own enum, as generated
        // into `ops::types`: a row naming a value outside it would assert nothing about
        // this service's wire surface.
        assert!(
            INSIGHT_FIELD_TYPE_VALUES.contains(&stored.field_type.as_str()),
            "case {case}: the row must use a modelled field type, got {:?}",
            stored.field_type
        );

        let field = reported
            .iter()
            .find(|field| field["Name"].as_str() == Some(stored.name.as_str()))
            .unwrap_or_else(|| panic!("case {case}: expected the field to be reported"));
        assert_eq!(
            field["Type"].as_str(),
            Some(stored.field_type.as_str()),
            "case {case}: expected the stored type, got {:?}",
            field["Type"]
        );
        // No `ShowHtmlContent` on the request, so every value is the plain rendering —
        // including the fields whose content also has a marked-up one.
        assert_eq!(
            field["Value"].as_str(),
            Some(stored.value.as_str()),
            "case {case}: expected the stored plain value, got {:?}",
            field["Value"]
        );
    }

    // A detail read is a report: a second one answers exactly the same, and the listing
    // the insight belongs to is untouched, so reading the detail moved nothing.
    let before = list(&emulator, entity.clone()).await;
    let again = describe_insight(&emulator, entity.clone(), &id, json!({})).await;
    assert_eq!(
        again.body, answer.body,
        "case whole detailed view(twice): a read must leave the insight as it was, got {:?}",
        again.body
    );
    let after = list(&emulator, entity).await;
    assert_eq!(
        after.body, before.body,
        "case whole detailed view(listing): a detail read must leave the listing as it was, got {:?}",
        after.body
    );
}

#[tokio::test]
async fn show_html_content_picks_the_rendering_each_field_is_reported_in() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-html").await;
    let entity = domain_entity("insight-detail-html");

    // The three answers a request can give `ShowHtmlContent`: the marked-up rendering
    // only where it asks for HTML content.
    for (show_html, extra) in [
        (false, json!({})),
        (false, json!({"ShowHtmlContent": false})),
        (true, json!({"ShowHtmlContent": true})),
    ] {
        let case = format!("ShowHtmlContent {extra:?}");
        let answer = describe_insight(&emulator, entity.clone(), &id, extra.clone()).await;
        let reported = fields(&answer, &case);
        assert_eq!(
            names(&reported),
            field_names(),
            "case {case}: expected the same fields whichever rendering was asked for, got {:?}",
            names(&reported)
        );

        for stored in stored_fields() {
            let case = format!("{case}/{:?}", stored.name);
            let expected = match (show_html, stored.html_value.as_deref()) {
                // A field with a marked-up rendering reports it only for a request that
                // asked for HTML content; one without reports its plain content either
                // way.
                (true, Some(html)) => html.to_string(),
                _ => stored.value.clone(),
            };
            assert_eq!(
                reported_value(&reported, &stored.name, &case),
                expected,
                "case {case}: expected the {} rendering, got {:?}",
                if show_html { "marked-up" } else { "plain" },
                reported_value(&reported, &stored.name, &case)
            );
        }
    }
}

#[tokio::test]
async fn an_insight_is_addressed_through_the_entity_that_holds_it() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-entity").await;
    create_baseline(&emulator, "insight-detail-other").await;

    // One insight held for the ACCOUNT itself, so the entity table covers both homes an
    // insight can have.
    let account_wide_id = guid("000000000002");
    let account_wide = record(
        &account_wide_id,
        SystemTime::now(),
        vec![InsightDetailField {
            name: "Summary".to_string(),
            field_type: "text".to_string(),
            value: "Reserved Instance coverage is low for this account.".to_string(),
            html_value: None,
        }],
    );
    {
        let store = emulator.store();
        let mut guard = store.lock();
        guard
            .account_insights
            .insert(account_wide.insight_id.clone(), account_wide);
    }

    // Every entity type the model admits, with the account named both ways a caller can
    // name it: a `DomainName` entity holds the insights of that one domain, and an
    // `Account` entity those of the account and of every domain it holds.
    for (entity, insight_id, expected_first_field) in [
        (
            domain_entity("insight-detail-entity"),
            id.clone(),
            "Summary",
        ),
        (json!({"Type": "Account"}), id.clone(), "Summary"),
        (
            json!({"Type": "Account", "Value": engine::emulator_account_id()}),
            id.clone(),
            "Summary",
        ),
        (
            json!({"Type": "Account"}),
            account_wide_id.clone(),
            "Summary",
        ),
    ] {
        let case = format!("DescribeInsightDetails({entity:?}, {insight_id:?})");
        assert!(
            INSIGHT_ENTITY_TYPE_VALUES.contains(&entity["Type"].as_str().unwrap_or_default()),
            "case {case}: the row must name a modelled entity type"
        );
        let answer = describe_insight(&emulator, entity, &insight_id, json!({})).await;
        let reported = fields(&answer, &case);
        assert_eq!(
            names(&reported).first().map(String::as_str),
            Some(expected_first_field),
            "case {case}: expected the insight's own fields, got {:?}",
            answer.body
        );
    }

    // A sibling domain of the same account does not hold this insight, and no
    // `DomainName` entity holds the account's own one: an insight is reported by the
    // entity that holds it, so another entity of the same account names no insight to
    // detail and is refused rather than answered.
    for (entity, insight_id) in [
        (domain_entity("insight-detail-other"), id.clone()),
        (
            domain_entity("insight-detail-entity"),
            account_wide_id.clone(),
        ),
    ] {
        let answer = describe_insight(&emulator, entity.clone(), &insight_id, json!({})).await;
        refused(
            &answer,
            &format!("DescribeInsightDetails({entity:?}, {insight_id:?})"),
            409,
            "ResourceNotFoundException",
        );
    }
}

#[tokio::test]
async fn an_id_the_entity_holds_no_insight_under_is_refused() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-missing").await;
    let entity = domain_entity("insight-detail-missing");

    // Rows differ along the shape of the id: one never raised, one all zeroes, one all
    // `f`s, and the stored id with its hex digits in the other case — `GUID`'s pattern
    // admits either case, and an id is the exact value the service handed out, so one
    // differing in case names no insight.
    for not_an_insight in [
        guid("000000000099"),
        "00000000-0000-0000-0000-000000000000".to_string(),
        "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        id.to_uppercase(),
    ] {
        assert_ne!(
            not_an_insight, id,
            "case DescribeInsightDetails({not_an_insight:?}): the row must name another id"
        );
        let case = format!("DescribeInsightDetails({not_an_insight:?})");
        let answer = describe_insight(&emulator, entity.clone(), &not_an_insight, json!({})).await;
        refused(&answer, &case, 409, "ResourceNotFoundException");
        // The refusal names the id that could not be resolved and the entity it was
        // looked up in, both as the request gave them.
        let message = answer.body["message"]
            .as_str()
            .or_else(|| answer.body["Message"].as_str())
            .unwrap_or_else(|| panic!("case {case}: expected a message, got {:?}", answer.body));
        assert!(
            message.contains(&not_an_insight) && message.contains("insight-detail-missing"),
            "case {case}: expected the message to name the id and the entity, got {message:?}"
        );
    }

    // The insight the entity DOES hold is still reported in full, so an unresolvable id
    // leaves the set it was looked up in as it was.
    let answer = describe_insight(&emulator, entity, &id, json!({})).await;
    assert_eq!(
        names(&fields(&answer, "the insight that is held")),
        field_names(),
        "case the insight that is held: expected its stored fields, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn an_entity_this_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let id = seeded(&emulator, "insight-detail-entities").await;

    // Rows differ along the shape of the name: one never created, one at the shortest
    // the model admits, and one at its longest.
    for name in [
        "never-created",
        "abc",
        "a-domain-name-at-its-upper-b",
    ] {
        let answer = describe_insight(&emulator, domain_entity(name), &id, json!({})).await;
        refused(
            &answer,
            &format!("DescribeInsightDetails(DomainName {name:?})"),
            409,
            "ResourceNotFoundException",
        );
    }

    // A `DomainName` entity names its domain in `Value`, so one that names none
    // resolves to no entity rather than to every domain; and an `Account` entity's
    // `Value` is "the Amazon Web Services account ID", of which this emulator serves
    // exactly the one the request was made for.
    for entity in [
        json!({"Type": "DomainName"}),
        json!({"Type": "DomainName", "Value": null}),
        json!({"Type": "Account", "Value": "123456789012"}),
        json!({"Type": "Account", "Value": "210987654321"}),
    ] {
        assert_ne!(
            entity["Value"].as_str().unwrap_or_default(),
            engine::emulator_account_id(),
            "case DescribeInsightDetails({entity:?}): the row must not name the served account"
        );
        let answer = describe_insight(&emulator, entity.clone(), &id, json!({})).await;
        refused(
            &answer,
            &format!("DescribeInsightDetails({entity:?})"),
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn an_insight_the_service_raised_no_field_for_has_an_empty_detailed_view() {
    let emulator = Emulator::new();
    create_baseline(&emulator, "insight-detail-empty").await;
    let bare_id = guid("000000000003");
    raise_on_domain(
        &emulator,
        "insight-detail-empty",
        record(&bare_id, SystemTime::now(), Vec::new()),
    );

    // `Fields` is required, so an insight with no detail field is reported with an empty
    // list rather than refused or answered without the member.
    let answer = describe_insight(
        &emulator,
        domain_entity("insight-detail-empty"),
        &bare_id,
        json!({}),
    )
    .await;
    let reported = fields(&answer, "insight with no detail field");
    assert!(
        reported.is_empty(),
        "case insight with no detail field: expected no fields, got {:?}",
        answer.body["Fields"]
    );
}
