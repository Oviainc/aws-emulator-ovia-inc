// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for SEARCHING THE RELATED ITEMS OF A WHOLE
//! DOMAIN, driven through the real `MultiServiceRouter` so every assertion goes
//! over the wire the SDK speaks.
//!
//! The rule under test: `SearchAllRelatedItems` reports the stored associations
//! of EVERY case of one domain and of no other domain, each projected into the
//! `RelatedItemContent` member its type names — carrying what the association
//! recorded, plus the values the reader's union derives from it (an SLA's target
//! instant, computed from the association and the minutes it was given, and its
//! status, which follows from whether the tracked case field has reached one of
//! the targeted values and when) — narrowed to the associations ANY of the
//! request's type filters admits, ordered by its sort terms in the order they
//! are given, and cut into pages of at most the requested size with an opaque
//! token for the rest.
//!
//! The tables below are the class, not one recorded call: their rows differ in
//! the content type, which case the association was made on, which part of a
//! filter clause is named, how many clauses there are, which property and
//! direction the answer is sorted by, and (for the page size the model bounds)
//! the value at each published bound and one past it.

use std::collections::HashMap;

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::ops::types::{
    ORDER_VALUES, RELATED_ITEM_TYPE_VALUES, SEARCH_ALL_RELATED_ITEMS_SORT_PROPERTY_VALUES,
};

use crate::harness::{
    domain, field_of_type, fields_of_every_type, is_minted_id, string_at, template, Emulator,
};

/// An Amazon Connect contact ARN: the external identifier a `Contact` item
/// links to, which belongs to another service and names no Cases resource.
const CONTACT_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/contact/c-1";

/// A second contact ARN, for the row that filters on a contact the domain holds
/// no association with.
const OTHER_CONTACT_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/contact/c-2";

/// An Amazon Connect attached-file ARN, as a `File` item links to one.
const FILE_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/file/f-1";

/// A second attached-file ARN, for the row that filters on a file the domain
/// holds no association with.
const OTHER_FILE_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/file/f-2";

/// An Amazon Connect user ARN, for the entity a request names as having
/// performed an association.
const AGENT_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1";

/// The channels an Amazon Connect contact can arrive on, as that service's own
/// model enumerates its `Channel` (`CHAT | EMAIL | TASK | VOICE`). Cases' own
/// `Channel` is a free-form string, so this catalog is only what the negative
/// `ContactFilter$channel` row draws the channels an association was NOT
/// recorded under from.
const CONNECT_CHANNELS: &[&str] = &["CHAT", "EMAIL", "TASK", "VOICE"];

/// The SLA target this suite's SLA association is written with, in minutes, and
/// the value its tracked field has to reach for it to be met.
const SLA_TARGET_MINUTES: i64 = 60;
const SLA_TARGET_VALUE: &str = "resolved";

/// Smallest and largest `maxResults` this search admits: `@range(1..25)`
/// (`smithy_query(query="operation", operation="SearchAllRelatedItems")`).
const PAGE_SIZE_BOUNDS: (i32, i32) = (1, 25);

/// The two members of `SearchAllRelatedItemsSortProperty` and the descending
/// `Order`, as the rows below name them
/// (`SEARCH_ALL_RELATED_ITEMS_SORT_PROPERTY_VALUES`, `ORDER_VALUES`).
const SORT_BY_CASE_ID: &str = "CaseId";
const ORDER_DESCENDING: &str = "Desc";

/// The `SearchAllRelatedItems` path of one domain.
fn search_path(domain_id: &str) -> String {
    format!("/domains/{domain_id}/related-items-search")
}

/// The `CreateRelatedItem` path of one case.
fn related_items_path(domain_id: &str, case_id: &str) -> String {
    format!("/domains/{domain_id}/cases/{case_id}/related-items/")
}

/// One association this suite made: how a failing assertion names it, the case
/// it was made on, the id the service minted for it, the type it declared, the
/// entity it named as having performed it, and the content a reader must report
/// for it — `None` where the reader's union carries derived members the write
/// never gave it (a contact's own properties, an SLA's target instant and
/// status), which get their own assertions.
struct Association {
    label: &'static str,
    case_id: String,
    related_item_id: String,
    item_type: &'static str,
    performed_by: Option<Value>,
    reported_content: Option<Value>,
}

