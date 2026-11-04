// SPDX-License-Identifier: CC0-1.0

//! `ListDataGrants` — the listing of the data grants this account SENT.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the status
//! code, the boundary validator and the serialized body are all exercised. The harness
//! (`emulator`, `harness`, `call`, `create_data_set`, `ACCOUNT`, `REGION`) is the one
//! `data_sets.rs` already uses, and the grant-plane calls (`create_data_grant`,
//! `get_data_grant`, `accept_data_grant`, `delete_data_grant`, `message`) are the ones
//! `data_grants.rs` already sends.
//!
//! Four rules are under test.
//!
//! 1. The listing REPORTS the data grants state holds: every grant this account created
//!    appears exactly once, in creation order, with each member projected from the stored
//!    record — the same record the single-grant reader answers with — nothing invented for
//!    one the record leaves absent, and nothing carried that the summary does not model. A
//!    grant whose state has since moved on (accepted) is listed in the state it is in now,
//!    and a deleted one is gone from the listing.
//! 2. Which listing a grant appears in follows from its sender: a grant another account
//!    sent to this one is RECEIVED, not sent, and `ListReceivedDataGrants` /
//!    `GetReceivedDataGrant` are the operations that report it, so `ListDataGrants` does
//!    not.
//! 3. It pages like every other `@paginated` listing of this service: a page carries at
//!    most `maxResults` entries, the `NextToken` resumes exactly after the last one, the
//!    final page carries no token, a page size outside the modelled range is refused, and
//!    a token this caller was not issued is the modelled 400 `ValidationException`.
//! 4. It is reporting-only: listing changes nothing a later caller sees.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_grants::{
    accept_data_grant, create_data_grant, delete_data_grant, get_data_grant, message,
};
use crate::data_sets::{call, create_data_set, emulator, harness, Answer, ACCOUNT, REGION};

/// Another account, as the modelled `SenderPrincipal` / `ReceiverPrincipal` pattern
/// `^\d{12}$` spells one. Never this emulator's own account (`ACCOUNT`, the account the
/// harness signs as and so the sender of every grant it creates).
const OTHER_ACCOUNT: &str = "111122223333";

/// The published bounds of `com.amazonaws.dataexchange#MaxResults`
/// (`@range(min: 1, max: 200)`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 200;

/// The modelled bounds of `com.amazonaws.dataexchange#DataGrantName`
/// (`@length(min: 1, max: 256)`).
const MIN_NAME_CHARS: usize = 1;
const MAX_NAME_CHARS: usize = 256;

/// Every member the model gives `DataGrantSummaryEntry`
/// (`smithy_query(query="shape", shape="DataGrantSummaryEntry")`), which is exactly what a
/// listed entry has to carry.
const SUMMARY_MEMBERS: &[&str] = &[
    "Name",
    "SenderPrincipal",
    "ReceiverPrincipal",
    "AcceptanceState",
    "AcceptedAt",
    "EndsAt",
    "DataSetId",
    "SourceDataSetId",
    "Id",
    "Arn",
    "CreatedAt",
    "UpdatedAt",
];

/// Members of the grant itself that `DataGrantSummaryEntry` does NOT model, from the same
/// shape: the projection must not carry them.
const MEMBERS_NOT_IN_THE_SUMMARY: &[&str] = &["GrantDistributionScope", "Description", "Tags"];

/// The refusal a `NextToken` this caller was not issued earns, worded by
/// `crate::state::invalid_next_token`.
const INVALID_NEXT_TOKEN: &str =
    "Invalid NextToken. Account Id does not match Caller's Account Id.";

/// One grant to create, differing from its neighbours along what the projection
/// generalises over: the receiver, the distribution scope, whether access ends, whether a
/// description or tags are carried, and how long the name is.
struct GrantCase {
    case: &'static str,
    name: String,
    receiver: &'static str,
    scope: &'static str,
    ends_at: Option<&'static str>,
    description: Option<&'static str>,
    tags: Option<Value>,
}

