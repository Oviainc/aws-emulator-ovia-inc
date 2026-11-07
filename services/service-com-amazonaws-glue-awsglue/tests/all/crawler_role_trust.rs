// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resolution of a crawler's `Role` against this emulator's own
//! IAM records.
//!
//! The rule under test: a crawl is performed BY Glue acting as the crawler's `Role`, so a
//! crawler write resolves that role BEFORE it judges any other member of the request and
//! refuses it with `InvalidInputException` (HTTP 400) — persisting nothing — when IAM
//! ANSWERED that Glue cannot assume it (this account holds no such role, or the trust
//! document the role carries does not admit the Glue service principal). A role nothing can
//! be shown about is not refused: an emulator with no IAM to ask refuses nothing, which is
//! why every other suite in this crate, which mounts Glue alone, still creates crawlers
//! whose role no IAM record backs.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: what the
//! role's trust document says (which principal, which action, which effect, which casing,
//! how many statements), whether the account holds the role at all, and the form the `Role`
//! member names it by (an ARN of this account, a bare role name, another account's ARN).
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::GLUE_SERVICE_PRINCIPAL;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn, ComAmazonawsIamAwsidentitymanagementv20100508State as IamState,
    DEFAULT_PATH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, ok, ACCOUNT_ID};

/// The whole `Message` a role this service cannot assume is refused with — the service's
/// fixed template for this check, written out here rather than read from the crate's own
/// constant so this suite pins the wording.
const ROLE_NOT_ASSUMABLE: &str =
    "Service is unable to assume provided role. Please verify role's TrustPolicy";

/// An emulator with Glue AND IAM mounted on one shared engine, plus that engine: the role
/// resolution asks IAM over the emulator's own in-process callback, so IAM has to be
/// mounted for it to answer at all.
///
/// Shared with [`crate::execution_role_admission`], which holds the other writes that run as
/// a caller-supplied role to the same resolution, so both suites stand the same emulator up.
pub(crate) fn emulator_with_iam() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Seed `role_name` into the shared engine's IAM store carrying `trust` as its
/// `AssumeRolePolicyDocument` — the record `CreateRole` writes — and answer the ARN IAM
/// holds it under.
pub(crate) fn seed_role(
    engine: &Arc<engine::InMemoryEngine>,
    role_name: &str,
    trust: Option<String>,
) -> String {
    let state = engine.state::<IamState>();
    let mut store = state.lock();
    let arn = role_arn(DEFAULT_PATH, role_name);
    store.roles.insert(
        name_key(role_name),
        Role {
            path: DEFAULT_PATH.to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROA{}", role_name.to_uppercase()),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            assume_role_policy_document: trust,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    arn
}

/// A trust document with one statement: `effect` for `principal` performing `action`.
pub(crate) fn trust(effect: &str, principal: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }],
    })
    .to_string()
}

/// The trust document a role Glue can assume carries.
pub(crate) fn glue_trust() -> String {
    trust("Allow", GLUE_SERVICE_PRINCIPAL, "sts:AssumeRole")
}

/// A `CreateCrawler` body naming one S3 crawl target and `role`.
fn crawler_body(name: &str, role: &str) -> Value {
    json!({
        "Name": name,
        "Role": role,
        "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] },
    })
}

/// Assert that `name` names no crawler: a refused create must have persisted nothing.
async fn assert_absent(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = call(router, "GetCrawler", &json!({ "Name": name })).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a refused create must store no crawler, but GetCrawler answered \
         {status} with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case {case:?}: expected the crawler to be absent, got {body}"
    );
}

/// What one row asks of the role resolution.
struct Row {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The role the account holds under the row's role name, as its trust document — or
    /// `None` for a row whose role the account does not hold at all.
    held: Option<Option<String>>,
    /// Whether the create must be admitted.
    admitted: bool,
}