/// The resources this suite searches over: one domain with three cases, two of
/// which carry associations, and the field ids a custom item writes values
/// against.
struct Fixture {
    domain_id: String,
    primary_case: String,
    secondary_case: String,
    linked_case: String,
    text_field: String,
    number_field: String,
    associations: Vec<Association>,
}

impl Fixture {
    /// The association `label` names.
    fn find(&self, label: &str) -> &Association {
        self.associations
            .iter()
            .find(|association| association.label == label)
            .unwrap_or_else(|| panic!("this suite made no {label:?} association"))
    }

    /// The ids of the associations `labels` names, in that order.
    fn ids(&self, labels: &[&str]) -> Vec<String> {
        labels
            .iter()
            .map(|label| self.find(label).related_item_id.clone())
            .collect()
    }

    /// Every association's id.
    fn all_ids(&self) -> Vec<String> {
        self.associations
            .iter()
            .map(|association| association.related_item_id.clone())
            .collect()
    }
}

/// Create a case of `domain_id` and return its id.
async fn create_case(emulator: &Emulator, domain_id: &str, template_id: &str, case: &str) -> String {
    let created = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({ "templateId": template_id, "fields": [] }),
        )
        .await;
    string_at(&created, "caseId", case)
}

/// Associate one item with a case and return the id the service minted.
async fn associate(
    emulator: &Emulator,
    domain_id: &str,
    case_id: &str,
    label: &'static str,
    item_type: &'static str,
    content: Value,
    performed_by: Option<Value>,
) -> String {
    let mut body = json!({ "type": item_type, "content": content });
    if let Some(performed_by) = performed_by {
        body["performedBy"] = performed_by;
    }
    let created = emulator
        .ok(
            label,
            "POST",
            &related_items_path(domain_id, case_id),
            body,
        )
        .await;
    let id = string_at(&created, "relatedItemId", label);
    assert!(
        is_minted_id(&id),
        "case {label:?}: the relatedItemId must be a server-minted opaque id, got {id:?}"
    );
    id
}

/// The domain this suite searches: one case carrying an association of every
/// modelled `RelatedItemType`, a second case carrying two more (so a search of
/// the whole domain spans cases), and a third case for a `ConnectCase` link to
/// point at.
async fn fixture(emulator: &Emulator, name: &str) -> Fixture {
    let domain_id = domain(emulator, name).await;
    let fields = fields_of_every_type(emulator, &domain_id).await;
    let text_field = field_of_type(&fields, "Text");
    let number_field = field_of_type(&fields, "Number");
    let template_id = template(emulator, &domain_id, &format!("{name}-template"), &[]).await;
    let primary_case = create_case(emulator, &domain_id, &template_id, "the primary case").await;
    let secondary_case =
        create_case(emulator, &domain_id, &template_id, "the secondary case").await;
    let linked_case = create_case(emulator, &domain_id, &template_id, "the linked case").await;

    let comment_content =
        json!({ "comment": { "body": "called the customer back", "contentType": "Text/Plain" } });
    let file_content = json!({ "file": { "fileArn": FILE_ARN } });
    let link_content = json!({ "connectCase": { "caseId": linked_case } });
    let custom_content = json!({
        "custom": {
            "fields": [
                { "id": text_field, "value": { "stringValue": "order-4711" } },
                { "id": number_field, "value": { "doubleValue": 12.5 } },
            ],
        },
    });
    let second_comment_content =
        json!({ "comment": { "body": "escalated to tier two", "contentType": "Text/Plain" } });
    let second_custom_content = json!({
        "custom": {
            "fields": [{ "id": text_field, "value": { "stringValue": "order-0815" } }],
        },
    });

    let mut associations: Vec<Association> = Vec::new();
    for (label, case_id, item_type, content, performed_by, reported) in [
        (
            "the comment on the primary case",
            primary_case.clone(),
            "Comment",
            comment_content.clone(),
            Some(json!({ "userArn": AGENT_ARN })),
            Some(comment_content),
        ),
        (
            "the contact on the primary case",
            primary_case.clone(),
            "Contact",
            json!({ "contact": { "contactArn": CONTACT_ARN } }),
            Some(json!({ "customEntity": "night-shift-bot" })),
            None,
        ),
        (
            "the file on the primary case",
            primary_case.clone(),
            "File",
            file_content.clone(),
            None,
            Some(file_content),
        ),
        (
            "the case link on the primary case",
            primary_case.clone(),
            "ConnectCase",
            link_content.clone(),
            None,
            Some(link_content),
        ),
        (
            "the custom item on the primary case",
            primary_case.clone(),
            "Custom",
            custom_content.clone(),
            Some(json!({ "userArn": AGENT_ARN })),
            Some(custom_content),
        ),
        (
            "the sla on the primary case",
            primary_case.clone(),
            "Sla",
            json!({
                "sla": {
                    "slaInputConfiguration": {
                        "name": "resolve before the target",
                        "type": "CaseField",
                        "fieldId": text_field,
                        "targetFieldValues": [{ "stringValue": SLA_TARGET_VALUE }],
                        "targetSlaMinutes": SLA_TARGET_MINUTES,
                    },
                },
            }),
            None,
            None,
        ),
        (
            "the comment on the secondary case",
            secondary_case.clone(),
            "Comment",
            second_comment_content.clone(),
            None,
            Some(second_comment_content),
        ),
        (
            "the custom item on the secondary case",
            secondary_case.clone(),
            "Custom",
            second_custom_content.clone(),
            None,
            Some(second_custom_content),
        ),
    ] {
        let related_item_id = associate(
            emulator,
            &domain_id,
            &case_id,
            label,
            item_type,
            content,
            performed_by.clone(),
        )
        .await;
        associations.push(Association {
            label,
            case_id,
            related_item_id,
            item_type,
            performed_by,
            reported_content: reported,
        });
    }

    Fixture {
        domain_id,
        primary_case,
        secondary_case,
        linked_case,
        text_field,
        number_field,
        associations,
    }
}

