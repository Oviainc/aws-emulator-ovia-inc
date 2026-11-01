// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListApplicationAssociations`.
//!
//! The rule under test: the listing reports one page of the associations the stored
//! Application the request names holds — each one as the ARN, application ARN and client
//! id persisted for it — paginates them with `MaxResults`/`NextToken`, and answers a 404
//! for an identifier no stored application carries.
//!
//! AppIntegrations publishes no create for an application association
//! (`smithy_query(query="digest")` binds one for every other resource of this service
//! and none for this one): the association comes into being on the side of the client
//! that associates the application. A case that needs associations stored therefore puts
//! them in the store the router serves from (`Emulator::appintegrations_state`), and then
//! reads every assertion off the wire — so a response that is not computed from
//! persisted state cannot pass.
//!
//! The suites are tables: rows differ in how many associations are stored, in the client
//! id's form and length, in the identifier form the request addresses the application by
//! and in the page size. A new application with no associations — the recorded case — is
//! one row.

use serde_json::{json, Value};
use service_com_amazonaws_appintegrations_amazonappintegrationservice::state::{
    application_association_arn, ApplicationAssociationRecord,
};

use super::harness::{encode_label, Emulator, Reply, REGION};

/// The canonical account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// `MaxResults` is `range 1..50` on `ListApplicationAssociations` (model:
/// `smithy_query(query="operation", operation="ListApplicationAssociations")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 50;

/// `ClientId` is `len 1..255, pattern .*` (model: `smithy_query(query="shape",
/// shape="ApplicationAssociationSummary")`).
const CLIENT_ID_MAX: usize = 255;