/// Every form of trust document, and every absence of one, decides whether a crawler can be
/// created with that role — and a refusal persists no crawler.
///
/// The rows differ along what the rule generalises over: which principal the document
/// admits, which action it names, which effect it states, the casing it names them in, how
/// many statements it carries, whether it is readable as a policy at all, whether it is
/// present on the role, and whether the account holds the role.
#[tokio::test]
async fn a_crawler_is_created_only_for_a_role_iam_shows_glue_can_assume() {
    let (router, engine) = emulator_with_iam();

    let rows: Vec<Row> = vec![
        Row {
            case: "a trust document admitting the Glue principal",
            held: Some(Some(glue_trust())),
            admitted: true,
        },
        Row {
            case: "a service wildcard in the action",
            held: Some(Some(trust("Allow", GLUE_SERVICE_PRINCIPAL, "sts:*"))),
            admitted: true,
        },
        Row {
            case: "the principal and the action named in another casing",
            held: Some(Some(trust("Allow", "GLUE.amazonaws.com", "STS:AssumeRole"))),
            admitted: true,
        },
        Row {
            case: "a document whose second statement admits the Glue principal",
            held: Some(Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": "ec2.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        },
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": GLUE_SERVICE_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        },
                    ],
                })
                .to_string(),
            )),
            admitted: true,
        },
        Row {
            case: "a trust document admitting another service only",
            held: Some(Some(trust(
                "Allow",
                "lambda.amazonaws.com",
                "sts:AssumeRole",
            ))),
            admitted: false,
        },
        Row {
            case: "a trust document naming another action",
            held: Some(Some(trust(
                "Allow",
                GLUE_SERVICE_PRINCIPAL,
                "s3:GetObject",
            ))),
            admitted: false,
        },
        Row {
            case: "a trust document explicitly denying the Glue principal",
            held: Some(Some(trust(
                "Deny",
                GLUE_SERVICE_PRINCIPAL,
                "sts:AssumeRole",
            ))),
            admitted: false,
        },
        Row {
            case: "a trust document carrying no statement",
            held: Some(Some(json!({ "Version": "2012-10-17" }).to_string())),
            admitted: false,
        },
        Row {
            case: "a trust document that is not a policy at all",
            held: Some(Some("this is not a policy".to_string())),
            admitted: false,
        },
        Row {
            case: "a role carrying no trust document",
            held: Some(None),
            admitted: false,
        },
        Row {
            case: "a role this account does not hold",
            held: None,
            admitted: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let role_name = format!("crawler-trust-role-{index}");
        let crawler = format!("crawler-trust-{index}");
        let role = match &row.held {
            Some(trust) => seed_role(&engine, &role_name, trust.clone()),
            // Not seeded, so IAM answers that this account holds no such role. The ARN is
            // the one IAM would hold it under.
            None => role_arn(DEFAULT_PATH, &role_name),
        };

        let (status, body) = call(&router, "CreateCrawler", &crawler_body(&crawler, &role)).await;
        if row.admitted {
            assert_eq!(
                status, 200,
                "case {:?}: expected the create to be admitted, got {status} with {body}",
                row.case
            );
            // Round trip: the crawler the create admitted is readable, carrying the role
            // it was created with.
            let read = ok(&router, "GetCrawler", &json!({ "Name": crawler })).await;
            assert_eq!(
                read["Crawler"]["Role"],
                json!(role),
                "case {:?}: expected the stored crawler to carry the role it was created \
                 with, got {read}",
                row.case
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {:?}: expected the create to be refused, got {status} with {body}",
            row.case
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {:?}: expected InvalidInputException, got {body}",
            row.case
        );
        assert_eq!(
            error_message(&body),
            ROLE_NOT_ASSUMABLE,
            "case {:?}: expected Message {ROLE_NOT_ASSUMABLE:?}, got {:?}",
            row.case,
            error_message(&body)
        );
        assert_absent(&router, row.case, &crawler).await;
    }
}

