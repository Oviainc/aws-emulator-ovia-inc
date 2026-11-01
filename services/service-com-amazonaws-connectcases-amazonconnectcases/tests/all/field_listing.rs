// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `ListFields`, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: a listing reports exactly the live resources its named
//! container holds, computed from persisted state, in one stable order, cut into
//! pages at `maxResults` (and at the model's own cap when the request names
//! none) with a `nextToken` that resumes immediately after the last entry
//! reported — so a caller following tokens to exhaustion sees every resource
//! exactly once, and reading changes nothing a later caller sees. A container
//! this service does not hold is refused with `ResourceNotFoundException` (404),
//! and a page size outside the published range or a token this listing did not
//! issue with `ValidationException` (400).
//!
//! The rows span the class the rule generalises over: how many fields exist
//! (none, one, several, one more than a full default page), every page size at
//! and past the model's published bounds, every `FieldType` the model declares,
//! a field whose attributes are set and fields where they are absent, several
//! domains in one store, and tokens this service did and did not issue.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::ops::types::FIELD_TYPE_VALUES;
use service_com_amazonaws_connectcases_amazonconnectcases::state::{
    FIELD_NAMESPACE_CUSTOM, FIELD_NAMESPACE_SYSTEM, MAX_PAGE_SIZE, RESERVED_SYSTEM_FIELDS,
};

// The crate's shared test harness: one emulator behind the production router,
// and the domain-creating helper every suite in this target uses.
use crate::harness::{domain, string_at, Emulator};

/// `ListFields` for `domain_id` with `query` appended verbatim.
async fn list(emulator: &Emulator, case: &str, domain_id: &str, query: &str) -> Value {
    emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/fields-list{query}"),
            json!({}),
        )
        .await
}

/// `ListFields` that must be refused, as its status and wire error code.
async fn refused(emulator: &Emulator, case: &str, domain_id: &str, query: &str) -> (u16, String) {
    emulator
        .failure(
            case,
            "POST",
            &format!("/domains/{domain_id}/fields-list{query}"),
            Some(json!({})),
        )
        .await
}

/// The `fields` member of a listing answer.
fn listed(case: &str, answer: &Value) -> Vec<Value> {
    answer
        .get("fields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a listing must carry fields: {answer:?}"))
        .clone()
}

/// The ids a listing answer reports, in the order it reported them.
fn listed_ids(case: &str, answer: &Value) -> Vec<String> {
    listed(case, answer)
        .iter()
        .map(|summary| string_at(summary, "fieldId", case))
        .collect()
}

