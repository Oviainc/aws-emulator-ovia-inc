// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the resolution of an investigation group's `roleArn`
//! against this emulator's own IAM records.
//!
//! The rule under test: a group gathers its investigation data AS the role `roleArn`
//! names, so every write of that member resolves the role BEFORE it persists anything
//! and refuses the request with the modelled `ValidationException` (HTTP 400) — storing
//! nothing — when the value names a role this service could not assume. Two answers
//! ground that refusal: the value names no IAM role of THIS account at all (a bucket, a
//! user, another account's role), or this account's IAM answered that it holds no role
//! under that name or that the role's trust policy does not grant
//! `aiops.amazonaws.com` `sts:AssumeRole`. A role nothing can be shown about is NOT
//! refused — an emulator with no IAM to ask refuses nothing, which is why every other
//! suite in this crate, which mounts this service alone, still creates groups whose role
//! no IAM record backs.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: what
//! the role's trust document says (which principal, which action, which effect, which
//! casing, how many statements, whether it is a policy at all), whether the account
//! holds the role, and the form the member names it by (an ARN of this account, a role
//! under a path, another partition, another account, another IAM entity type, a
//! non-IAM ARN). Every call goes over the wire through the production
//! `MultiServiceRouter`, so what is asserted is HTTP behaviour and not a direct call
//! into a handler.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state::{ROLE_NOT_ASSUMABLE, SERVICE_PRINCIPAL};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// An account OTHER than the investigation account's.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// The region every request below signs for. One group per Region is all an account may
/// hold, so each row builds its own emulator rather than sharing this one.
const REGION: &str = "us-west-2";

/// The modelled error a refused write reports
/// (`smithy_query(query="shape",shape="ValidationException")`: `@httpError 400`).
const VALIDATION: &str = "ValidationException";

/// An emulator with this service AND IAM mounted on ONE engine, plus that engine's
/// router: the role resolution asks IAM over the emulator's own in-process callback, so
/// IAM has to be mounted for it to answer at all.
fn emulator_with_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `aiops` in [`REGION`], the signal the router routes on and the
/// handlers read the region and the caller principal from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{REGION}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one aiops call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` a failed call carried.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Percent-encode one value of IAM's `application/x-www-form-urlencoded` awsQuery body.
fn form_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Create an IAM role named `name` under `path` carrying `trust` as its
/// `AssumeRolePolicyDocument`, through IAM's OWN awsQuery wire, and answer the ARN IAM
/// minted for it — read off the response rather than constructed, so the value the
/// aiops request names is the one IAM actually holds.
async fn create_role(
    router: &MultiServiceRouter,
    path: &str,
    name: &str,
    trust: &str,
) -> String {
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&Path={}&RoleName={}&AssumeRolePolicyDocument={}",
        form_value(path),
        form_value(name),
        form_value(trust)
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 response body");
    assert_eq!(
        status, 200,
        "CreateRole {name:?} expected 200, got {status} {xml}"
    );
    let start = xml
        .find("<Arn>")
        .unwrap_or_else(|| panic!("CreateRole {name:?} returned no <Arn>: {xml}"))
        + "<Arn>".len();
    let end = xml[start..]
        .find("</Arn>")
        .unwrap_or_else(|| panic!("CreateRole {name:?} left <Arn> unclosed: {xml}"));
    xml[start..start + end].to_string()
}

/// A trust document with one statement: `effect` for the service `principal` performing
/// `action`.
fn trust(effect: &str, principal: &str, action: &str) -> String {
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

/// The trust document a role this service can assume carries.
fn service_trust() -> String {
    trust("Allow", SERVICE_PRINCIPAL, "sts:AssumeRole")
}

/// A `CreateInvestigationGroup` body naming `name` and `role_arn`.
fn create_body(name: &str, role_arn: &str) -> Value {
    json!({ "name": name, "roleArn": role_arn })
}

/// Assert that this Region holds no investigation group: a refused write must have
/// persisted nothing, and must not have consumed the Region's one-group quota either.
async fn assert_no_group(router: &MultiServiceRouter, case: &str) {
    let (status, listed) = call(router, "GET", "/investigationGroups", None).await;
    let groups = listed
        .get("investigationGroups")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        (status, groups),
        (200, 0),
        "case {case:?}: a refused create must store no group, got {status} / {listed:?}"
    );
}