/// A second domain built exactly like [`fixture`]'s, so a search can be
/// asserted to report its associations and no other domain's.
async fn another_domain(emulator: &Emulator, name: &str) -> Fixture {
    fixture(emulator, name).await
}

/// One page of a search: the `relatedItems` list and the token that resumes
/// after it.
async fn search(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    body: Value,
) -> (Vec<Value>, Option<String>) {
    let response = emulator.ok(case, "POST", &search_path(domain_id), body).await;
    let items = response
        .get("relatedItems")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no relatedItems list in {response:?}"))
        .clone();
    let token = response
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (items, token)
}

/// Every association a search reports, following its cursor to the end.
async fn walk(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    body: &Value,
    max_results: Option<i32>,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut page_body = body.clone();
        if let Some(max_results) = max_results {
            page_body["maxResults"] = json!(max_results);
        }
        if let Some(token) = token.as_deref() {
            page_body["nextToken"] = json!(token);
        }
        let (items, next) = search(emulator, case, domain_id, page_body).await;
        if let Some(max_results) = max_results {
            assert!(
                items.len() <= max_results as usize,
                "case {case:?}: a page must not exceed the requested size, got {items:?}"
            );
        }
        collected.extend(items);
        token = next;
        if token.is_none() {
            break;
        }
    }
    collected
}

/// The ids a search reported, in the order it reported them.
fn reported_ids(items: &[Value], case: &str) -> Vec<String> {
    items
        .iter()
        .map(|item| string_at(item, "relatedItemId", case))
        .collect()
}

/// The reported association keyed by its id.
fn by_id(items: &[Value], case: &str) -> HashMap<String, Value> {
    items
        .iter()
        .map(|item| (string_at(item, "relatedItemId", case), item.clone()))
        .collect()
}

/// One reported instant, in the epoch seconds this emulator's `date-time`
/// members carry.
fn instant(item: &Value, member: &str, case: &str) -> f64 {
    item.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: no instant member {member:?} in {item:?}"))
}