/// The `nextToken` a listing answer carries, if any.
fn next_token(answer: &Value) -> Option<String> {
    answer
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The query string for a page: `maxResults` and `nextToken` are `httpQuery`
/// members, and a token this service issues is hex, so it needs no escaping.
fn page_query(max_results: Option<i32>, token: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(size) = max_results {
        parts.push(format!("maxResults={size}"));
    }
    if let Some(token) = token {
        parts.push(format!("nextToken={token}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// Create `count` fields in `domain_id`, cycling the modelled field types, and
/// return the `FieldSummary` each one must be listed as, in creation order.
///
/// The expectation is built from what `CreateField` ANSWERED (the minted id and
/// ARN) and what the request asked for (name and type), so it is the persisted
/// record rather than a copy of the listing under test.
async fn create_fields(emulator: &Emulator, domain_id: &str, count: usize) -> Vec<(String, Value)> {
    let mut expected: Vec<(String, Value)> = Vec::with_capacity(count);
    for index in 0..count {
        // The type catalog is the crate's `FIELD_TYPE_VALUES`, generated from
        // the model's `FieldType` enum
        // (`smithy_query(query="shape", shape="FieldType")`).
        let field_type = FIELD_TYPE_VALUES[index % FIELD_TYPE_VALUES.len()];
        let name = format!("field-{index:04}-{field_type}");
        let created = emulator
            .ok(
                &name,
                "POST",
                &format!("/domains/{domain_id}/fields"),
                json!({ "name": name, "type": field_type }),
            )
            .await;
        let field_id = string_at(&created, "fieldId", &name);
        expected.push((
            field_id.clone(),
            json!({
                "fieldId": field_id,
                "fieldArn": string_at(&created, "fieldArn", &name),
                "name": name,
                "type": field_type,
                "namespace": FIELD_NAMESPACE_CUSTOM,
            }),
        ));
    }
    expected
}

/// The reserved `System` fields every domain is provisioned with at creation
/// (`state::RESERVED_SYSTEM_FIELDS`), as the summaries a listing must report for
/// them: they are fields of the domain like any other, so a listing reports them
/// too. They share the domain's creation instant, so the listing's order key
/// falls back to the id, which is what sorts them here.
fn reserved_summaries(domain_id: &str) -> Vec<(String, Value)> {
    let mut reserved: Vec<(String, Value)> = RESERVED_SYSTEM_FIELDS
        .iter()
        .map(|(field_id, name, field_type)| {
            (
                (*field_id).to_string(),
                json!({
                    "fieldId": field_id,
                    "fieldArn": format!(
                        "arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/field/{field_id}"
                    ),
                    "name": name,
                    "type": field_type,
                    "namespace": FIELD_NAMESPACE_SYSTEM,
                }),
            )
        })
        .collect();
    reserved.sort_by(|(left, _), (right, _)| left.cmp(right));
    reserved
}

/// The reserved ids a listing of `domain_id` reports, in that order.
fn reserved_ids(domain_id: &str) -> Vec<String> {
    reserved_summaries(domain_id)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// Every summary a listing of `domain_id` must report after `count` fields are
/// created in it: the reserved ones the domain was provisioned with, then the
/// created ones in creation order.
async fn expected_listing(
    emulator: &Emulator,
    domain_id: &str,
    count: usize,
) -> Vec<(String, Value)> {
    let mut expected = reserved_summaries(domain_id);
    expected.extend(create_fields(emulator, domain_id, count).await);
    expected
}

/// Follow the cursor to its end, returning every summary reported and how many
/// pages it took. Asserts the page-size bound and that a token only ever
/// arrives with a full page on the way.
async fn walk(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    max_results: Option<i32>,
    expected_entries: usize,
) -> (Vec<Value>, usize) {
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE) as usize;
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let answer = list(
            emulator,
            case,
            domain_id,
            &page_query(max_results, token.as_deref()),
        )
        .await;
        let page = listed(case, &answer);
        pages += 1;
        assert!(
            page.len() <= limit,
            "case {case:?} page {pages}: a page holds at most {limit} entries, got {}",
            page.len()
        );
        token = next_token(&answer);
        if token.is_some() {
            assert_eq!(
                page.len(),
                limit,
                "case {case:?} page {pages}: a nextToken may only arrive with a full page, \
                 got {} entries",
                page.len()
            );
        }
        collected.extend(page);
        if token.is_none() {
            break;
        }
        assert!(
            pages <= expected_entries + 1,
            "case {case:?}: following nextToken must terminate, but page {pages} still \
             carried one after {} entries",
            collected.len()
        );
    }
    (collected, pages)
}

#[tokio::test]
async fn list_fields_walks_every_stored_field_exactly_once() {
    // The rows differ along the two things the rule generalises over: how many
    // fields the domain holds — including none at all and one more than a full
    // default page — and the page size the walk asks for, spanning the model's
    // published `maxResults` range (1..100, per
    // `smithy_query(query="operation", operation="ListFields")`): both bounds,
    // sizes that do and do not divide the listing, and one that holds everything.
    let rows: Vec<(&str, usize, Option<i32>)> = vec![
        ("an empty domain, no page size", 0, None),
        ("an empty domain, one per page", 0, Some(1)),
        ("one field, one per page", 1, Some(1)),
        ("three fields, no page size", 3, None),
        ("seven fields, two per page", 7, Some(2)),
        ("twelve fields, five per page", 12, Some(5)),
        ("twelve fields, thirteen per page", 12, Some(13)),
        (
            "five fields, the largest page the model allows",
            5,
            Some(MAX_PAGE_SIZE),
        ),
        (
            "one field more than a default page holds",
            MAX_PAGE_SIZE as usize + 1,
            None,
        ),
    ];

    for (case, count, max_results) in rows {
        let emulator = Emulator::new();
        let domain_id = domain(&emulator, &format!("domain-of-{count}-fields")).await;
        let expected = expected_listing(&emulator, &domain_id, count).await;
        let expected_ids: Vec<String> = expected.iter().map(|(id, _)| id.clone()).collect();
        let total = expected.len();

        let (reported, pages) = walk(&emulator, case, &domain_id, max_results, total).await;
        let ids: Vec<String> = reported
            .iter()
            .map(|summary| string_at(summary, "fieldId", case))
            .collect();

        assert_eq!(
            ids, expected_ids,
            "case {case:?}: following nextToken to exhaustion must report every stored field \
             exactly once, in one order, got {ids:?}"
        );
        let distinct: BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            distinct.len(),
            ids.len(),
            "case {case:?}: no field may be reported twice across pages, got {ids:?}"
        );
        let limit = max_results.unwrap_or(MAX_PAGE_SIZE) as usize;
        let wanted_pages = total.div_ceil(limit).max(1);
        assert_eq!(
            pages, wanted_pages,
            "case {case:?}: {total} fields at {limit} per page is {wanted_pages} page(s), \
             took {pages}"
        );

        for ((field_id, summary), reported) in expected.iter().zip(reported.iter()) {
            assert_eq!(
                reported, summary,
                "case {case:?}: the summary of {field_id} must be the stored record, \
                 got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn list_fields_summarises_every_field_type_as_the_batch_read_does() {
    let case = "every modelled field type";
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "domain-of-every-field-type").await;

    // One field per entry of the model's `FieldType` catalog, plus a Text field
    // carrying the one modelled `FieldAttributes` variant
    // (`smithy_query(query="shape", shape="FieldAttributes,TextAttributes")`), so
    // a summary is checked on a field whose attributes are set as well as on
    // fields where they are absent.
    let mut requested: Vec<(String, Value)> = Vec::new();
    for field_type in FIELD_TYPE_VALUES {
        requested.push((
            format!("plain-{field_type}"),
            json!({
                "name": format!("plain-{field_type}"),
                "type": field_type,
                "description": format!("a {field_type} field"),
            }),
        ));
    }
    requested.push((
        "multiline-text".to_string(),
        json!({
            "name": "multiline-text",
            "type": "Text",
            "attributes": { "text": { "isMultiline": true } },
        }),
    ));

    let mut ids: Vec<String> = Vec::new();
    for (name, body) in &requested {
        let created = emulator
            .ok(
                name,
                "POST",
                &format!("/domains/{domain_id}/fields"),
                body.clone(),
            )
            .await;
        ids.push(string_at(&created, "fieldId", name));
    }

    let answer = list(&emulator, case, &domain_id, "").await;
    let summaries = listed(case, &answer);
    let mut expected_ids = reserved_ids(&domain_id);
    expected_ids.extend(ids.iter().cloned());
    assert_eq!(
        listed_ids(case, &answer),
        expected_ids,
        "case {case:?}: the reserved fields and every created field must be listed, in order, \
         got {answer:?}"
    );

    // The same fields read through `BatchGetField`: the listing's summary must be
    // the record that read reports, member for member.
    let batch = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/fields-batch"),
            json!({
                "fields": ids.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
            }),
        )
        .await;
    let read_back = batch
        .get("fields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: BatchGetField must report fields, got {batch:?}"))
        .clone();
    assert_eq!(
        read_back.len(),
        ids.len(),
        "case {case:?}: the batch read must answer for every field, got {batch:?}"
    );

    for record in &read_back {
        let field_id = string_at(record, "fieldId", case);
        let summary = summaries
            .iter()
            .find(|entry| entry.get("fieldId").and_then(Value::as_str) == Some(field_id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: field {field_id} must be listed, got {summaries:?}")
            });
        // `FieldSummary` carries exactly these members, and `description` is not
        // one of them (`smithy_query(query="shape", shape="FieldSummary")`).
        for member in [
            "fieldId",
            "fieldArn",
            "name",
            "type",
            "namespace",
            "attributes",
        ] {
            assert_eq!(
                summary.get(member),
                record.get(member),
                "case {case:?}: {member:?} of {field_id} must match the batch read, \
                 got {summary:?} against {record:?}"
            );
        }
        assert_eq!(
            summary.get("description"),
            None,
            "case {case:?}: a summary carries no description, got {summary:?}"
        );
        assert_eq!(
            summary.get("namespace").and_then(Value::as_str),
            Some(FIELD_NAMESPACE_CUSTOM),
            "case {case:?}: a field a caller created is in the Custom namespace, got {summary:?}"
        );
    }

    let multiline = summaries
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some("multiline-text"))
        .unwrap_or_else(|| panic!("case {case:?}: the multiline field must be listed"));
    assert_eq!(
        multiline.get("attributes"),
        Some(&json!({ "text": { "isMultiline": true } })),
        "case {case:?}: the summary must report the stored attributes, got {multiline:?}"
    );
}

#[tokio::test]
async fn list_fields_reports_only_the_fields_of_the_domain_it_names() {
    let case = "three domains in one store";
    let emulator = Emulator::new();
    let first = domain(&emulator, "first-domain").await;
    let second = domain(&emulator, "second-domain").await;
    let empty = domain(&emulator, "domain-with-no-fields").await;

    let in_first: Vec<String> = create_fields(&emulator, &first, 4)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let in_second: Vec<String> = create_fields(&emulator, &second, 6)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();

    for (domain_id, stored, size) in [
        (&first, in_first, Some(2)),
        (&second, in_second, None),
        (&empty, Vec::new(), Some(1)),
    ] {
        let mut expected = reserved_ids(domain_id);
        expected.extend(stored.iter().cloned());
        let (reported, _) = walk(&emulator, case, domain_id, size, expected.len()).await;
        let ids: Vec<String> = reported
            .iter()
            .map(|summary| string_at(summary, "fieldId", case))
            .collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: domain {domain_id} must list only its own fields, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn list_fields_refuses_an_unknown_domain_a_bad_page_size_and_a_foreign_cursor() {
    let emulator = Emulator::new();
    let listed_domain = domain(&emulator, "the-listed-domain").await;
    let other_domain = domain(&emulator, "the-other-domain").await;
    let stored: Vec<String> = create_fields(&emulator, &listed_domain, 3)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    // The whole listing: the reserved fields the domain carries, then the three
    // created here.
    let mut all_ids = reserved_ids(&listed_domain);
    all_ids.extend(stored.iter().cloned());
    create_fields(&emulator, &other_domain, 3).await;

    let (status, code) = refused(
        &emulator,
        "a domain this store does not hold",
        "no-such-domain",
        "",
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a domain this store does not hold\": expected the modelled 404, \
         got {status} {code}"
    );

    // A page size outside the published range is refused rather than clamped
    // silently: one past each bound of `MaxResults` (`range 1..100`).
    for bad in [0, -1, MAX_PAGE_SIZE + 1] {
        let case = format!("maxResults={bad}");
        let (status, code) = refused(
            &emulator,
            &case,
            &listed_domain,
            &format!("?maxResults={bad}"),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a maxResults outside 1..{MAX_PAGE_SIZE} must be refused with a 400 \
             ValidationException, got {status} {code}"
        );
    }

    // Tokens this listing did and did not issue. `ListTemplates` pages the same
    // domain's templates, so its cursor is a well-formed token of ANOTHER
    // listing; the other domain's own `ListFields` cursor is a well-formed token
    // of the same listing over another container. Both name a position this
    // listing's order never placed, so both are refused rather than resumed.
    let issued = next_token(&list(&emulator, "first page", &listed_domain, "?maxResults=1").await)
        .expect("a page smaller than the listing carries a nextToken");
    for name in ["template-one", "template-two"] {
        emulator
            .ok(
                name,
                "POST",
                &format!("/domains/{listed_domain}/templates"),
                json!({ "name": name }),
            )
            .await;
    }
    let from_templates = next_token(
        &emulator
            .ok(
                "ListTemplates first page",
                "POST",
                &format!("/domains/{listed_domain}/templates-list?maxResults=1"),
                json!({}),
            )
            .await,
    )
    .expect("a page smaller than the template listing carries a nextToken");
    let from_other_domain =
        next_token(&list(&emulator, "another domain's first page", &other_domain, "?maxResults=1").await)
            .expect("a page smaller than the listing carries a nextToken");

    for (case, forged) in [
        ("not a token at all", "not-a-token".to_string()),
        // Valid hex, but not a token this service minted.
        ("hex this service never minted", "68656c6c6f".to_string()),
        // The issued token with its first hex digit pair dropped: its version
        // marker no longer reads as this service's.
        ("the issued token, truncated at the front", issued[2..].to_string()),
        // The issued token one character short, so it is not even a byte string.
        (
            "the issued token, one character short",
            issued[..issued.len() - 1].to_string(),
        ),
        ("a cursor another listing of this domain issued", from_templates),
        ("a cursor this listing issued for another domain", from_other_domain),
    ] {
        let (status, code) = refused(
            &emulator,
            case,
            &listed_domain,
            &format!("?maxResults=1&nextToken={forged}"),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a nextToken this listing did not issue must be refused with a 400 \
             ValidationException, got {status} {code}"
        );
    }

    // An empty `nextToken` is a value the model's `NextToken` (`@length(0..9000)`)
    // admits and means the start of the listing.
    let from_empty = list(&emulator, "empty nextToken", &listed_domain, "?nextToken=").await;
    assert_eq!(
        listed_ids("empty nextToken", &from_empty),
        all_ids,
        "case \"empty nextToken\": an empty token means the start of the listing, \
         got {from_empty:?}"
    );

    // And the cursor this listing DID issue still resumes it, strictly after the
    // page it came from.
    let resumed = list(
        &emulator,
        "the listing's own cursor",
        &listed_domain,
        &format!("?maxResults=1&nextToken={issued}"),
    )
    .await;
    assert_eq!(
        listed_ids("the listing's own cursor", &resumed),
        all_ids[1..2].to_vec(),
        "case \"the listing's own cursor\": a cursor resumes after the page it came from, \
         got {resumed:?}"
    );
}

#[tokio::test]
async fn listing_fields_changes_nothing_a_later_caller_sees() {
    let case = "a listing is a read";
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "domain-read-twice").await;
    let stored: Vec<String> = create_fields(&emulator, &domain_id, 5)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let mut all_ids = reserved_ids(&domain_id);
    all_ids.extend(stored.iter().cloned());

    // A cursor is not consumed by being followed: the same page and the same
    // resumption answer the same entries however often they are replayed.
    let first = list(&emulator, case, &domain_id, "?maxResults=2").await;
    let again = list(&emulator, case, &domain_id, "?maxResults=2").await;
    assert_eq!(
        first, again,
        "case {case:?}: replaying a page answers the same page, got {again:?}"
    );
    let cursor = next_token(&first).expect("five fields at two per page carry a cursor");
    let resumed = list(
        &emulator,
        case,
        &domain_id,
        &format!("?maxResults=2&nextToken={cursor}"),
    )
    .await;
    let resumed_again = list(
        &emulator,
        case,
        &domain_id,
        &format!("?maxResults=2&nextToken={cursor}"),
    )
    .await;
    assert_eq!(
        resumed, resumed_again,
        "case {case:?}: replaying a cursor answers the same page, got {resumed_again:?}"
    );

    // And a second full walk — at a different page size — still reports exactly
    // the same fields in the same order, so nothing the first walk did moved or
    // dropped one.
    let (second_walk, second_pages) =
        walk(&emulator, case, &domain_id, Some(3), all_ids.len()).await;
    let ids: Vec<String> = second_walk
        .iter()
        .map(|summary| string_at(summary, "fieldId", case))
        .collect();
    assert_eq!(
        ids, all_ids,
        "case {case:?}: a listing must not change what a later listing sees, got {ids:?}"
    );
    let wanted_pages = all_ids.len().div_ceil(3);
    assert_eq!(
        second_pages, wanted_pages,
        "case {case:?}: {} fields take {wanted_pages} pages of three, took {second_pages}",
        all_ids.len()
    );
}
