// SPDX-License-Identifier: CC0-1.0

//! `ListDataLakeExceptions`, end to end over HTTP.
//!
//! RULES under test:
//!
//! * the read reports the caller's OWN account state, so it is never refused on the
//!   caller's Security Lake role — whatever role the account holds, the answer is the
//!   account's enablement state and the exceptions its Regions have recorded;
//! * an account with Security Lake enabled in no Region at all holds no exception
//!   collection, so the read is refused with the modelled `ResourceNotFoundException`
//!   (HTTP 404) carrying the documented remedy, and the refusal comes back as soon as the
//!   last data lake goes;
//! * once the account is enabled the read reports the failures its stored data lakes
//!   carry, in the Regions' creation order, narrowed to the `regions` the request names
//!   and paged by `maxResults` behind an opaque `nextToken`;
//! * a refusal is a property of the ROUTE: the same unentitled caller whose
//!   `UpdateDataLake` is refused at the front door is still served this read.
//!
//! Every case drives a real `restJson1` request through the router into this service, on
//! the shared harness in `data_lake_and_subscriber_lifecycle`. Only the per-Region FAILURE
//! is seeded through the store, because no modelled operation records one: Security Lake
//! writes it when a Region's own configuration or ingestion fails.

use serde_json::{json, Value};
use service_com_amazonaws_securitylake_securitylake::ops::types::{
    DataLakeUpdateException, DataLakeUpdateStatus,
};
use service_com_amazonaws_securitylake_securitylake::state::{AccountRole, DATA_LAKE_STATUS_FAILED};

use super::data_lake_and_subscriber_lifecycle::{array, text, Emulator, Reply};

/// The wording the enablement precondition answers with
/// (`service_com_amazonaws_securitylake_securitylake::state::NOT_ENABLED_IN_ANY_REGION_MESSAGE`),
/// read from the service rather than copied, so there is one source for it.
const GATE_MESSAGE: &str =
    service_com_amazonaws_securitylake_securitylake::state::NOT_ENABLED_IN_ANY_REGION_MESSAGE;

/// The message the account-entitlement gate refuses a GATED route with, pinned here so a
/// case can assert this read never carries it.
const ENTITLEMENT_MESSAGE: &str = "To perform this operation, your account must be a delegated Security Lake administrator account or a standalone Security Lake account.";

/// Every role the account topology holds (`state::AccountRole`), so the rows below span
/// the dimension the entitlement gate decides on.
const ROLES: &[(&str, AccountRole)] = &[
    ("an organization member", AccountRole::OrganizationMember),
    (
        "the delegated administrator",
        AccountRole::DelegatedAdministrator,
    ),
    ("a standalone account", AccountRole::Standalone),
];

/// Send `ListDataLakeExceptions` with `body`, signed for `region`.
async fn list_exceptions(emulator: &Emulator, region: &str, body: Value) -> Reply {
    emulator
        .send("POST", "/v1/datalake/exceptions", region, Some(body))
        .await
}

/// The `message` an error reply carries, or `None` when it carries none.
fn message(reply: &Reply) -> Option<&str> {
    reply.body["message"].as_str()
}

/// Enable Security Lake in `region`, as an account that may administer it.
async fn enable(emulator: &Emulator, region: &str) {
    let previous = emulator.set_account_role(AccountRole::Standalone);
    let created = emulator
        .create_data_lake(region, json!({ "region": region }))
        .await;
    assert_eq!(
        created.status, 200,
        "setup: enabling {region:?} answered {} with {:?}",
        created.status, created.body
    );
    emulator.set_account_role(previous);
}

/// Record, against the stored data lake of `region`, the FAILED request status whose code
/// `ListDataLakeExceptions` reports. Seeded through the store because no modelled
/// operation writes it.
fn record_failure(emulator: &Emulator, region: &str, code: &str) {
    let store = emulator.store();
    let mut store = store.lock();
    let lake = store
        .data_lake_mut(region)
        .unwrap_or_else(|| panic!("setup: no data lake stored for {region:?}"));
    lake.update_status = Some(DataLakeUpdateStatus {
        request_id: None,
        status: Some(DATA_LAKE_STATUS_FAILED.to_string()),
        exception: Some(DataLakeUpdateException {
            code: Some(code.to_string()),
            reason: Some(format!("the {region} data lake could not be configured")),
        }),
    });
}