#[tokio::test]
async fn every_association_of_the_domain_is_reported_with_the_content_it_was_written_with() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "all-related-items").await;
    // A second domain with its own association: a search is scoped to the
    // domain it addresses, so none of this one's items may appear in it.
    let other = another_domain(&emulator, "another-domain").await;

    let case = "every association of the domain";
    let (items, token) = search(&emulator, case, &fixture.domain_id, json!({})).await;
    assert_eq!(
        token, None,
        "case {case:?}: {} associations fit one page, got {items:?}",
        fixture.associations.len()
    );
    assert_eq!(
        items.len(),
        fixture.associations.len(),
        "case {case:?}: every association of every case of the domain must be reported, got \
         {items:?}"
    );

    let reported = by_id(&items, case);
    for other_item in other.all_ids() {
        assert!(
            !reported.contains_key(&other_item),
            "case {case:?}: the search must report no association of another domain, but \
             {other_item:?} appeared in {items:?}"
        );
    }

    for association in &fixture.associations {
        let label = association.label;
        let item = reported
            .get(&association.related_item_id)
            .unwrap_or_else(|| panic!("case {label:?}: not reported at all in {items:?}"));
        assert_eq!(
            string_at(item, "caseId", label),
            association.case_id,
            "case {label:?}: the association must name the case it was made on, got {item:?}"
        );
        assert_eq!(
            string_at(item, "type", label),
            association.item_type,
            "case {label:?}: the association must report the type it was written with, got \
             {item:?}"
        );
        assert_eq!(
            item.get("performedBy"),
            association.performed_by.as_ref(),
            "case {label:?}: the association must report the entity the request named as \
             having performed it, got {item:?}"
        );
        if let Some(expected) = association.reported_content.as_ref() {
            assert_eq!(
                item.get("content"),
                Some(expected),
                "case {label:?}: the content must be reported exactly as it was written, got \
                 {item:?}"
            );
        }
    }

    // The table covers the whole class: one association of every modelled
    // related-item type.
    for item_type in RELATED_ITEM_TYPE_VALUES {
        assert!(
            fixture
                .associations
                .iter()
                .any(|association| association.item_type == *item_type),
            "the fixture must carry an association of the {item_type} type"
        );
    }

    // A contact's own properties are not this service's state: the association
    // records the ARN it was written with and the instant it was made, and the
    // channel every contact association here is recorded under, which has to be
    // a `Channel` the model admits (`@length(1..100)`).
    let contact = fixture.find("the contact on the primary case");
    let item = &reported[&contact.related_item_id];
    let content = item
        .get("content")
        .and_then(|content| content.get("contact"))
        .unwrap_or_else(|| panic!("case {:?}: no contact content in {item:?}", contact.label));
    assert_eq!(
        string_at(content, "contactArn", contact.label),
        CONTACT_ARN,
        "case {:?}: the contact content must report the ARN it was written with, got \
         {content:?}",
        contact.label
    );
    let channel = string_at(content, "channel", contact.label);
    assert!(
        (1..=100).contains(&channel.chars().count()),
        "case {:?}: the reported channel must be a Channel the model admits, got {channel:?}",
        contact.label
    );
    assert_eq!(
        instant(content, "connectedToSystemTime", contact.label),
        instant(item, "associationTime", contact.label),
        "case {:?}: the only instant this service observes for the contact is the one the \
         association was made at, got {item:?}",
        contact.label
    );

    // An SLA reports the configuration it was written with, plus the two values
    // the reader derives: the target INSTANT, which is the association plus the
    // minutes the write named, and the status, which is `Active` while that
    // instant has not passed and the tracked field has not reached a target
    // value.
    let sla = fixture.find("the sla on the primary case");
    let item = &reported[&sla.related_item_id];
    let configuration = item
        .get("content")
        .and_then(|content| content.get("sla"))
        .and_then(|sla| sla.get("slaConfiguration"))
        .unwrap_or_else(|| panic!("case {:?}: no sla configuration in {item:?}", sla.label));
    assert_eq!(
        string_at(configuration, "name", sla.label),
        "resolve before the target",
        "case {:?}: the SLA must report the name it was written with, got {configuration:?}",
        sla.label
    );
    assert_eq!(
        string_at(configuration, "type", sla.label),
        "CaseField",
        "case {:?}: the SLA must report the type it was written with, got {configuration:?}",
        sla.label
    );
    assert_eq!(
        string_at(configuration, "fieldId", sla.label),
        fixture.text_field,
        "case {:?}: the SLA must report the field it tracks, got {configuration:?}",
        sla.label
    );
    assert_eq!(
        configuration.get("targetFieldValues"),
        Some(&json!([{ "stringValue": SLA_TARGET_VALUE }])),
        "case {:?}: the SLA must report the values it targets, got {configuration:?}",
        sla.label
    );
    assert!(
        (instant(configuration, "targetTime", sla.label)
            - instant(item, "associationTime", sla.label)
            - (SLA_TARGET_MINUTES * 60) as f64)
            .abs()
            < 0.001,
        "case {:?}: the target instant must be the association plus the minutes it was given, \
         got {item:?}",
        sla.label
    );
    assert_eq!(
        string_at(configuration, "status", sla.label),
        "Active",
        "case {:?}: an SLA whose target instant has not passed and whose field has reached no \
         target value is Active, got {configuration:?}",
        sla.label
    );
    assert_eq!(
        configuration.get("completionTime"),
        None,
        "case {:?}: an SLA that has reached no target value has no completion instant, got \
         {configuration:?}",
        sla.label
    );
}

