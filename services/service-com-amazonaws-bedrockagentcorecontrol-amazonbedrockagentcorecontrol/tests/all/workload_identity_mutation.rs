// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the workload-identity mutations
//! (`UpdateWorkloadIdentity`, `DeleteWorkloadIdentity`) and what they leave behind for
//! `GetWorkloadIdentity` and `ListWorkloadIdentities` to report.
//!
//! The rules under test:
//!
//! * an update of a stored identity REPLACES its `allowedResourceOauth2ReturnUrls` with
//!   exactly the list the request carried, in that order ("This list replaces the existing
//!   list", `smithy_query doc shape=UpdateWorkloadIdentityRequest
//!   member=allowedResourceOauth2ReturnUrls`), bumps `lastUpdatedTime`, keeps `name`,
//!   `workloadIdentityArn` and `createdTime`, and the following read reports that record;
//! * a delete of a stored identity answers the modelled 204 carrying no member and removes
//!   the entry, so the read misses, the listing no longer reports it and the name is free
//!   to be created again;
//! * either mutation addressing a `name` this account does not hold — including a name an
//!   earlier delete removed — is refused with the modelled 404
//!   `ResourceNotFoundException`, naming the resource type and the requested name, and
//!   stores nothing.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! `MultiServiceRouter::dispatch` (through [`super::harness`]), so the wire protocol
//! (`restJson1`), the boundary constraint validation and the handlers are all exercised.

use serde_json::{json, Value};

use super::harness::{
    create_identity, delete_identity, error_code, expected_arn, get_identity, list_identities,
    listed_names, not_found_by_id, not_found_by_name, router, text, update_identity,
    MAX_RESULTS_MAX, NAME_MAX_LENGTH, NAME_MIN_LENGTH,
};

/// The modelled success statuses: `UpdateWorkloadIdentity` answers 200, and
/// `DeleteWorkloadIdentity` answers 204 with an empty output shape
/// (`smithy_query operation=UpdateWorkloadIdentity,DeleteWorkloadIdentity`: `http.code`).
const UPDATE_STATUS: u16 = 200;
const DELETE_STATUS: u16 = 204;

/// The members an identity carries unchanged across an update
/// (`smithy_query operation=UpdateWorkloadIdentity`: output members).
const STABLE_MEMBERS: [&str; 3] = ["name", "workloadIdentityArn", "createdTime"];

/// One row of the replacement table: the identity to store, and the list the update
/// carries for it.
struct UpdateCase {
    case: &'static str,
    name: String,
    created_urls: Option<Vec<&'static str>>,
    updated_urls: Option<Vec<&'static str>>,
}

/// One row of the delete table: the identity to store before it is deleted.
struct DeleteCase {
    case: &'static str,
    name: String,
    urls: Option<Vec<&'static str>>,
}

/// Whether an answer carries no member at all: `DeleteWorkloadIdentity`'s output is an
/// empty structure, so a success holds nothing whichever envelope it renders in (no body,
/// or an empty object).
fn carries_no_member(body: &Value) -> bool {
    match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// Assert `answer` is the modelled 404 `ResourceNotFoundException` carrying `message`.
fn assert_not_found(case: &str, status: u16, body: &Value, message: &str) {
    assert_eq!(
        (status, error_code(body), text(body, "message")),
        (
            404,
            "ResourceNotFoundException".to_string(),
            message.to_string()
        ),
        "case {case:?}: expected HTTP 404 ResourceNotFoundException {message:?}, got {status} {} \
         ({body})",
        error_code(body)
    );
}

/// An epoch-seconds instant off a response member. The prelude `Timestamp` carries no
/// `@timestampFormat`, so on this service's restJson1 wire each instant is a NUMBER.
fn instant(case: &str, body: &Value, member: &str) -> f64 {
    body[member].as_f64().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {member} as epoch-seconds, got {}",
            body[member]
        )
    })
}

/// The names the listing reports, at a page size above every count these tests store.
async fn stored_names(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let (_, page) = list_identities(router, Some(MAX_RESULTS_MAX), None).await;
    listed_names(&page)
}