/// The `(region, exception)` pairs one reply reports, in the order it reports them.
type Reported = Vec<(String, Option<String>)>;

/// The `(region, exception)` pairs a reply reports, in the order it reports them.
fn reported(reply: &Reply, case: &str) -> Reported {
    array(reply, "exceptions", case)
        .iter()
        .map(|recorded| {
            (
                text(recorded, "region", case).to_string(),
                recorded["exception"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// The request bodies this read is sent with, spanning what the rule generalises over:
/// no members at all, a `regions` list of one and of several, `maxResults` at each
/// `@range` bound and at its `@default`, and a `nextToken` of exactly the form this
/// service issues.
fn request_bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("no members", json!({})),
        ("one region", json!({ "regions": ["us-east-1"] })),
        (
            "two regions",
            json!({ "regions": ["us-east-1", "ap-southeast-2"] }),
        ),
        ("maxResults at the minimum", json!({ "maxResults": 1 })),
        ("maxResults at the default", json!({ "maxResults": 50 })),
        ("maxResults at the maximum", json!({ "maxResults": 100 })),
        (
            "a well-formed nextToken",
            json!({ "nextToken": hex::encode("0:us-east-1") }),
        ),
    ]
}

/// RULE: with Security Lake enabled in no Region, the read is refused 404
/// `ResourceNotFoundException` with the enablement remedy — whatever the request carries,
/// whichever Region it is signed for, and whatever Security Lake role the account holds
/// (it is NOT a gated route, so the entitlement refusal must never appear).
#[tokio::test]
async fn a_never_enabled_account_is_refused_the_enablement_precondition() {
    for (role_case, role) in ROLES {
        for region in ["us-east-1", "eu-west-2", "ap-southeast-2"] {
            for (body_case, body) in request_bodies() {
                let emulator = Emulator::new();
                emulator.set_account_role(*role);
                let case = format!("{role_case} / {body_case} in {region}");

                let reply = list_exceptions(&emulator, region, body).await;

                assert_eq!(
                    (reply.status, reply.error_type.as_deref(), message(&reply)),
                    (404, Some("ResourceNotFoundException"), Some(GATE_MESSAGE)),
                    "case {case:?}: expected 404 ResourceNotFoundException with the enablement \
                     message, got {} {:?} with {:?}",
                    reply.status,
                    reply.error_type,
                    reply.body
                );
            }
        }
    }
}

/// RULE: the read reports the caller's own account state, so an account holding NO
/// Security Lake role is served it exactly as an administering account is — the answer
/// turns on enablement, never on the role.
#[tokio::test]
async fn the_read_is_not_refused_on_the_callers_security_lake_role() {
    const REGION: &str = "us-west-2";

    let mut answers: Vec<(String, u16, Value)> = Vec::new();
    for (role_case, role) in ROLES {
        let emulator = Emulator::new();
        enable(&emulator, REGION).await;
        emulator.set_account_role(*role);

        let reply = list_exceptions(&emulator, REGION, json!({})).await;
        assert_eq!(
            reply.status, 200,
            "case {role_case:?}: an enabled account must be served, got {} with {:?}",
            reply.status, reply.body
        );
        assert_ne!(
            message(&reply),
            Some(ENTITLEMENT_MESSAGE),
            "case {role_case:?}: this read is not gated on the role, got {:?}",
            reply.body
        );
        assert!(
            array(&reply, "exceptions", role_case).is_empty(),
            "case {role_case:?}: a Region that recorded no failure reports none, got {:?}",
            reply.body
        );
        answers.push(((*role_case).to_string(), reply.status, reply.body));
    }

    let (first_case, first_status, first_body) = answers[0].clone();
    for (case, status, body) in &answers[1..] {
        assert_eq!(
            (*status, body),
            (first_status, &first_body),
            "case {case:?}: the answer must be identical to {first_case:?}, got {status} \
             {body:?} against {first_status} {first_body:?}"
        );
    }
}

/// RULE: a refusal belongs to the ROUTE, so one unentitled caller gets the front door's
/// 401 refusal on the gated `UpdateDataLake` and this read's own state-derived answer on
/// the same emulator, in the same breath.
#[tokio::test]
async fn one_unentitled_caller_is_refused_the_update_and_served_the_read() {
    const REGION: &str = "eu-central-1";

    let emulator = Emulator::new();
    enable(&emulator, REGION).await;
    emulator.set_account_role(AccountRole::OrganizationMember);

    let refused = emulator
        .send(
            "PUT",
            "/v1/datalake",
            REGION,
            Some(json!({ "configurations": [{ "region": REGION }] })),
        )
        .await;
    assert_eq!(
        (
            refused.status,
            refused.error_type.as_deref(),
            message(&refused)
        ),
        (401, Some("UnauthorizedException"), Some("Unauthorized")),
        "case \"UpdateDataLake\": expected the route's 401 refusal, got {} {:?} with {:?}",
        refused.status,
        refused.error_type,
        refused.body
    );

    let served = list_exceptions(&emulator, REGION, json!({})).await;
    assert_eq!(
        served.status, 200,
        "case \"ListDataLakeExceptions\": the same caller must be served the read, got {} \
         with {:?}",
        served.status, served.body
    );
}

/// RULE: once the account is enabled the read reports the failures its stored data lakes
/// carry, in the Regions' creation order, and `regions` narrows the report to the Regions
/// it names.
#[tokio::test]
async fn the_read_reports_the_recorded_failures_of_the_regions_named() {
    // The Regions, in creation order, and the exception code recorded against each. The
    // middle Region records nothing, so the rows also cover a Region that is enabled and
    // healthy. The codes are space-free tokens, as `DataLakeException$exception`'s
    // `SafeString` pattern `^[\\\w\-_:/.@=+]*$` requires.
    let enabled: [(&str, Option<&str>); 3] = [
        ("us-east-1", Some("S3_BUCKET_POLICY_DENIED")),
        ("eu-west-2", None),
        ("ap-southeast-2", Some("KMS_KEY_UNAVAILABLE")),
    ];

    let emulator = Emulator::new();
    for (region, code) in enabled {
        enable(&emulator, region).await;
        if let Some(code) = code {
            record_failure(&emulator, region, code);
        }
    }

    // Rows: the `regions` filter, and the `(region, exception)` pairs the read must then
    // report — in creation order, whatever order the filter names them in.
    let rows: Vec<(&str, Value, Reported)> = vec![
        (
            "no regions member asks about every Region",
            json!({}),
            vec![
                (
                    "us-east-1".to_string(),
                    Some("S3_BUCKET_POLICY_DENIED".to_string()),
                ),
                (
                    "ap-southeast-2".to_string(),
                    Some("KMS_KEY_UNAVAILABLE".to_string()),
                ),
            ],
        ),
        (
            "one Region that recorded a failure",
            json!({ "regions": ["ap-southeast-2"] }),
            vec![(
                "ap-southeast-2".to_string(),
                Some("KMS_KEY_UNAVAILABLE".to_string()),
            )],
        ),
        (
            "one Region that recorded none",
            json!({ "regions": ["eu-west-2"] }),
            Vec::new(),
        ),
        (
            "a Region this account has not enabled",
            json!({ "regions": ["sa-east-1"] }),
            Vec::new(),
        ),
        (
            "every Region, named in reverse",
            json!({ "regions": ["ap-southeast-2", "eu-west-2", "us-east-1"] }),
            vec![
                (
                    "us-east-1".to_string(),
                    Some("S3_BUCKET_POLICY_DENIED".to_string()),
                ),
                (
                    "ap-southeast-2".to_string(),
                    Some("KMS_KEY_UNAVAILABLE".to_string()),
                ),
            ],
        ),
    ];

    for (case, body, expected) in rows {
        let reply = list_exceptions(&emulator, "us-east-1", body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reported(&reply, case),
            expected,
            "case {case:?}: wrong exceptions reported, got {:?}",
            reply.body
        );
        assert!(
            reply.body["nextToken"].is_null(),
            "case {case:?}: nothing follows the page, so no token, got {:?}",
            reply.body
        );

        // The read changes nothing a later caller sees: the same request answers the same
        // thing.
        let again = list_exceptions(&emulator, "us-east-1", body).await;
        assert_eq!(
            again.body, reply.body,
            "case {case:?}: the read must be repeatable, got {:?} after {:?}",
            again.body, reply.body
        );
    }
}

/// RULE: the report is paged — `maxResults` bounds the page, a `nextToken` resumes
/// exactly after the last entry returned, the pages concatenate to the whole report, and
/// a token this service did not issue is refused as a bad request.
#[tokio::test]
async fn the_read_pages_the_report() {
    // Five enabled Regions, each with a recorded failure, in creation order.
    let regions = [
        "us-east-1",
        "eu-west-2",
        "ap-southeast-2",
        "ca-central-1",
        "sa-east-1",
    ];

    let emulator = Emulator::new();
    for (index, region) in regions.iter().enumerate() {
        enable(&emulator, region).await;
        record_failure(&emulator, region, &format!("FAILURE_{index}"));
    }
    let whole: Reported = regions
        .iter()
        .enumerate()
        .map(|(index, region)| ((*region).to_string(), Some(format!("FAILURE_{index}"))))
        .collect();

    // Rows: a page size below, at and above the number of entries, plus both `@range`
    // bounds of `maxResults`.
    for page_size in [1usize, 2, 4, 5, 50, 100] {
        let case = format!("maxResults={page_size}");
        let mut collected: Reported = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let body = match &token {
                None => json!({ "maxResults": page_size }),
                Some(token) => json!({ "maxResults": page_size, "nextToken": token }),
            };
            let reply = list_exceptions(&emulator, "us-east-1", body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: page {pages} answered {} with {:?}",
                reply.status, reply.body
            );
            let page = reported(&reply, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page may not exceed maxResults, got {:?}",
                reply.body
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= regions.len() + 1,
                "case {case:?}: paging did not terminate, collected {collected:?}"
            );
            token = reply.body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            collected, whole,
            "case {case:?}: the pages must concatenate to the whole report, got {collected:?}"
        );
        let expected_pages = regions.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} page(s), took {pages}"
        );
    }

    // Rows: tokens this service never issued, in the forms a client can send — not hex at
    // all, hex of text with no index, and hex of an index with no anchor.
    for token in ["not-hex".to_string(), hex::encode("no-index"), hex::encode("7:")] {
        let case = format!("nextToken={token:?}");
        let reply = list_exceptions(&emulator, "us-east-1", json!({ "nextToken": token })).await;
        assert_eq!(
            (reply.status, reply.error_type.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a token this service did not issue is a bad request, got {} with {:?}",
            reply.status,
            reply.body
        );
    }
}

/// RULE: the collection tracks the stored data lakes, so deleting the last one takes it
/// away and the read goes back to the enablement precondition.
#[tokio::test]
async fn deleting_the_last_data_lake_restores_the_precondition() {
    let regions = ["us-east-1", "eu-west-2"];

    let emulator = Emulator::new();
    for region in regions {
        enable(&emulator, region).await;
        record_failure(&emulator, region, "S3_BUCKET_POLICY_DENIED");
    }

    for (removed, region) in regions.iter().enumerate() {
        let deleted = emulator.delete_data_lake(region, &[region]).await;
        assert_eq!(
            deleted.status, 200,
            "case {region:?}: deleting the data lake answered {} with {:?}",
            deleted.status, deleted.body
        );

        let reply = list_exceptions(&emulator, regions[0], json!({})).await;
        let last_one_gone = removed + 1 == regions.len();
        if last_one_gone {
            assert_eq!(
                (reply.status, reply.error_type.as_deref(), message(&reply)),
                (404, Some("ResourceNotFoundException"), Some(GATE_MESSAGE)),
                "case {region:?}: with no data lake left the read must be the precondition, \
                 got {} {:?} with {:?}",
                reply.status,
                reply.error_type,
                reply.body
            );
        } else {
            assert_eq!(
                reported(&reply, region)
                    .into_iter()
                    .map(|(region, _)| region)
                    .collect::<Vec<_>>(),
                regions[removed + 1..]
                    .iter()
                    .map(|region| (*region).to_string())
                    .collect::<Vec<_>>(),
                "case {region:?}: a deleted Region's exception must go with it, got {:?}",
                reply.body
            );
        }
    }
}
