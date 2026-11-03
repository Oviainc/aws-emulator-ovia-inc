// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `ListTemplates`, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: a listing reports exactly the live resources its named
//! container holds, computed from persisted state, in one stable order, narrowed
//! by the filters the request carries, and cut into pages at `maxResults` with a
//! `nextToken` that resumes immediately after the last entry reported — so a
//! caller following tokens to exhaustion sees every matching resource exactly
//! once, and reading changes nothing a later caller sees.
//!
//! The rows span the class the rule generalises over: how many templates exist,
//! which `TemplateStatus` each carries, every subset of the status catalog as a
//! filter, every page size at and past the model's published bounds, and tokens
//! this service did and did not issue.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::ops::types::TEMPLATE_STATUS_VALUES;
use service_com_amazonaws_connectcases_amazonconnectcases::state::MAX_PAGE_SIZE;

// The crate's shared test harness: one emulator behind the production router,
// and the domain-creating helper every suite in this target uses.
use crate::harness::{domain, string_at, Emulator};

/// Create a template in `domain_id`, optionally with an explicit `status`, and
/// hand back its id.
async fn template(
    emulator: &Emulator,
    domain_id: &str,
    name: &str,
    status: Option<&str>,
) -> String {
    let mut body = json!({ "name": name });
    if let Some(status) = status {
        body["status"] = json!(status);
    }
    let created = emulator
        .ok(name, "POST", &format!("/domains/{domain_id}/templates"), body)
        .await;
    string_at(&created, "templateId", name)
}

/// `ListTemplates` for `domain_id` with `query` appended verbatim.
pub(crate) async fn list(emulator: &Emulator, case: &str, domain_id: &str, query: &str) -> Value {
    emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/templates-list{query}"),
            json!({}),
        )
        .await
}

/// `ListTemplates` that must be refused, as its status and wire error code.
async fn refused(emulator: &Emulator, case: &str, domain_id: &str, query: &str) -> (u16, String) {
    emulator
        .failure(
            case,
            "POST",
            &format!("/domains/{domain_id}/templates-list{query}"),
            Some(json!({})),
        )
        .await
}

/// `GetTemplate`, so a listed entry can be checked against the record a reader
/// of the single resource sees.
async fn get_template(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    template_id: &str,
) -> Value {
    emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            json!({}),
        )
        .await
}

/// The `templates` member of a listing answer, as the ids it reports in order.
pub(crate) fn listed_ids(case: &str, answer: &Value) -> Vec<String> {
    answer
        .get("templates")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a listing must carry templates: {answer:?}"))
        .iter()
        .map(|summary| string_at(summary, "templateId", case))
        .collect()
}

/// The `nextToken` a listing answer carries, if any.
fn next_token(answer: &Value) -> Option<String> {
    answer
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `status` query string for `statuses`: `status` is an `httpQuery` list, so
/// each value is its own parameter.
pub(crate) fn status_query(statuses: &[&str]) -> String {
    if statuses.is_empty() {
        String::new()
    } else {
        let joined: Vec<String> = statuses
            .iter()
            .map(|status| format!("status={status}"))
            .collect();
        format!("?{}", joined.join("&"))
    }
}

/// `count` templates in `domain_id`, each carrying the status of its row: the
/// catalog is cycled so every `TemplateStatus` the model declares is
/// represented, and every third template is created with no `status` at all —
/// which `CreateTemplate` stores as `Active`. Returns `(templateId, status)` in
/// creation order.
async fn templates_of_every_status(
    emulator: &Emulator,
    domain_id: &str,
    count: usize,
) -> Vec<(String, String)> {
    let mut created = Vec::new();
    for index in 0..count {
        let (requested, stored) = if index % 3 == 2 {
            // No `status` member: the default a template takes when it is not
            // given one.
            (None, "Active")
        } else {
            let status = TEMPLATE_STATUS_VALUES[index % TEMPLATE_STATUS_VALUES.len()];
            (Some(status), status)
        };
        let name = format!("template-{index:03}");
        let id = template(emulator, domain_id, &name, requested).await;
        created.push((id, stored.to_string()));
    }
    created
}

#[tokio::test]
async fn a_listing_reports_every_live_template_of_its_domain_in_a_stable_order() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "listing-domain").await;

    // An empty container reports an empty page and owes no token.
    let empty = list(&emulator, "no templates yet", &domain_id, "").await;
    assert!(
        listed_ids("no templates yet", &empty).is_empty(),
        "case \"no templates yet\": a domain holding no template must report none, got {empty:?}"
    );
    assert_eq!(
        next_token(&empty),
        None,
        "case \"no templates yet\": nothing remains, so no nextToken is owed, got {empty:?}"
    );

    let created = templates_of_every_status(&emulator, &domain_id, 7).await;
    let expected: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();

    let answer = list(&emulator, "every template", &domain_id, "").await;
    let listed = listed_ids("every template", &answer);
    assert_eq!(
        listed, expected,
        "case \"every template\": the listing must report every stored template in creation \
         order, got {listed:?}"
    );
    assert_eq!(
        next_token(&answer),
        None,
        "case \"every template\": one page holds them all, so no nextToken is owed, got {answer:?}"
    );

    // A read changes nothing a later caller sees: the same call answers the same
    // listing, in the same order.
    let again = list(&emulator, "every template, again", &domain_id, "").await;
    assert_eq!(
        listed_ids("every template, again", &again),
        listed,
        "case \"every template, again\": a listing is a read, so a second call must answer the \
         same order, got {again:?}"
    );

    // Each summary is computed from the stored record: its members are the ones
    // `GetTemplate` reports for the same template.
    let summaries = answer["templates"]
        .as_array()
        .expect("the listing carries templates")
        .clone();
    for (summary, (id, stored_status)) in summaries.iter().zip(created.iter()) {
        let case = format!("summary of {id}");
        let record = get_template(&emulator, &case, &domain_id, id).await;
        for member in ["templateId", "templateArn", "name", "status"] {
            let listed = string_at(summary, member, &case);
            let read = string_at(&record, member, &case);
            assert_eq!(
                listed, read,
                "case {case:?}: the summary's {member} must be the stored one GetTemplate \
                 reports ({read}), got {listed}"
            );
        }
        assert_eq!(
            string_at(summary, "status", &case),
            *stored_status,
            "case {case:?}: the summary must report the status the template was stored with"
        );
    }
}