/// The grants these tests create. Both values the model's `GrantDistributionScope` enum
/// admits are covered (`smithy_query(query="shape", shape="GrantDistributionScope")` ->
/// `["AWS_ORGANIZATION", "NONE"]`).
fn grant_cases() -> Vec<GrantCase> {
    vec![
        GrantCase {
            case: "open-ended, no description",
            name: "open-ended".to_string(),
            receiver: OTHER_ACCOUNT,
            scope: "NONE",
            ends_at: None,
            description: None,
            tags: None,
        },
        GrantCase {
            case: "ends at a named instant, organization-wide",
            name: "ending".to_string(),
            receiver: OTHER_ACCOUNT,
            scope: "AWS_ORGANIZATION",
            ends_at: Some("2031-03-04T05:06:07Z"),
            description: Some("shared until the end of the quarter"),
            tags: None,
        },
        GrantCase {
            case: "name at the modelled minimum length",
            name: "n".repeat(MIN_NAME_CHARS),
            receiver: "222233334444",
            scope: "NONE",
            ends_at: Some("2030-01-01T00:00:00.500Z"),
            description: None,
            tags: Some(json!({ "team": "data" })),
        },
        GrantCase {
            case: "name at the modelled maximum length",
            name: "n".repeat(MAX_NAME_CHARS),
            receiver: "333344445555",
            scope: "AWS_ORGANIZATION",
            ends_at: None,
            description: Some("d"),
            tags: None,
        },
        GrantCase {
            case: "receiver is this account",
            name: "to-self".to_string(),
            receiver: ACCOUNT,
            scope: "NONE",
            ends_at: Some("2040-12-31T23:59:59Z"),
            description: None,
            tags: None,
        },
    ]
}

/// Create one grant over its own source data set and hand back the create's answer, which
/// carries every member the stored record holds.
async fn create(router: &MultiServiceRouter, spec: &GrantCase) -> Answer {
    let data_set_id = create_data_set(router, spec.case, "g", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let mut body = json!({
        "Name": spec.name,
        "GrantDistributionScope": spec.scope,
        "ReceiverPrincipal": spec.receiver,
        "SourceDataSetId": data_set_id,
    });
    if let Some(ends_at) = spec.ends_at {
        body["EndsAt"] = json!(ends_at);
    }
    if let Some(description) = spec.description {
        body["Description"] = json!(description);
    }
    if let Some(tags) = &spec.tags {
        body["Tags"] = tags.clone();
    }

    let created = create_data_grant(router, body).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected 201 from CreateDataGrant, got {} body {:?}",
        spec.case, created.status, created.body
    );
    created
}