#[tokio::test]
async fn a_filter_narrows_the_answer_to_the_associations_it_admits() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "filtered-related-items").await;
    let text_field = &fixture.text_field;
    let number_field = &fixture.number_field;
    // The channel the store recorded the contact association under, read back
    // rather than assumed, so the rows below filter on what the service reports.
    let (all, _) = search(&emulator, "the whole domain", &fixture.domain_id, json!({})).await;
    let contact_item = &by_id(&all, "the whole domain")
        [&fixture.find("the contact on the primary case").related_item_id];
    let recorded_channel = contact_item
        .get("content")
        .and_then(|content| content.get("contact"))
        .map(|contact| string_at(contact, "channel", "the recorded channel"))
        .expect("the contact association reports a contact content");
    let other_channels: Vec<&str> = CONNECT_CHANNELS
        .iter()
        .copied()
        .filter(|channel| *channel != recorded_channel)
        .collect();

    // One row per narrowing a `RelatedItemTypeFilter` can express: the clause,
    // and the associations it must admit.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter at all",
            json!([]),
            fixture.all_ids(),
        ),
        (
            "every comment",
            json!([{ "comment": {} }]),
            fixture.ids(&[
                "the comment on the primary case",
                "the comment on the secondary case",
            ]),
        ),
        (
            "every contact",
            json!([{ "contact": {} }]),
            fixture.ids(&["the contact on the primary case"]),
        ),
        (
            "a contact by its ARN",
            json!([{ "contact": { "contactArn": CONTACT_ARN } }]),
            fixture.ids(&["the contact on the primary case"]),
        ),
        (
            "a contact ARN the domain holds no association with",
            json!([{ "contact": { "contactArn": OTHER_CONTACT_ARN } }]),
            Vec::new(),
        ),
        (
            "a contact on the channel it was recorded under",
            json!([{ "contact": { "channel": [recorded_channel] } }]),
            fixture.ids(&["the contact on the primary case"]),
        ),
        (
            "a contact on channels it was not recorded under",
            json!([{ "contact": { "channel": other_channels } }]),
            Vec::new(),
        ),
        (
            "a file by its ARN",
            json!([{ "file": { "fileArn": FILE_ARN } }]),
            fixture.ids(&["the file on the primary case"]),
        ),
        (
            "a file ARN the domain holds no association with",
            json!([{ "file": { "fileArn": OTHER_FILE_ARN } }]),
            Vec::new(),
        ),
        (
            "a case link by the case it points at",
            json!([{ "connectCase": { "caseId": fixture.linked_case } }]),
            fixture.ids(&["the case link on the primary case"]),
        ),
        (
            "a case link to a case nothing links to",
            json!([{ "connectCase": { "caseId": fixture.secondary_case } }]),
            Vec::new(),
        ),
        (
            "an SLA by its name",
            json!([{ "sla": { "name": "resolve before the target" } }]),
            fixture.ids(&["the sla on the primary case"]),
        ),
        (
            "an SLA by another name",
            json!([{ "sla": { "name": "resolve by close of business" } }]),
            Vec::new(),
        ),
        (
            "an SLA by the status it is in",
            json!([{ "sla": { "status": "Active" } }]),
            fixture.ids(&["the sla on the primary case"]),
        ),
        (
            "an SLA by a status it is not in",
            json!([{ "sla": { "status": "Met" } }]),
            Vec::new(),
        ),
        (
            "a custom item by a field value it carries",
            json!([{
                "custom": {
                    "fields": {
                        "field": { "equalTo": { "id": text_field, "value": { "stringValue": "order-4711" } } },
                    },
                },
            }]),
            fixture.ids(&["the custom item on the primary case"]),
        ),
        (
            "a custom item by a field value nothing carries",
            json!([{
                "custom": {
                    "fields": {
                        "field": { "equalTo": { "id": text_field, "value": { "stringValue": "order-9999" } } },
                    },
                },
            }]),
            Vec::new(),
        ),
        (
            "custom items across cases by what a field value contains",
            json!([{
                "custom": {
                    "fields": {
                        "field": { "contains": { "id": text_field, "value": { "stringValue": "ORDER-" } } },
                    },
                },
            }]),
            fixture.ids(&[
                "the custom item on the primary case",
                "the custom item on the secondary case",
            ]),
        ),
        (
            "a custom item by a numeric comparison",
            json!([{
                "custom": {
                    "fields": {
                        "field": { "greaterThan": { "id": number_field, "value": { "doubleValue": 12.0 } } },
                    },
                },
            }]),
            fixture.ids(&["the custom item on the primary case"]),
        ),
        (
            "a custom item by the negation of a clause",
            json!([{
                "custom": {
                    "fields": {
                        "not": {
                            "field": { "equalTo": { "id": text_field, "value": { "stringValue": "order-4711" } } },
                        },
                    },
                },
            }]),
            fixture.ids(&["the custom item on the secondary case"]),
        ),
        (
            "a custom item by two clauses that must both hold",
            json!([{
                "custom": {
                    "fields": {
                        "andAll": [
                            { "field": { "equalTo": { "id": text_field, "value": { "stringValue": "order-4711" } } } },
                            { "field": { "lessThanOrEqualTo": { "id": number_field, "value": { "doubleValue": 12.5 } } } },
                        ],
                    },
                },
            }]),
            fixture.ids(&["the custom item on the primary case"]),
        ),
        (
            "a custom item by either of two clauses",
            json!([{
                "custom": {
                    "fields": {
                        "orAll": [
                            { "field": { "equalTo": { "id": text_field, "value": { "stringValue": "order-4711" } } } },
                            { "field": { "equalTo": { "id": text_field, "value": { "stringValue": "order-0815" } } } },
                        ],
                    },
                },
            }]),
            fixture.ids(&[
                "the custom item on the primary case",
                "the custom item on the secondary case",
            ]),
        ),
        (
            "a custom item by a field it holds no value for",
            json!([{
                "custom": {
                    "fields": {
                        "field": { "equalTo": { "id": number_field, "value": { "emptyValue": {} } } },
                    },
                },
            }]),
            fixture.ids(&["the custom item on the secondary case"]),
        ),
        (
            "two clauses of different types, which hold as an OR",
            json!([{ "comment": {} }, { "file": { "fileArn": FILE_ARN } }]),
            fixture.ids(&[
                "the comment on the primary case",
                "the comment on the secondary case",
                "the file on the primary case",
            ]),
        ),
        (
            "one clause per modelled type",
            json!([
                { "comment": {} },
                { "contact": {} },
                { "file": {} },
                { "sla": {} },
                { "connectCase": {} },
                { "custom": {} },
            ]),
            fixture.all_ids(),
        ),
    ];

    for (case, filters, expected) in rows {
        let (items, _) = search(
            &emulator,
            case,
            &fixture.domain_id,
            json!({ "filters": filters }),
        )
        .await;
        let mut reported = reported_ids(&items, case);
        reported.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the associations the filter admits, got {items:?}"
        );
    }
}