/// What one row asks of the role resolution: the trust document the account holds the
/// row's role under, and whether the write must be admitted.
struct Trust {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The trust document the role carries.
    document: String,
    /// Whether the write must be admitted.
    admitted: bool,
}

/// The rows the TRUST half of the rule generalises over: every form of trust document,
/// and what each one decides.
fn trust_rows() -> Vec<Trust> {
    vec![
        Trust {
            case: "a trust document admitting this service's principal",
            document: service_trust(),
            admitted: true,
        },
        Trust {
            case: "a wildcard covering the assume action",
            document: trust("Allow", SERVICE_PRINCIPAL, "sts:*"),
            admitted: true,
        },
        Trust {
            case: "the principal and the action named in another casing",
            document: trust("Allow", "AIOPS.amazonaws.com", "STS:AssumeRole"),
            admitted: true,
        },
        Trust {
            case: "a second statement admitting this service's principal",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": "ec2.amazonaws.com" },
                        "Action": "sts:AssumeRole",
                    },
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    },
                ],
            })
            .to_string(),
            admitted: true,
        },
        Trust {
            case: "a trust document admitting another service only",
            document: trust("Allow", "lambda.amazonaws.com", "sts:AssumeRole"),
            admitted: false,
        },
        Trust {
            case: "an explicit deny for this service's principal",
            document: trust("Deny", SERVICE_PRINCIPAL, "sts:AssumeRole"),
            admitted: false,
        },
        Trust {
            case: "this service's principal named for another action",
            document: trust("Allow", SERVICE_PRINCIPAL, "sts:TagSession"),
            admitted: false,
        },
        Trust {
            case: "a statement granting the assume to an account principal, not a service",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
                    "Action": "sts:AssumeRole",
                }],
            })
            .to_string(),
            admitted: false,
        },
    ]
}

/// A create is admitted exactly for a role whose trust document this account's IAM shows
/// admits this service, and a refused one persists nothing.
#[tokio::test]
async fn a_group_is_created_only_for_a_role_iam_shows_this_service_can_assume() {
    for (index, row) in trust_rows().into_iter().enumerate() {
        // One emulator per row: an account holds one group per Region, so a row that
        // creates one needs its own store (and its own IAM store with its own role).
        let router = emulator_with_iam();
        let role = create_role(&router, "/", &format!("aiops-role-{index}"), &row.document).await;

        let name = "role-resolution-probe";
        let (status, body) = call(
            &router,
            "POST",
            "/investigationGroups",
            Some(&create_body(name, &role)),
        )
        .await;

        if row.admitted {
            assert_eq!(
                status, 201,
                "case {:?}: expected the create to be served, got {status} / {body:?}",
                row.case
            );
            // Round trip: the group the create minted carries the role it was created
            // with, read back through the implemented reader.
            let arn = body
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {:?}: no arn in {body:?}", row.case))
                .to_string();
            let (status, read) = call(
                &router,
                "GET",
                &format!("/investigationGroups/{}", label(&arn)),
                None,
            )
            .await;
            assert_eq!(
                (status, read.get("roleArn").and_then(Value::as_str)),
                (200, Some(role.as_str())),
                "case {:?}: the stored group must carry the accepted role, got {status} / \
                 {read:?}",
                row.case
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body).as_str()),
                (400, VALIDATION, ROLE_NOT_ASSUMABLE),
                "case {:?}: expected a 400 {VALIDATION} refusal, got {status} / {body:?}",
                row.case
            );
            assert_no_group(&router, row.case).await;
        }
    }
}

/// What one row asks of the FORM half of the rule: the value the member carries and
/// whether the write must be admitted.
struct Form {
    case: &'static str,
    /// The `roleArn` the request carries — built from the ARN IAM minted for the row's
    /// role, so a row that names a role of this account names the one IAM holds.
    role: String,
    admitted: bool,
}