/// Create every case and hand back the answers, in the order the grants were created.
async fn created_grants(router: &MultiServiceRouter) -> Vec<(&'static str, Answer)> {
    let mut created: Vec<(&'static str, Answer)> = Vec::new();
    for spec in grant_cases() {
        created.push((spec.case, create(router, &spec).await));
    }
    created
}

/// `?maxResults=<size>` when a page size is asked for, with an optional continuation.
fn uri(max_results: Option<i64>, next_token: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(size) = max_results {
        query.push(format!("maxResults={size}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    if query.is_empty() {
        "/v1/data-grants".to_string()
    } else {
        format!("/v1/data-grants?{}", query.join("&"))
    }
}

/// Send `ListDataGrants`.
async fn list(router: &MultiServiceRouter, max_results: Option<i64>, token: Option<&str>) -> Answer {
    let answer = call(router, "GET", &uri(max_results, token), None).await;
    assert_eq!(
        answer.status, 200,
        "case list(maxResults={max_results:?}, nextToken={token:?}): expected 200, got {} \
         body {:?}",
        answer.status, answer.body
    );
    answer
}

/// The `DataGrantSummaries` of an answer.
fn summaries(answer: &Answer) -> Vec<Value> {
    answer
        .member("DataGrantSummaries")
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("DataGrantSummaries is not a list: {:?}", answer.body))
}

/// The `Id` of every entry of a page, in the order the page carries them.
fn ids(answer: &Answer) -> Vec<String> {
    summaries(answer)
        .iter()
        .map(|entry| {
            entry["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry carries no Id: {entry:?}"))
                .to_string()
        })
        .collect()
}

/// The listed entry for one grant, or `None` when the listing does not carry it.
fn entry(answer: &Answer, grant_id: &str) -> Option<Value> {
    summaries(answer)
        .into_iter()
        .find(|entry| entry["Id"].as_str() == Some(grant_id))
}

/// Every modelled member of a listed entry, against the answer of a reader of the same
/// record (the create, the single-grant read, the accept).
fn assert_projects(case: &str, listed: &Value, reader: &Answer) {
    for member in SUMMARY_MEMBERS {
        assert_eq!(
            &listed[member],
            reader.member(member),
            "case {case}: expected the listed {member} to be the stored record's {:?}, got \
             {:?}",
            reader.member(member),
            listed[member]
        );
    }
    for member in MEMBERS_NOT_IN_THE_SUMMARY {
        assert!(
            listed[member].is_null(),
            "case {case}: expected the summary entry to omit {member}, got {:?}",
            listed[member]
        );
    }
}

/// RULE 1: every created grant is listed exactly once, in creation order, with each member
/// projected from the stored record — the same record `GetDataGrant` answers with.
#[tokio::test]
async fn every_created_data_grant_is_listed_from_the_stored_record() {
    let router = emulator();
    let created = created_grants(&router).await;

    let listed = list(&router, None, None).await;
    assert_eq!(
        ids(&listed),
        created
            .iter()
            .map(|(_, answer)| answer.string("Id"))
            .collect::<Vec<String>>(),
        "case the whole listing: expected every created grant exactly once, in creation \
         order"
    );

    for (case, answer) in &created {
        let grant_id = answer.string("Id");
        let listed_entry = entry(&listed, &grant_id)
            .unwrap_or_else(|| panic!("case {case}: the listing carries no entry for {grant_id}"));
        assert_projects(case, &listed_entry, answer);
        // The single-grant reader and the listing report ONE record, so they cannot differ.
        let fetched = get_data_grant(&router, &grant_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected 200 from GetDataGrant, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert_projects(case, &listed_entry, &fetched);
    }

    assert!(
        listed.member("NextToken").is_null(),
        "case the whole listing: expected no NextToken on a page that exhausted the \
         collection, got {:?}",
        listed.member("NextToken")
    );
}

/// RULE 1: the listing reports the state a grant is in NOW — an accepted grant is listed
/// `ACCEPTED` with the `AcceptedAt` the accept stamped, and a deleted grant is not listed
/// at all.
#[tokio::test]
async fn the_listing_follows_a_grant_through_its_lifecycle() {
    let router = emulator();
    // Rows differ along what happens to the grant after it is created: nothing, an accept
    // by the account it was made out to, and a delete.
    let untouched = create(
        &router,
        &GrantCase {
            case: "untouched",
            name: "untouched".to_string(),
            receiver: OTHER_ACCOUNT,
            scope: "NONE",
            ends_at: None,
            description: None,
            tags: None,
        },
    )
    .await;
    let accepted = create(
        &router,
        &GrantCase {
            case: "accepted",
            name: "accepted".to_string(),
            receiver: ACCOUNT,
            scope: "AWS_ORGANIZATION",
            ends_at: Some("2033-06-01T00:00:00Z"),
            description: Some("accepted by its receiver"),
            tags: None,
        },
    )
    .await;
    let deleted = create(
        &router,
        &GrantCase {
            case: "deleted",
            name: "deleted".to_string(),
            receiver: OTHER_ACCOUNT,
            scope: "NONE",
            ends_at: None,
            description: None,
            tags: None,
        },
    )
    .await;

    let before = list(&router, None, None).await;
    let pending = entry(&before, &accepted.string("Id")).expect("the created grant is listed");
    assert_projects("accepted, before the accept", &pending, &accepted);
    assert!(
        pending["AcceptedAt"].is_null(),
        "case accepted, before the accept: expected no AcceptedAt on a pending grant, got \
         {:?}",
        pending["AcceptedAt"]
    );

    let answer = accept_data_grant(&router, &accepted.string("Arn")).await;
    assert_eq!(
        answer.status, 200,
        "case accepted: expected 200 from AcceptDataGrant, got {} body {:?}",
        answer.status, answer.body
    );
    let removed = delete_data_grant(&router, &deleted.string("Id")).await;
    assert_eq!(
        removed.status, 204,
        "case deleted: expected 204 from DeleteDataGrant, got {} body {:?}",
        removed.status, removed.body
    );

    let after = list(&router, None, None).await;
    assert_eq!(
        ids(&after),
        vec![untouched.string("Id"), accepted.string("Id")],
        "case after the lifecycle: expected the deleted grant to be gone and the others to \
         keep their order"
    );
    let now_accepted =
        entry(&after, &accepted.string("Id")).expect("the accepted grant is still listed");
    // `AcceptDataGrantResponse` models no `SourceDataSetId`, so the whole record is read
    // back through the single-grant reader, and the accept's own answer is compared on the
    // two members it is about.
    let reread = get_data_grant(&router, &accepted.string("Id")).await;
    assert_projects("accepted, after the accept", &now_accepted, &reread);
    for member in ["AcceptanceState", "AcceptedAt"] {
        assert_eq!(
            &now_accepted[member],
            answer.member(member),
            "case accepted, after the accept: expected the listed {member} to be what the \
             accept answered ({:?}), got {:?}",
            answer.member(member),
            now_accepted[member]
        );
    }
    assert!(
        !now_accepted["AcceptedAt"].is_null(),
        "case accepted, after the accept: expected the AcceptedAt the accept stamped, got \
         {:?}",
        now_accepted["AcceptedAt"]
    );
    assert_eq!(
        entry(&after, &untouched.string("Id")).map(|listed| listed["AcceptanceState"].clone()),
        Some(untouched.member("AcceptanceState").clone()),
        "case untouched: expected the other grant's state to be untouched by the accept"
    );
}

/// RULE 2: a grant another account SENT to this one is not part of this listing — it is a
/// received grant, which the model gives its own operations.
#[tokio::test]
async fn a_grant_this_account_did_not_send_is_not_listed() {
    let (router, engine) = harness();
    let sent = create(
        &router,
        &GrantCase {
            case: "sent by this account",
            name: "sent".to_string(),
            receiver: OTHER_ACCOUNT,
            scope: "NONE",
            ends_at: None,
            description: None,
            tags: None,
        },
    )
    .await;
    // No operation this emulator implements mints a grant whose sender is another account
    // (AWS mints one when that account calls CreateDataGrant), so the record is written
    // through the shared engine, the way `event_actions.rs` writes an entitled data set.
    let received_id = seed_received_grant(&engine);

    let listed = list(&router, None, None).await;
    assert_eq!(
        ids(&listed),
        vec![sent.string("Id")],
        "case a received grant: expected only the grant this account sent, got {:?}",
        ids(&listed)
    );
    assert!(
        entry(&listed, &received_id).is_none(),
        "case a received grant: expected {received_id} to be absent from the sent listing"
    );
}

/// Write one grant SENT BY ANOTHER ACCOUNT into the shared store, and hand back its id.
fn seed_received_grant(engine: &Arc<engine::InMemoryEngine>) -> String {
    let created_at = dataexchange::state::now();
    let id = dataexchange::state::mint_id();
    let record = dataexchange::state::DataGrant {
        arn: dataexchange::state::data_grant_arn(REGION, &id),
        id: id.clone(),
        name: "received-not-sent".to_string(),
        description: None,
        sender_principal: OTHER_ACCOUNT.to_string(),
        receiver_principal: ACCOUNT.to_string(),
        acceptance_state: dataexchange::state::DATA_GRANT_PENDING_RECEIVER_ACCEPTANCE.to_string(),
        accepted_at: None,
        ends_at: None,
        grant_distribution_scope: "NONE".to_string(),
        data_set_id: dataexchange::state::mint_id(),
        source_data_set_id: dataexchange::state::mint_id(),
        tags: HashMap::new(),
        created_at,
        updated_at: created_at,
    };

    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let mut store = state.lock();
    store.data_grants.insert(id.clone(), record);
    store.data_grant_order.push(id.clone());
    id
}

/// RULE 3: a page carries at most the page size asked for, its `NextToken` resumes exactly
/// after its last entry, and walking the tokens yields the whole collection once.
#[tokio::test]
async fn a_page_size_bounds_the_page_and_its_token_resumes_the_walk() {
    let router = emulator();
    let expected: Vec<String> = created_grants(&router)
        .await
        .iter()
        .map(|(_, answer)| answer.string("Id"))
        .collect();

    // Rows differ along the page size: one entry per page, a page that splits the
    // collection, one that holds it whole, and the largest the model admits.
    for size in [MAX_RESULTS_MIN, 2, expected.len() as i64, MAX_RESULTS_MAX] {
        let case = format!("maxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let answer = list(&router, Some(size), token.as_deref()).await;
            let page = ids(&answer);
            assert!(
                page.len() as i64 <= size,
                "case {case}: expected at most {size} entries on page {pages}, got {page:?}"
            );
            walked.extend(page);
            token = answer
                .member("NextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            pages += 1;
            if token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len(),
                "case {case}: expected the walk to end within {} pages, got {pages} and \
                 {walked:?}",
                expected.len()
            );
        }
        assert_eq!(
            walked, expected,
            "case {case}: expected the pages to carry every grant exactly once, in creation \
             order"
        );
    }
}

/// RULE 3: a page size the request carries is checked against the modelled
/// `@range(min: 1, max: 200)` — the published bounds are served, anything outside them is
/// the modelled 400.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = emulator();
    created_grants(&router).await;

    for served in [MAX_RESULTS_MIN, 3, MAX_RESULTS_MAX] {
        let answer = call(&router, "GET", &uri(Some(served), None), None).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={served}: expected 200 inside the published bounds, got {} body \
             {:?}",
            answer.status, answer.body
        );
    }

    for refused in [MAX_RESULTS_MIN - 1, -1, MAX_RESULTS_MAX + 1] {
        let answer = call(&router, "GET", &uri(Some(refused), None), None).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={refused}: expected 400 outside the published bounds, got {} \
             body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case maxResults={refused}: expected ValidationException, got {:?}",
            answer.error
        );
    }
}