#[tokio::test]
async fn an_sla_is_met_once_its_tracked_field_reaches_a_targeted_value() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "sla-status").await;
    let sla = fixture.find("the sla on the primary case");

    // Writing the targeted value into the case the SLA tracks is what reaches
    // its target, so the SAME SLA now reports itself met, with the instant that
    // edit was recorded at as its completion.
    let case = "the tracked field reaches the targeted value";
    emulator
        .ok(
            case,
            "PUT",
            &format!(
                "/domains/{}/cases/{}",
                fixture.domain_id, fixture.primary_case
            ),
            json!({
                "fields": [
                    { "id": fixture.text_field, "value": { "stringValue": SLA_TARGET_VALUE } },
                ],
            }),
        )
        .await;

    let (items, _) = search(
        &emulator,
        case,
        &fixture.domain_id,
        json!({ "filters": [{ "sla": { "status": "Met" } }] }),
    )
    .await;
    assert_eq!(
        reported_ids(&items, case),
        vec![sla.related_item_id.clone()],
        "case {case:?}: an SLA whose tracked field has reached a targeted value before its \
         target instant is Met, got {items:?}"
    );
    let configuration = items[0]
        .get("content")
        .and_then(|content| content.get("sla"))
        .and_then(|sla| sla.get("slaConfiguration"))
        .unwrap_or_else(|| panic!("case {case:?}: no sla configuration in {items:?}"));
    let completion = instant(configuration, "completionTime", case);
    let association = instant(&items[0], "associationTime", case);
    let target = instant(configuration, "targetTime", case);
    assert!(
        (association..=target).contains(&completion),
        "case {case:?}: the completion instant must be when the edit was recorded, between the \
         association and the target, got {configuration:?}"
    );

    let case = "the status the SLA left";
    let (items, _) = search(
        &emulator,
        case,
        &fixture.domain_id,
        json!({ "filters": [{ "sla": { "status": "Active" } }] }),
    )
    .await;
    assert!(
        items.is_empty(),
        "case {case:?}: an SLA that has reached its target is no longer Active, got {items:?}"
    );
}