/// Every form the member's modelled constraints admit (`@length 20..2048`, `^arn:.*$`)
/// decides whether the value names a role this service could act as.
#[tokio::test]
async fn only_a_value_naming_an_assumable_role_of_this_account_is_accepted() {
    // One emulator for the whole table: only the first admitted row creates a group, and
    // each row below asserts what it got, so the rows are built against ONE pair of
    // seeded roles — one under the default path, one under a path of its own.
    let router = emulator_with_iam();
    let held = create_role(&router, "/", "aiops-form-role", &service_trust()).await;
    let under_path =
        create_role(&router, "/service-role/", "aiops-path-role", &service_trust()).await;

    let rows: Vec<Form> = vec![
        Form {
            case: "the ARN IAM holds the role under",
            role: held.clone(),
            admitted: true,
        },
        Form {
            case: "a role under a path of its own",
            role: under_path.clone(),
            admitted: true,
        },
        Form {
            // An IAM ARN is partition-global and names the same role in every partition,
            // so the partition field is not what decides this.
            case: "the same role named in another partition",
            role: held.replace("arn:aws:", "arn:aws-cn:"),
            admitted: true,
        },
        Form {
            case: "a role this account does not hold",
            role: format!("arn:aws:iam::{ACCOUNT}:role/aiops-never-created"),
            admitted: false,
        },
        Form {
            case: "a role of another account",
            role: format!("arn:aws:iam::{FOREIGN_ACCOUNT}:role/aiops-form-role"),
            admitted: false,
        },
        Form {
            case: "an IAM user, which is not a role",
            role: format!("arn:aws:iam::{ACCOUNT}:user/aiops-form-role"),
            admitted: false,
        },
        Form {
            case: "an ARN of another vendor entirely",
            role: "arn:aws:s3:::aiops-investigation-data-bucket".to_string(),
            admitted: false,
        },
        Form {
            case: "an IAM ARN carrying a Region, which an IAM ARN does not",
            role: format!("arn:aws:iam:{REGION}:{ACCOUNT}:role/aiops-form-role"),
            admitted: false,
        },
        Form {
            case: "an ARN naming no resource at all",
            role: format!("arn:aws:iam::{ACCOUNT}:"),
            admitted: false,
        },
    ];

    let mut created: Option<String> = None;
    for (index, row) in rows.iter().enumerate() {
        let name = format!("form-probe-{index}");
        let (status, body) = call(
            &router,
            "POST",
            "/investigationGroups",
            Some(&create_body(&name, &row.role)),
        )
        .await;

        match (row.admitted, created.is_some()) {
            // The first admitted row creates the Region's one group; a later admitted
            // row is refused for the QUOTA, which is a different refusal and is what
            // shows the role was accepted.
            (true, false) => {
                assert_eq!(
                    status, 201,
                    "case {:?}: expected the create to be served, got {status} / {body:?}",
                    row.case
                );
                created = body
                    .get("arn")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                assert!(
                    created.is_some(),
                    "case {:?}: no arn in {body:?}",
                    row.case
                );
            }
            (true, true) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (402, "ServiceQuotaExceededException"),
                    "case {:?}: the role must be accepted and the Region's quota must be \
                     what refuses this create, got {status} / {body:?}",
                    row.case
                );
            }
            (false, _) => {
                assert_eq!(
                    (status, error_code(&body).as_str(), message(&body).as_str()),
                    (400, VALIDATION, ROLE_NOT_ASSUMABLE),
                    "case {:?}: expected a 400 {VALIDATION} refusal, got {status} / {body:?}",
                    row.case
                );
            }
        }
    }

    // The group the first admitted row created is the only one this Region holds, which
    // is what shows every refused row above stored nothing.
    let (status, listed) = call(&router, "GET", "/investigationGroups", None).await;
    let arns: Vec<&str> = listed
        .get("investigationGroups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.get("arn").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        (status, arns.as_slice()),
        (200, [created.as_deref().unwrap_or_default()].as_slice()),
        "the Region must hold exactly the group the accepted role created, got {status} / \
         {listed:?}"
    );
}