#[tokio::test]
async fn a_status_filter_narrows_the_listing_to_the_statuses_it_names() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "filter-domain").await;
    let created = templates_of_every_status(&emulator, &domain_id, 8).await;

    // Every subset of the status catalog the model's `@length(1..2)` filter
    // admits, plus the absent filter. `TEMPLATE_STATUS_VALUES` is the model's own
    // `TemplateStatus` enum (smithy_query(query="shape", shape="TemplateStatus")).
    let mut filters: Vec<Vec<&str>> = vec![vec![]];
    for status in TEMPLATE_STATUS_VALUES {
        filters.push(vec![*status]);
    }
    filters.push(TEMPLATE_STATUS_VALUES.to_vec());
    // The same two values in the other order must select the same templates.
    let mut reversed = TEMPLATE_STATUS_VALUES.to_vec();
    reversed.reverse();
    filters.push(reversed);

    for filter in filters {
        let case = format!("status={filter:?}");
        let expected: Vec<String> = created
            .iter()
            .filter(|(_, status)| filter.is_empty() || filter.contains(&status.as_str()))
            .map(|(id, _)| id.clone())
            .collect();
        let answer = list(&emulator, &case, &domain_id, &status_query(&filter)).await;
        let listed = listed_ids(&case, &answer);
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing must report exactly the templates whose stored status \
             the filter names, got {listed:?}"
        );
    }

    // A value that is not a `TemplateStatus` cannot match a template, so it is a
    // bad request rather than an empty page.
    for unknown in ["Draft", "active", "ACTIVE", "Deleted"] {
        let case = format!("status={unknown}");
        let (status, code) =
            refused(&emulator, &case, &domain_id, &format!("?status={unknown}")).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a status that is not a TemplateStatus must be refused with a 400 \
             ValidationException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_page_is_cut_at_max_results_and_its_token_resumes_exactly_once() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "paging-domain").await;
    let created = templates_of_every_status(&emulator, &domain_id, 12).await;
    let expected: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();

    // Page sizes spanning the model's published `maxResults` range (1..100,
    // smithy_query(query="operation", operation="ListTemplates")): both bounds,
    // sizes inside it that do and do not divide the listing, and one that holds
    // everything.
    for page_size in [1, 2, 5, 7, 12, 13, MAX_PAGE_SIZE] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(value) => format!("?maxResults={page_size}&nextToken={value}"),
                None => format!("?maxResults={page_size}"),
            };
            let answer = list(&emulator, &case, &domain_id, &query).await;
            let page = listed_ids(&case, &answer);
            pages += 1;
            assert!(
                page.len() <= page_size as usize,
                "case {case:?} page {pages}: a page must hold at most maxResults entries, got {}",
                page.len()
            );
            token = next_token(&answer);
            if token.is_some() {
                assert_eq!(
                    page.len(),
                    page_size as usize,
                    "case {case:?} page {pages}: a nextToken may only arrive with a full page, \
                     got {} entries",
                    page.len()
                );
            }
            seen.extend(page);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: following nextToken must terminate, but page {pages} still \
                 carried one after {} entries",
                seen.len()
            );
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: following nextToken to exhaustion must report every template \
             exactly once, in one order, got {seen:?}"
        );
        let distinct: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            distinct.len(),
            seen.len(),
            "case {case:?}: no template may be reported twice across pages, got {seen:?}"
        );
    }

    // A page size outside the published range is refused rather than clamped
    // silently: one past each bound.
    for bad in [0, -1, MAX_PAGE_SIZE + 1] {
        let case = format!("maxResults={bad}");
        let (status, code) =
            refused(&emulator, &case, &domain_id, &format!("?maxResults={bad}")).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a maxResults outside 1..{MAX_PAGE_SIZE} must be refused with a 400 \
             ValidationException, got {status} {code}"
        );
    }

    // A token this service never issued is refused, rather than resumed from or
    // silently read as the start of the listing.
    let issued = next_token(&list(&emulator, "first page", &domain_id, "?maxResults=1").await)
        .expect("a page smaller than the listing carries a nextToken");
    for forged in [
        "not-a-token".to_string(),
        // Valid hex, but not a token this service minted.
        "68656c6c6f".to_string(),
        // The issued token with its first hex digit pair dropped: its version
        // marker no longer reads as this service's.
        issued[2..].to_string(),
        // The issued token one character short, so it is not even a byte string.
        issued[..issued.len() - 1].to_string(),
    ] {
        let case = format!("nextToken={forged}");
        let (status, code) =
            refused(&emulator, &case, &domain_id, &format!("?nextToken={forged}")).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a nextToken this service did not issue must be refused with a 400 \
             ValidationException, got {status} {code}"
        );
    }

    // An empty `nextToken` is a value the model's @length(0..9000) admits and
    // means the start of the listing.
    let from_empty = list(&emulator, "empty nextToken", &domain_id, "?nextToken=").await;
    assert_eq!(
        listed_ids("empty nextToken", &from_empty),
        expected,
        "case \"empty nextToken\": an empty token means the start of the listing, got \
         {from_empty:?}"
    );

    // A token belongs to the listing that issued it: presented to a listing
    // narrowed differently, it is refused rather than resumed from, because the
    // entry it names need not even be in that listing.
    let (status, code) = refused(
        &emulator,
        "a token issued for the unfiltered listing, presented with a filter",
        &domain_id,
        &format!("?status=Inactive&maxResults=1&nextToken={issued}"),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"a token issued for the unfiltered listing, presented with a filter\": a token \
         from another listing must be refused with a 400 ValidationException, got {status} {code}"
    );

    // And a token belongs to the container it was issued for: a second domain's
    // listing does not accept the first domain's cursor.
    let other_domain = domain(&emulator, "paging-domain-two").await;
    templates_of_every_status(&emulator, &other_domain, 4).await;
    let (status, code) = refused(
        &emulator,
        "a token issued for another domain",
        &other_domain,
        &format!("?maxResults=1&nextToken={issued}"),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"a token issued for another domain\": a token from another container must be \
         refused with a 400 ValidationException, got {status} {code}"
    );

    // The status filter and the page cut compose: paging a filtered listing
    // reports exactly the filtered templates.
    let filtered_expected: Vec<String> = created
        .iter()
        .filter(|(_, status)| status == "Inactive")
        .map(|(id, _)| id.clone())
        .collect();
    let mut filtered_seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let case = "filtered paging".to_string();
        let query = match &token {
            Some(value) => format!("?status=Inactive&maxResults=1&nextToken={value}"),
            None => "?status=Inactive&maxResults=1".to_string(),
        };
        let answer = list(&emulator, &case, &domain_id, &query).await;
        filtered_seen.extend(listed_ids(&case, &answer));
        token = next_token(&answer);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        filtered_seen, filtered_expected,
        "case \"filtered paging\": paging a filtered listing must report exactly the templates \
         the filter selects, got {filtered_seen:?}"
    );
}

#[tokio::test]
async fn a_listing_is_scoped_to_the_domain_it_names_and_refuses_one_that_does_not_exist() {
    let emulator = Emulator::new();
    let first = domain(&emulator, "first-domain").await;
    let second = domain(&emulator, "second-domain").await;

    let in_first = templates_of_every_status(&emulator, &first, 3).await;
    let in_second = templates_of_every_status(&emulator, &second, 2).await;

    for (case, domain_id, expected) in [
        ("first domain", &first, &in_first),
        ("second domain", &second, &in_second),
    ] {
        let answer = list(&emulator, case, domain_id, "").await;
        let listed = listed_ids(case, &answer);
        let expected: Vec<String> = expected.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: a listing reports only the templates of the domain it names, got \
             {listed:?}"
        );
    }

    // A domain the store does not hold is the modelled 404, not an empty page.
    let (status, code) = refused(&emulator, "unknown domain", "no-such-domain", "").await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"unknown domain\": listing a domain that does not exist must be refused with a \
         404 ResourceNotFoundException, got {status} {code}"
    );
}