/// An update replaces the stored return-URL list with exactly the one the request carried,
/// and the read reports that record rather than the create-time snapshot.
///
/// Rows differ along what the replacement is: growing, shrinking and same-size lists, a
/// list replacing none, the empty list, no list at all (which leaves the identity with
/// none), an order no sort would produce, and names at the modelled length bounds.
#[tokio::test]
async fn an_update_replaces_the_stored_return_url_list_and_the_read_reports_it() {
    let router = router();

    let rows: [UpdateCase; 7] = [
        UpdateCase {
            case: "none at create, two at update",
            name: "update-adds-urls".to_string(),
            created_urls: None,
            updated_urls: Some(vec!["https://example.com/a", "myapp://auth/return"]),
        },
        UpdateCase {
            case: "one at create, three at update, in an order no sort produces",
            name: "update-grows_the.list".to_string(),
            created_urls: Some(vec!["https://example.com/first"]),
            updated_urls: Some(vec![
                "https://example.com/zulu",
                "http://localhost:8080/return",
                "https://example.com/alpha",
            ]),
        },
        UpdateCase {
            case: "three at create, one at update",
            name: "update-narrows-the-list".to_string(),
            created_urls: Some(vec![
                "https://example.com/a",
                "https://example.com/b",
                "https://example.com/c",
            ]),
            updated_urls: Some(vec!["https://example.com/only"]),
        },
        UpdateCase {
            case: "one at create, a different one of the same count at update",
            name: "a".repeat(NAME_MIN_LENGTH),
            created_urls: Some(vec!["https://example.com/old"]),
            updated_urls: Some(vec!["https://example.com/new"]),
        },
        UpdateCase {
            case: "a list replaced by the empty list",
            name: "update-empties-the-list".to_string(),
            created_urls: Some(vec!["https://example.com/revoked"]),
            updated_urls: Some(vec![]),
        },
        UpdateCase {
            case: "a list replaced by no list at all",
            name: "update-clears_the.list".to_string(),
            created_urls: Some(vec!["https://example.com/revoked"]),
            updated_urls: None,
        },
        UpdateCase {
            case: "the longest name the model admits",
            name: "z".repeat(NAME_MAX_LENGTH),
            created_urls: None,
            updated_urls: Some(vec!["https://example.com/longest"]),
        },
    ];

    for row in &rows {
        let UpdateCase {
            case,
            name,
            created_urls,
            updated_urls,
        } = row;

        let mut body = json!({"name": name});
        if let Some(urls) = created_urls {
            body["allowedResourceOauth2ReturnUrls"] = json!(urls);
        }
        let (status, created) = create_identity(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateWorkloadIdentity, got {status} ({created})"
        );

        let (_, before) = get_identity(&router, name).await;
        let created_time = instant(case, &before, "createdTime");

        let (status, updated) = update_identity(&router, name, updated_urls.as_ref()).await;
        assert_eq!(
            status, UPDATE_STATUS,
            "case {case:?}: expected HTTP {UPDATE_STATUS} from UpdateWorkloadIdentity, got \
             {status} ({updated})"
        );
        let expected_list = updated_urls
            .as_ref()
            .map_or(Value::Null, |urls| json!(urls));
        assert_eq!(
            updated["allowedResourceOauth2ReturnUrls"], expected_list,
            "case {case:?}: the update must report exactly the list it was given, got {}",
            updated["allowedResourceOauth2ReturnUrls"]
        );
        assert_eq!(
            text(&updated, "name"),
            *name,
            "case {case:?}: expected the name the request carried, got {}",
            text(&updated, "name")
        );
        assert_eq!(
            text(&updated, "workloadIdentityArn"),
            expected_arn(name),
            "case {case:?}: an update does not re-mint the ARN, got {}",
            text(&updated, "workloadIdentityArn")
        );
        assert_eq!(
            instant(case, &updated, "createdTime"),
            created_time,
            "case {case:?}: an update does not move createdTime, got {} for {created_time}",
            instant(case, &updated, "createdTime")
        );
        assert!(
            instant(case, &updated, "lastUpdatedTime") >= created_time,
            "case {case:?}: expected lastUpdatedTime at or after createdTime {created_time}, got \
             {}",
            instant(case, &updated, "lastUpdatedTime")
        );

        // The read reports the record the update left, member for member.
        let (status, after) = get_identity(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetWorkloadIdentity, got {status} ({after})"
        );
        assert_eq!(
            after["allowedResourceOauth2ReturnUrls"], expected_list,
            "case {case:?}: the read must report the updated list, got {}",
            after["allowedResourceOauth2ReturnUrls"]
        );
        for member in STABLE_MEMBERS {
            assert_eq!(
                after[member], updated[member],
                "case {case:?}: the read must report the {member} the update reported, got {} \
                 for {}",
                after[member], updated[member]
            );
        }
        assert_eq!(
            after["lastUpdatedTime"], updated["lastUpdatedTime"],
            "case {case:?}: the read must report the lastUpdatedTime the update reported, got {} \
             for {}",
            after["lastUpdatedTime"], updated["lastUpdatedTime"]
        );

        // The update addressed one identity and removed none: the one it named is still
        // stored alongside every row before it.
        assert!(
            stored_names(&router).await.contains(name),
            "case {case:?}: the updated identity is still listed, got {:?}",
            stored_names(&router).await
        );
    }

    // A second update applied to an already-updated identity replaces the list again, so
    // the store holds the LAST list rather than an accumulation.
    let name = "update-grows_the.list";
    let final_list = vec!["https://example.com/final"];
    let (status, again) = update_identity(&router, name, Some(&final_list)).await;
    assert_eq!(
        (status, again["allowedResourceOauth2ReturnUrls"].clone()),
        (UPDATE_STATUS, json!(final_list)),
        "case \"a second update of the same identity\": expected the latest list only, got \
         {status} ({again})"
    );
    let (_, read) = get_identity(&router, name).await;
    assert_eq!(
        read["allowedResourceOauth2ReturnUrls"],
        json!(final_list),
        "case \"a read after the second update\": expected the latest list only, got {}",
        read["allowedResourceOauth2ReturnUrls"]
    );
}