/// An update that REPLACES the role is held to the same resolution, and a refused one
/// leaves the stored role as it was.
#[tokio::test]
async fn an_update_replaces_the_role_only_with_one_this_service_can_assume() {
    let router = emulator_with_iam();
    let original = create_role(&router, "/", "aiops-update-role", &service_trust()).await;
    let replacement = create_role(&router, "/", "aiops-update-role-two", &service_trust()).await;
    let untrusted = create_role(
        &router,
        "/",
        "aiops-update-role-untrusted",
        &trust("Allow", "ec2.amazonaws.com", "sts:AssumeRole"),
    )
    .await;

    let (status, created) = call(
        &router,
        "POST",
        "/investigationGroups",
        Some(&create_body("update-probe", &original)),
    )
    .await;
    assert_eq!(
        status, 201,
        "creating the group: expected 201, got {status} / {created:?}"
    );
    let arn = created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no arn in {created:?}"))
        .to_string();
    let uri = format!("/investigationGroups/{}", label(&arn));

    // (case, the replacement the request names, whether it must be applied)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a replacement this service cannot assume",
            untrusted.clone(),
            false,
        ),
        (
            "a replacement this account does not hold",
            format!("arn:aws:iam::{ACCOUNT}:role/aiops-never-created"),
            false,
        ),
        (
            "a replacement of another account",
            format!("arn:aws:iam::{FOREIGN_ACCOUNT}:role/aiops-update-role"),
            false,
        ),
        (
            "a replacement this service can assume",
            replacement.clone(),
            true,
        ),
    ];

    let mut expected = original.clone();
    for (case, role, applied) in rows {
        let (status, body) = call(
            &router,
            "PATCH",
            &uri,
            Some(&json!({ "roleArn": role })),
        )
        .await;
        if applied {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status} / {body:?}"
            );
            expected = role.clone();
        } else {
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body).as_str()),
                (400, VALIDATION, ROLE_NOT_ASSUMABLE),
                "case {case:?}: expected a 400 {VALIDATION} refusal, got {status} / {body:?}"
            );
        }
        let (status, read) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            (status, read.get("roleArn").and_then(Value::as_str)),
            (200, Some(expected.as_str())),
            "case {case:?}: expected the stored role to be {expected:?}, got {status} / {read:?}"
        );
    }

    // An update that names no role at all touches none of this: the stored role is the
    // one the last accepted write left behind.
    let (status, body) = call(
        &router,
        "PATCH",
        &uri,
        Some(&json!({ "isCloudTrailEventHistoryEnabled": false })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an update naming no role\": expected 200, got {status} / {body:?}"
    );
    let (status, read) = call(&router, "GET", &uri, None).await;
    assert_eq!(
        (status, read.get("roleArn").and_then(Value::as_str)),
        (200, Some(expected.as_str())),
        "case \"an update naming no role\": expected the stored role to be unchanged, got \
         {status} / {read:?}"
    );
}

/// An update addressed at a group this Region does not hold is still the modelled 404,
/// whatever role the request names: the group is resolved before the role is.
#[tokio::test]
async fn an_update_of_an_absent_group_is_reported_as_the_absent_group() {
    let router = emulator_with_iam();

    for (case, role) in [
        (
            "a role this account does not hold",
            format!("arn:aws:iam::{ACCOUNT}:role/aiops-never-created"),
        ),
        (
            "a role of another account",
            format!("arn:aws:iam::{FOREIGN_ACCOUNT}:role/aiops-never-created"),
        ),
    ] {
        let absent = format!("arn:aws:aiops:{REGION}:{ACCOUNT}:investigation-group/AAAAAAAAAAAAAAAA");
        let (status, body) = call(
            &router,
            "PATCH",
            &format!("/investigationGroups/{}", label(&absent)),
            Some(&json!({ "roleArn": role })),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected the absent group to be reported, got {status} / {body:?}"
        );
    }
}