/// RULE 3: a `NextToken` this caller was not issued is the modelled 400
/// `ValidationException`, never a silently different page.
#[tokio::test]
async fn a_next_token_this_caller_was_not_issued_is_refused() {
    let router = emulator();
    let created = created_grants(&router).await;
    let anchor = created[0].1.string("Id");

    // Rows differ along the shape of the token: text that is not hex at all, hex that
    // decodes to nothing this service encodes, a well-formed cursor minted for ANOTHER
    // account, one carrying a version this service does not issue, and one longer than it
    // can issue. `hex::encode` is what `state::Cursor::encode` hands a token out with, so
    // a forged one is built with exactly the encoding the service decodes.
    for token in [
        "not-a-token".to_string(),
        hex::encode("whatever"),
        hex::encode(format!("1|{OTHER_ACCOUNT}|1|{anchor}")),
        hex::encode(format!("9|{ACCOUNT}|1|{anchor}")),
        "61".repeat(200),
    ] {
        let case = format!("nextToken={token}");
        let answer = call(&router, "GET", &uri(None, Some(&token)), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case {case}: expected ValidationException, got {:?}",
            answer.error
        );
        assert_eq!(
            message(&answer),
            INVALID_NEXT_TOKEN,
            "case {case}: expected the invalid-token refusal, got {:?}",
            answer.body
        );
    }
}