/// The same decision, over the forms the `Role` member may name a role by: `Role` is "the
/// IAM role or Amazon Resource Name (ARN) of an IAM role"
/// (`smithy_query operation=CreateCrawler`, `CreateCrawler$Role`), so a bare name and this
/// account's role ARN name the same role and are judged alike — while an ARN of ANOTHER
/// account names a role this emulator's IAM cannot be asked about, so nothing is shown and
/// nothing is refused.
#[tokio::test]
async fn the_role_member_is_resolved_whether_it_names_a_role_or_its_arn() {
    let (router, engine) = emulator_with_iam();
    seed_role(&engine, "glue-trusting-role", Some(glue_trust()));
    seed_role(
        &engine,
        "ec2-trusting-role",
        Some(trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")),
    );

    // (case, the `Role` value, whether the create must be admitted)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a bare role name, held and trusting Glue",
            "glue-trusting-role".to_string(),
            true,
        ),
        (
            "this account's ARN of the same role",
            role_arn(DEFAULT_PATH, "glue-trusting-role"),
            true,
        ),
        (
            "the same role under a service-role path, which IAM holds by name",
            format!("arn:aws:iam::{ACCOUNT_ID}:role/service-role/glue-trusting-role"),
            true,
        ),
        (
            "a bare role name, held but not trusting Glue",
            "ec2-trusting-role".to_string(),
            false,
        ),
        (
            "this account's ARN of that same role",
            role_arn(DEFAULT_PATH, "ec2-trusting-role"),
            false,
        ),
        (
            "a bare role name this account holds no role under",
            "no-such-crawler-role".to_string(),
            false,
        ),
        (
            // Another account's IAM is not this emulator's to ask, so nothing is shown
            // about the role and the create goes ahead.
            "another account's role ARN, which this IAM cannot be asked about",
            "arn:aws:iam::123456789012:role/their-crawler-role".to_string(),
            true,
        ),
    ];

    for (index, (case, role, admitted)) in rows.iter().enumerate() {
        let crawler = format!("crawler-form-{index}");
        let (status, body) = call(&router, "CreateCrawler", &crawler_body(&crawler, role)).await;
        if *admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be admitted, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            ROLE_NOT_ASSUMABLE,
            "case {case:?}: expected Message {ROLE_NOT_ASSUMABLE:?}, got {:?}",
            error_message(&body)
        );
        assert_absent(&router, case, &crawler).await;
    }
}