#[tokio::test]
async fn the_sort_terms_order_the_answer_in_the_order_they_are_given() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "sorted-related-items").await;

    // Every value of each sort enum is a row, and the last row is two terms, so
    // the precedence of the first over the second is asserted too.
    for property in SEARCH_ALL_RELATED_ITEMS_SORT_PROPERTY_VALUES {
        for order in ORDER_VALUES {
            let case = format!("sorted by {property} {order}");
            let (items, _) = search(
                &emulator,
                &case,
                &fixture.domain_id,
                json!({ "sorts": [{ "sortProperty": property, "sortOrder": order }] }),
            )
            .await;
            assert_eq!(
                items.len(),
                fixture.associations.len(),
                "case {case:?}: sorting changes the order of the answer, not what is in it, \
                 got {items:?}"
            );
            let keys: Vec<String> = items
                .iter()
                .map(|item| {
                    if *property == SORT_BY_CASE_ID {
                        string_at(item, "caseId", &case)
                    } else {
                        // One key form for both properties, so the row asserts
                        // the same way whichever it sorts on: an instant in
                        // epoch seconds, zero-padded so lexicographic order over
                        // the key is chronological order.
                        format!("{:020.6}", instant(item, "associationTime", &case))
                    }
                })
                .collect();
            let mut expected = keys.clone();
            expected.sort();
            if *order == ORDER_DESCENDING {
                expected.reverse();
            }
            assert_eq!(
                keys, expected,
                "case {case:?}: the answer must be ordered by the sort term, got {items:?}"
            );
        }
    }

    let case = "sorted by case then by association time, newest first";
    let (items, _) = search(
        &emulator,
        case,
        &fixture.domain_id,
        json!({
            "sorts": [
                { "sortProperty": "CaseId", "sortOrder": "Asc" },
                { "sortProperty": "AssociationTime", "sortOrder": "Desc" },
            ],
        }),
    )
    .await;
    let keyed: Vec<(String, f64)> = items
        .iter()
        .map(|item| {
            (
                string_at(item, "caseId", case),
                instant(item, "associationTime", case),
            )
        })
        .collect();
    let mut expected = keyed.clone();
    expected.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.total_cmp(&left.1)));
    assert_eq!(
        keyed, expected,
        "case {case:?}: the first sort term must take precedence over the second, got {items:?}"
    );
}