/// RULE 4: the listing reports — it changes nothing a later caller sees. The same call
/// twice answers the same page, a paged walk writes nothing either, and every grant reads
/// back unchanged afterwards.
#[tokio::test]
async fn listing_data_grants_changes_nothing() {
    let router = emulator();
    let created = created_grants(&router).await;
    let mut before: Vec<Value> = Vec::new();
    for (_, answer) in &created {
        before.push(get_data_grant(&router, &answer.string("Id")).await.body);
    }

    let first = list(&router, None, None).await;
    // A bounded page and its resumption too, so neither the token this call issues nor the
    // one it consumes can write.
    let paged = list(&router, Some(1), None).await;
    let resumed = list(
        &router,
        Some(1),
        paged
            .member("NextToken")
            .as_str()
            .map(std::string::ToString::to_string)
            .as_deref(),
    )
    .await;
    let second = list(&router, None, None).await;

    assert_eq!(
        summaries(&second),
        summaries(&first),
        "case repeated listing: expected the same page twice"
    );
    assert_eq!(
        ids(&paged).len() + ids(&resumed).len(),
        2,
        "case paged walk: expected one entry per page of size 1, got {:?} then {:?}",
        ids(&paged),
        ids(&resumed)
    );
    for ((case, answer), stored) in created.iter().zip(before.iter()) {
        let fetched = get_data_grant(&router, &answer.string("Id")).await;
        assert_eq!(
            &fetched.body, stored,
            "case {case}: expected the stored grant to be untouched by the reads"
        );
    }
}