/// The role is resolved BEFORE the request's other members, so a request that is wrong in
/// two ways at once reports the role.
///
/// The rows differ along WHICH other member is also wrong: a `Targets` naming no crawl
/// target (absent, present-but-empty, and every list empty), a `Schedule` that is no cron
/// expression, and a `CatalogId` this account holds no Data Catalog under — each of which
/// this operation refuses on its own when the role is assumable.
#[tokio::test]
async fn the_role_is_judged_before_every_other_member_of_the_request() {
    let (router, engine) = emulator_with_iam();
    let assumable = seed_role(&engine, "ordering-glue-role", Some(glue_trust()));
    let unassumable = seed_role(
        &engine,
        "ordering-ec2-role",
        Some(trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")),
    );
    let targets = json!({ "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] });

    // (case, the members to send beside `Name` and `Role`, the Message the operation
    // answers when the role IS assumable)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "no Targets member at all",
            json!({ "Targets": {} }),
            "Targets must specify at least one crawl target",
        ),
        (
            "a Targets whose S3 list is empty",
            json!({ "Targets": { "S3Targets": [] } }),
            "Targets must specify at least one crawl target",
        ),
        (
            "a Targets whose every list is empty",
            json!({ "Targets": { "S3Targets": [], "JdbcTargets": [], "DynamoDBTargets": [] } }),
            "Targets must specify at least one crawl target",
        ),
        (
            "a Schedule that is no cron expression",
            json!({ "Targets": targets, "Schedule": "every monday" }),
            "Schedule every monday is not a cron expression of the form \
             cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)",
        ),
        (
            "a CatalogId this account holds no Data Catalog under",
            json!({ "Targets": targets, "CatalogId": "no-such-catalog" }),
            "CatalogId no-such-catalog is not a Data Catalog in this account",
        ),
    ];

    for (index, (case, members, other_message)) in rows.iter().enumerate() {
        // With the un-assumable role, the ROLE is what the refusal reports.
        let mut request = members.clone();
        request["Name"] = json!(format!("ordering-{index}"));
        request["Role"] = json!(unassumable);
        let (status, body) = call(&router, "CreateCrawler", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            ROLE_NOT_ASSUMABLE,
            "case {case:?}: the role is judged first, so expected Message \
             {ROLE_NOT_ASSUMABLE:?}, got {:?}",
            error_message(&body)
        );

        // With the assumable role, the SAME request is refused for the other member —
        // which is what makes the row's first assertion about precedence and not about
        // the other check having gone away.
        let mut request = members.clone();
        request["Name"] = json!(format!("ordering-ok-{index}"));
        request["Role"] = json!(assumable);
        let (status, body) = call(&router, "CreateCrawler", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused for the other member, got \
             {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            *other_message,
            "case {case:?}: expected Message {other_message:?}, got {:?}",
            error_message(&body)
        );
    }
}

/// `UpdateCrawler` resolves a `Role` it would REPLACE the stored one with the same way the
/// create does: an un-assumable role is refused and the stored crawler keeps the role it
/// had, while an update that names no role at all is untouched by the resolution.
///
/// The rows differ along which role the update names and whether it names one at all.
#[tokio::test]
async fn an_update_that_replaces_the_role_is_held_to_the_same_resolution() {
    let (router, engine) = emulator_with_iam();
    let assumable = seed_role(&engine, "update-glue-role", Some(glue_trust()));
    let other_assumable = seed_role(&engine, "update-glue-role-two", Some(glue_trust()));
    let unassumable = seed_role(
        &engine,
        "update-ec2-role",
        Some(trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")),
    );
    let absent = role_arn(DEFAULT_PATH, "update-absent-role");

    // (case, the members the update carries beside `Name`, the role the crawler must carry
    // afterwards)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a replacement role Glue can assume",
            json!({ "Role": other_assumable }),
            other_assumable.clone(),
        ),
        (
            "a replacement role Glue cannot assume",
            json!({ "Role": unassumable }),
            assumable.clone(),
        ),
        (
            "a replacement role this account does not hold",
            json!({ "Role": absent }),
            assumable.clone(),
        ),
        (
            "an update that names no role",
            json!({ "Description": "changed" }),
            assumable.clone(),
        ),
    ];

    for (index, (case, members, expected_role)) in rows.iter().enumerate() {
        let crawler = format!("update-role-{index}");
        ok(&router, "CreateCrawler", &crawler_body(&crawler, &assumable)).await;

        let mut request = members.clone();
        request["Name"] = json!(crawler);
        let (status, body) = call(&router, "UpdateCrawler", &request).await;
        let refused = *expected_role == assumable && members.get("Role").is_some();
        if refused {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the update to be refused, got {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                ROLE_NOT_ASSUMABLE,
                "case {case:?}: expected Message {ROLE_NOT_ASSUMABLE:?}, got {:?}",
                error_message(&body)
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be admitted, got {status} with {body}"
            );
        }

        let read = ok(&router, "GetCrawler", &json!({ "Name": crawler })).await;
        assert_eq!(
            read["Crawler"]["Role"],
            json!(expected_role),
            "case {case:?}: expected the stored crawler to carry {expected_role:?}, got {read}"
        );
    }
}

/// An update of a crawler no name resolves is the crawler MISS, whatever role it names: the
/// crawler this request addresses is resolved before the members it would write.
#[tokio::test]
async fn an_update_of_an_absent_crawler_is_a_miss_whatever_role_it_names() {
    let (router, engine) = emulator_with_iam();
    let unassumable = seed_role(
        &engine,
        "miss-ec2-role",
        Some(trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole")),
    );

    for (case, role) in [
        ("an un-assumable role", unassumable.as_str()),
        ("a role this account does not hold", "no-such-role"),
    ] {
        let (status, body) = call(
            &router,
            "UpdateCrawler",
            &json!({ "Name": "no-such-crawler", "Role": role }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case:?}: expected the crawler miss, got {body}"
        );
    }
}