/// `POST /applications`, answering the `(Id, Arn)` it minted.
async fn create_application(emulator: &Emulator, name: &str) -> (String, String) {
    let reply = emulator
        .send(
            "POST",
            "/applications",
            REGION,
            Some(json!({
                "Name": name,
                "Namespace": name,
                "ApplicationSourceConfig": {
                    "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
                },
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "CreateApplication must succeed so a case has an application, got {} / {}",
        reply.status, reply.body
    );
    (
        reply.string("Id").expect("the create reported an Id"),
        reply.string("Arn").expect("the create reported an ARN"),
    )
}

/// `ListApplicationAssociations`: `GET /applications/{ApplicationId}/associations`.
async fn list(emulator: &Emulator, application: &str, query: &str) -> Reply {
    let base = format!("/applications/{}/associations", encode_label(application));
    let uri = if query.is_empty() {
        base
    } else {
        format!("{base}?{query}")
    };
    emulator.send("GET", &uri, REGION, None).await
}

/// The reported page, as a list (an absent member is an empty page).
fn page_of(reply: &Reply) -> Vec<Value> {
    reply
        .field("ApplicationAssociations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The ARN form an application association carries, written out here rather than taken
/// from the crate's helper so the test pins the format.
fn expected_association_arn(application_id: &str, association_id: &str) -> String {
    format!(
        "arn:aws:app-integrations:{REGION}:{ACCOUNT}:\
         application-association/{application_id}/{association_id}"
    )
}

/// The summary the listing must report for `record`: the three members
/// `ApplicationAssociationSummary` carries.
fn expected_summary(record: &ApplicationAssociationRecord) -> Value {
    json!({
        "ApplicationAssociationArn": record.arn,
        "ApplicationArn": record.application_arn,
        "ClientId": record.client_id,
    })
}

/// Store one association per entry of `client_ids` for the application
/// `(application_id, application_arn)`, answering the records in the order the store
/// keeps them.
///
/// The association ids are hex-ordered, so the store's order is the order they were
/// added in, and they continue after whatever is already stored so two applications
/// never share one.
fn seed_associations(
    emulator: &Emulator,
    application_id: &str,
    application_arn: &str,
    client_ids: &[String],
) -> Vec<ApplicationAssociationRecord> {
    let state = emulator.appintegrations_state();
    let mut guard = state.lock();
    let first = guard.application_associations.len();
    let mut seeded = Vec::new();
    for (offset, client_id) in client_ids.iter().enumerate() {
        let id = format!("{:08x}-0000-4000-8000-000000000000", first + offset);
        let record = ApplicationAssociationRecord {
            arn: application_association_arn(REGION, application_id, &id),
            id: id.clone(),
            application_id: application_id.to_string(),
            application_arn: application_arn.to_string(),
            client_id: client_id.clone(),
        };
        guard.application_associations.insert(id, record.clone());
        seeded.push(record);
    }
    seeded
}

#[tokio::test]
async fn the_listing_reports_the_stored_associations_of_the_application_it_names() {
    // Rows differ in how many associations the application holds and in the client id's
    // form and length: none (the recorded case), one, and several including both
    // published bounds of `ClientId`'s length.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("no association", Vec::new()),
        ("one association", vec!["connect-instance-1".to_string()]),
        (
            "several associations, at both bounds of ClientId",
            vec![
                "c".to_string(),
                "c".repeat(CLIENT_ID_MAX),
                "arn:aws:connect:us-east-1:111122223333:instance/abc".to_string(),
            ],
        ),
    ];

    for (label, client_ids) in rows {
        let emulator = Emulator::new();
        let (application_id, application_arn) = create_application(&emulator, "target").await;
        let (neighbour_id, neighbour_arn) = create_application(&emulator, "neighbour").await;
        // A neighbour's associations are not this application's: the listing reports the
        // ones belonging to the application the request names.
        seed_associations(
            &emulator,
            &neighbour_id,
            &neighbour_arn,
            &["neighbour-client".to_string()],
        );
        let stored =
            seed_associations(&emulator, &application_id, &application_arn, &client_ids);
        let expected: Vec<Value> = stored.iter().map(expected_summary).collect();

        // Every identifier form the `ArnOrUUID` label accepts, with and without the
        // trailing qualifier its pattern allows.
        for (form, identifier) in [
            ("by id", application_id.clone()),
            ("by arn", application_arn.clone()),
            ("by id with a qualifier", format!("{application_id}:1")),
            ("by arn with a qualifier", format!("{application_arn}:1")),
        ] {
            let reply = list(&emulator, &identifier, "").await;
            assert_eq!(
                reply.status, 200,
                "case {label:?} {form}: expected HTTP 200, got {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                page_of(&reply),
                expected,
                "case {label:?} {form}: expected {expected:?}, got {}",
                reply.body
            );
            assert_eq!(
                reply.field("NextToken"),
                None,
                "case {label:?} {form}: a last page must report no NextToken, got {:?}",
                reply.field("NextToken")
            );
        }

        // The ARN the listing reports is the documented form, built from the stored ids.
        for record in &stored {
            assert_eq!(
                record.arn,
                expected_association_arn(&application_id, &record.id),
                "case {label:?}: expected the documented association ARN, got {:?}",
                record.arn
            );
        }

        // The listing reports; it changes nothing a later caller sees.
        let repeated = list(&emulator, &application_id, "").await;
        assert_eq!(
            page_of(&repeated),
            expected,
            "case {label:?}: a repeated listing must report the same page, got {}",
            repeated.body
        );
        assert_eq!(
            page_of(&list(&emulator, &neighbour_id, "").await).len(),
            1,
            "case {label:?}: the neighbour must still hold its own association"
        );
    }
}

#[tokio::test]
async fn an_identifier_no_stored_application_carries_is_not_found() {
    let emulator = Emulator::new();
    let (application_id, _) = create_application(&emulator, "present").await;

    // Rows differ in why the identifier names no application here: a UUID never minted,
    // an ARN of another account, of another Region, and of another resource type of this
    // same service carrying a real application id.
    for (label, identifier) in [
        (
            "a UUID this service never minted",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an ARN from another account",
            format!("arn:aws:app-integrations:{REGION}:111122223333:application/{application_id}"),
        ),
        (
            "an ARN from another Region",
            format!("arn:aws:app-integrations:eu-west-1:{ACCOUNT}:application/{application_id}"),
        ),
        (
            "an ARN of another resource type",
            format!(
                "arn:aws:app-integrations:{REGION}:{ACCOUNT}:data-integration/{application_id}"
            ),
        ),
    ] {
        let reply = list(&emulator, &identifier, "").await;
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_type
        );
        let expected = format!("Could not find Application with identifier: {identifier}.");
        assert_eq!(
            reply.message().as_deref(),
            Some(expected.as_str()),
            "case {label:?}: expected {expected:?}, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn the_page_controls_of_the_association_listing_follow_their_published_bounds() {
    let emulator = Emulator::new();
    let (application_id, application_arn) = create_application(&emulator, "paged").await;
    let client_ids: Vec<String> = (0..7).map(|index| format!("client-{index}")).collect();
    let stored = seed_associations(&emulator, &application_id, &application_arn, &client_ids);
    let all: Vec<Value> = stored.iter().map(expected_summary).collect();

    // Rows differ in the page size the walk uses: one per page, a size that divides the
    // stored count, one that does not, exactly the stored count, and the largest the
    // model allows. Following the cursors must report every association exactly once, in
    // the store's order, whatever the page size.
    for page_size in [1_usize, 3, 4, 7, MAX_RESULTS_MAX as usize] {
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(cursor) => format!("maxResults={page_size}&nextToken={cursor}"),
            };
            let reply = list(&emulator, &application_id, &query).await;
            assert_eq!(
                reply.status, 200,
                "case maxResults={page_size} page {pages}: expected HTTP 200, got {} / {}",
                reply.status, reply.body
            );
            let page = page_of(&reply);
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size} page {pages}: a page must not exceed the \
                 requested size, got {} entries",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case maxResults={page_size}: the walk must terminate, got {pages} pages"
            );
            token = reply.string("NextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case maxResults={page_size}: expected every stored association once in \
             order, got {seen:?}"
        );
    }

    // One past each published bound of `MaxResults`, then each bound itself.
    for (max, expected_status) in [
        (MAX_RESULTS_MIN - 1, 400),
        (MAX_RESULTS_MAX + 1, 400),
        (MAX_RESULTS_MIN, 200),
        (MAX_RESULTS_MAX, 200),
    ] {
        let reply = list(&emulator, &application_id, &format!("maxResults={max}")).await;
        assert_eq!(
            reply.status, expected_status,
            "case maxResults={max}: expected HTTP {expected_status}, got {} / {}",
            reply.status, reply.body
        );
    }

    // Tokens differing in why they are not cursors this service minted: not hex at all,
    // odd-length hex, hex that decodes to bytes that are not text.
    for token in ["not-a-cursor", "abc", "fffe"] {
        let reply = list(&emulator, &application_id, &format!("nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected HTTP 400 for a foreign NextToken, got {} / {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "InvalidRequestException",
            "case {token:?}: expected InvalidRequestException, got {:?}",
            reply.error_type
        );
    }
}