#[tokio::test]
async fn a_page_holds_at_most_the_requested_size_and_resumes_where_it_left_off() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "paged-related-items").await;
    let body = json!({ "sorts": [{ "sortProperty": "AssociationTime", "sortOrder": "Asc" }] });

    let case = "the whole answer in one page";
    let whole = walk(&emulator, case, &fixture.domain_id, &body, None).await;
    let expected = reported_ids(&whole, case);
    assert_eq!(
        expected.len(),
        fixture.associations.len(),
        "case {case:?}: every association must be reported, got {whole:?}"
    );

    // One row per page size spanning the class: each published bound, and the
    // sizes in between that cut the answer into two or more pages.
    for size in [PAGE_SIZE_BOUNDS.0, 2, 3, 7, PAGE_SIZE_BOUNDS.1] {
        let case = format!("paged at {size}");
        let walked = walk(&emulator, &case, &fixture.domain_id, &body, Some(size)).await;
        assert_eq!(
            reported_ids(&walked, &case),
            expected,
            "case {case:?}: following the cursor must report every association exactly once, \
             in the same order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_search_refuses_what_it_cannot_honour() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "refused-related-items").await;
    let (_, token) = search(
        &emulator,
        "a cursor into this search",
        &fixture.domain_id,
        json!({ "maxResults": 1 }),
    )
    .await;
    let issued = token.expect("a search cut at one entry hands out a cursor");

    // One row per request a search cannot answer, with the status and code it
    // must be refused with.
    let rows: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "a domain this store does not hold",
            search_path("00000000-0000-4000-8000-000000000000"),
            json!({}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a page size below the published minimum",
            search_path(&fixture.domain_id),
            json!({ "maxResults": PAGE_SIZE_BOUNDS.0 - 1 }),
            400,
            "ValidationException",
        ),
        (
            "a page size above the published maximum",
            search_path(&fixture.domain_id),
            json!({ "maxResults": PAGE_SIZE_BOUNDS.1 + 1 }),
            400,
            "ValidationException",
        ),
        (
            "a sort property the model does not declare",
            search_path(&fixture.domain_id),
            json!({ "sorts": [{ "sortProperty": "AssociationDate", "sortOrder": "Asc" }] }),
            400,
            "ValidationException",
        ),
        (
            "a sort order the model does not declare",
            search_path(&fixture.domain_id),
            json!({ "sorts": [{ "sortProperty": "CaseId", "sortOrder": "Ascending" }] }),
            400,
            "ValidationException",
        ),
        (
            "a custom filter on a field the domain holds no record of",
            search_path(&fixture.domain_id),
            json!({
                "filters": [{
                    "custom": {
                        "fields": {
                            "field": {
                                "equalTo": {
                                    "id": "no-such-field",
                                    "value": { "stringValue": "x" },
                                },
                            },
                        },
                    },
                }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a cursor this search never issued",
            search_path(&fixture.domain_id),
            json!({ "nextToken": "not-a-token-this-service-issued" }),
            400,
            "ValidationException",
        ),
        (
            "a cursor issued for another domain's search",
            search_path(&fixture.domain_id),
            json!({ "nextToken": issued, "filters": [{ "comment": {} }] }),
            400,
            "ValidationException",
        ),
    ];

    for (case, path, body, status, code) in rows {
        let (actual_status, actual_code, message) = emulator
            .failure_detail(case, "POST", &path, Some(body))
            .await;
        assert_eq!(
            (actual_status, actual_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {actual_status} {actual_code}: \
             {message}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: a refusal must carry a message, got {actual_code}"
        );
    }

    // Each published page-size bound is honoured, so the refusals above are of
    // the sizes outside the range and not of the range itself.
    for size in [PAGE_SIZE_BOUNDS.0, PAGE_SIZE_BOUNDS.1] {
        let case = format!("a page size of {size}");
        let (items, _) = search(
            &emulator,
            &case,
            &fixture.domain_id,
            json!({ "maxResults": size }),
        )
        .await;
        assert!(
            !items.is_empty() && items.len() <= size as usize,
            "case {case:?}: the published bound must be honoured, got {items:?}"
        );
    }
}