/// A delete removes the identity: it answers the modelled 204 carrying no member, the read
/// then misses, the listing no longer reports it, a second delete of the same name is
/// refused rather than silently answered, and the name is free to be created again.
///
/// Rows differ along the identity's shape: each character class the name pattern admits,
/// both modelled length bounds, and whether the identity carried return URLs.
#[tokio::test]
async fn a_delete_removes_the_identity_and_is_refused_the_second_time() {
    let router = router();

    let rows: [DeleteCase; 5] = [
        DeleteCase {
            case: "the shortest name the model admits",
            name: "a".repeat(NAME_MIN_LENGTH),
            urls: None,
        },
        DeleteCase {
            case: "dots, dashes and underscores, with return URLs",
            name: "payments.service_v1-east".to_string(),
            urls: Some(vec!["https://example.com/oauth2/callback", "myapp://return"]),
        },
        DeleteCase {
            case: "digits only",
            name: "20261030".to_string(),
            urls: None,
        },
        DeleteCase {
            case: "mixed case, with an empty URL list",
            name: "MixedCaseIdentity".to_string(),
            urls: Some(vec![]),
        },
        DeleteCase {
            case: "the longest name the model admits",
            name: "z".repeat(NAME_MAX_LENGTH),
            urls: None,
        },
    ];

    let mut held: Vec<String> = Vec::new();
    for row in &rows {
        let mut body = json!({"name": row.name});
        if let Some(urls) = &row.urls {
            body["allowedResourceOauth2ReturnUrls"] = json!(urls);
        }
        let (status, created) = create_identity(&router, &body).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected HTTP 201 from CreateWorkloadIdentity, got {status} ({created})",
            row.case
        );
        held.push(row.name.clone());
    }

    for row in &rows {
        let case = row.case;
        let name = &row.name;

        let (status, deleted) = delete_identity(&router, name).await;
        assert_eq!(
            status, DELETE_STATUS,
            "case {case:?}: expected HTTP {DELETE_STATUS} from DeleteWorkloadIdentity, got \
             {status} ({deleted})"
        );
        assert!(
            carries_no_member(&deleted),
            "case {case:?}: a successful delete carries no member, got {deleted}"
        );

        // The read misses, with the read path's wording.
        let (status, read) = get_identity(&router, name).await;
        assert_not_found(case, status, &read, &not_found_by_id(name));

        // The listing no longer reports it, and still reports every identity not deleted.
        held.retain(|stored| stored != name);
        let mut expected = held.clone();
        expected.sort();
        assert_eq!(
            stored_names(&router).await,
            expected,
            "case {case:?}: expected only the identities not yet deleted, got {:?}",
            stored_names(&router).await
        );

        // A second delete of the same name is refused, with the delete path's wording —
        // the delete is retryable, not silently successful on a resource that is gone.
        let (status, again) = delete_identity(&router, name).await;
        assert_not_found(case, status, &again, &not_found_by_name(name));
    }

    // Every name a delete freed can be created again, and the new identity is its own
    // record: freshly created, with no trace of the list the deleted one carried.
    for row in &rows {
        let case = row.case;
        let name = &row.name;

        let (status, recreated) = create_identity(&router, &json!({"name": name})).await;
        assert_eq!(
            status, 201,
            "case {case:?}: a deleted name is free to create again, got {status} ({recreated})"
        );
        let (status, read) = get_identity(&router, name).await;
        assert_eq!(
            (status, read["allowedResourceOauth2ReturnUrls"].clone()),
            (200, Value::Null),
            "case {case:?}: the re-created identity carries no return URLs, got {status} ({read})"
        );
    }
}

/// Either mutation addressing a name this account does not hold is refused with the
/// modelled 404 and stores nothing — no phantom identity the read or the listing could
/// report.
///
/// Rows differ along the shape and length of the name the request names, every one inside
/// `WorkloadIdentityNameType`'s pattern and length bounds so the refusal comes from the
/// handler rather than the boundary validator.
#[tokio::test]
async fn the_mutations_refuse_a_name_the_account_does_not_hold() {
    let router = router();

    // One identity the account DOES hold, so each miss below is measured against a store
    // that is not empty and is left untouched by the refusals.
    let bystander = "bystander-identity";
    let bystander_urls = vec!["https://example.com/bystander"];
    let (status, created) = create_identity(
        &router,
        &json!({"name": bystander, "allowedResourceOauth2ReturnUrls": bystander_urls}),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"the identity the refusals are measured against\": expected HTTP 201, got {status} \
         ({created})"
    );

    // (case, a name that was never created)
    let rows: [(&str, String); 6] = [
        ("a plain unknown name", "no-such-identity".to_string()),
        (
            "an unknown name at the modelled minimum length",
            "a".repeat(NAME_MIN_LENGTH),
        ),
        (
            "an unknown name at the modelled maximum length",
            "z".repeat(NAME_MAX_LENGTH),
        ),
        (
            "an unknown name of dots, dashes and underscores",
            "missing.service_v2-west".to_string(),
        ),
        ("an unknown name of digits only", "19700101".to_string()),
        (
            "an unknown name differing from a held one only in case",
            "Bystander-Identity".to_string(),
        ),
    ];

    for (case, name) in &rows {
        let urls = vec!["https://example.com/cb"];
        let (status, updated) = update_identity(&router, name, Some(&urls)).await;
        assert_not_found(case, status, &updated, &not_found_by_id(name));

        let (status, deleted) = delete_identity(&router, name).await;
        assert_not_found(case, status, &deleted, &not_found_by_name(name));

        // Neither refusal created the identity it could not find.
        let (status, read) = get_identity(&router, name).await;
        assert_not_found(case, status, &read, &not_found_by_id(name));
    }

    // The held identity is exactly as it was created: the refusals wrote nothing.
    assert_eq!(
        stored_names(&router).await,
        vec![bystander.to_string()],
        "case \"after every refusal\": expected only the held identity, got {:?}",
        stored_names(&router).await
    );
    let (status, read) = get_identity(&router, bystander).await;
    assert_eq!(
        (status, read["allowedResourceOauth2ReturnUrls"].clone()),
        (200, json!(bystander_urls)),
        "case \"the held identity after every refusal\": expected its create-time list, got \
         {status} ({read})"
    );
}
