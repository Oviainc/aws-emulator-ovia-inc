// SPDX-License-Identifier: CC0-1.0

//! `CreateHarvestJob` accepts a submission only when it can be carried out.
//!
//! `tests/all/harvest_jobs.rs` covers what a submitted job IS — the record, the clock-owed
//! status transition, the listing, the id and member rules. This suite covers the two
//! rejections that decide whether a submission is accepted at all:
//!
//! 1. WINDOW. A harvest window is an interval of media, so its `EndTime` has to be LATER
//!    than its `StartTime`: a submission whose window is empty or inverted is refused with
//!    `UnprocessableEntityException` (422) keyed by `endTime` and persists no job, so a
//!    following `DescribeHarvestJob` is `NotFoundException` (404). `StartTime` and
//!    `EndTime` are modelled as plain `__string`, so text that is not an ISO-8601 instant
//!    at all is refused as that member's own failure.
//! 2. DESTINATION ROLE. MediaPackage writes the harvested content to the caller's bucket
//!    AS the role `S3Destination.RoleArn` names, so a submission naming a role the service
//!    could not act as is refused the same way and persists nothing: one whose trust policy
//!    withholds `sts:AssumeRole` from `mediapackage.amazonaws.com`, and one of THIS account
//!    that the account's IAM records hold no role for at all — there is no role to assume,
//!    so the harvest could never be written. The role is resolved against the account's IAM
//!    records — the very records `CreateRole` wrote, in the same engine — and an ARN of
//!    ANOTHER account stays exempt, because this emulator holds no record of another
//!    account's roles and so establishes nothing about one.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — over ONE engine carrying both MediaPackage's store and IAM's, so
//! the trust check reads roles a real `CreateRole` call created over IAM's own awsQuery
//! wire.
//!
//! The rows differ along what each rule generalises over: the window's direction and span,
//! how the two instants are spelled, which member is unreadable, and the form of the trust
//! policy (another principal, an explicit deny, a policy granting nothing, a wildcard
//! action, a list of principals), plus the two roles no `CreateRole` created: one of this
//! account and one of another.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The crate's own wire harness, reused verbatim: `call` builds and reads a MediaPackage
/// request exactly as every other suite in this target does, so the two cannot drift.
/// Only the ROUTER differs here, and `call` takes it as an argument. `urlencode` is this
/// target's one percent-encoder, which is the escaping IAM's awsQuery body needs too.
use crate::channel_lifecycle::{call, error_code, ACCOUNT, OTHER_ACCOUNT};
use crate::resource_tags::urlencode;

/// The channel, endpoint and bucket every row harvests from and to.
const CHANNEL_ID: &str = "harvest-channel";
const ENDPOINT_ID: &str = "harvest-endpoint";
const BUCKET: &str = "harvest-destination";

/// The service principal a harvest job's destination role must admit
/// (`crate::state::SERVICE_PRINCIPAL`).
///
/// Shared with `tests/all/origin_endpoint_contents.rs`, which asserts the same rule for the
/// roles an origin-endpoint write names.
pub(crate) const PRINCIPAL: &str = "mediapackage.amazonaws.com";

/// An emulator carrying BOTH services over one engine, with request authentication off.
///
/// The crate's own builder (`tests/all/channel_lifecycle.rs`) serves MediaPackage alone;
/// this suite needs IAM in the same engine, because the rule under test is about records
/// IAM holds. Shared with `tests/all/origin_endpoint_contents.rs` for the same reason.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine.clone()));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One MediaPackage round trip: the HTTP status and the parsed JSON body.
///
/// The crate's shared `call` ([`crate::channel_lifecycle`]), given THIS suite's router.
async fn mp(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call(router, method, uri, body).await
}

/// Create an IAM role carrying `document` as its trust policy, and answer the ARN IAM
/// minted for it — read off the response rather than constructed, so the ARN the harvest
/// job names is the one IAM actually holds.
pub(crate) async fn create_role(router: &MultiServiceRouter, name: &str, document: &str) -> String {
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        urlencode(name),
        urlencode(document)
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
    let xml = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
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

/// A trust policy with one statement, written as a client would send it.
pub(crate) fn policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// A trust policy that lets MediaPackage assume the role, for the rows whose subject is
/// the WINDOW rather than the role.
fn assumable() -> String {
    policy("Allow", "\"sts:AssumeRole\"", &format!("\"{PRINCIPAL}\""))
}

/// The channel and origin endpoint every harvest job below exports from.
async fn provision(router: &MultiServiceRouter) {
    let (status, body) = mp(
        router,
        "POST",
        "/channels",
        Some(&json!({ "id": CHANNEL_ID })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateChannel expected 200, got {status} {body:?}"
    );
    let (status, body) = mp(
        router,
        "POST",
        "/origin_endpoints",
        Some(&json!({
            "id": ENDPOINT_ID,
            "channelId": CHANNEL_ID,
            "hlsPackage": {},
            "startoverWindowSeconds": 86_400
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint expected 200, got {status} {body:?}"
    );
}

/// A `CreateHarvestJob` body.
fn submission(id: &str, start: &str, end: &str, role_arn: &str) -> Value {
    json!({
        "id": id,
        "originEndpointId": ENDPOINT_ID,
        "startTime": start,
        "endTime": end,
        "s3Destination": {
            "bucketName": BUCKET,
            "manifestKey": "harvests/index.m3u8",
            "roleArn": role_arn
        }
    })
}

/// Submit a harvest job.
async fn submit(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    mp(router, "POST", "/harvest_jobs", Some(body)).await
}

/// Read a harvest job back.
async fn describe(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    mp(router, "GET", &format!("/harvest_jobs/{id}"), None).await
}

/// Assert `refusal` is this service's 422 keyed by `member`.
fn assert_unprocessable(case: &str, status: u16, refusal: &Value, member: &str) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    assert!(
        refusal.pointer(&format!("/errors/{member}")).is_some(),
        "case {case:?}: the refusal must be keyed by {member:?}, got {refusal:?}"
    );
}

/// Assert no harvest job is stored under `id`.
async fn assert_not_stored(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, described) = describe(router, id).await;
    assert_eq!(
        status, 404,
        "case {case:?}: a refused submission must persist no job, got {status} {described:?}"
    );
    assert_eq!(
        error_code(&described),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {described:?}"
    );
}

/// One window row: the two instants as the request spells them, and either the member a
/// refusal is keyed by or `None` when the window is harvestable.
struct Window {
    case: &'static str,
    start: &'static str,
    end: &'static str,
    refused_member: Option<&'static str>,
}

/// Rows spanning the window's direction and span, two spellings of the same instant, and
/// each of the two members being unreadable.
const WINDOWS: &[Window] = &[
    Window {
        case: "a one-second window",
        start: "2026-01-01T00:00:00Z",
        end: "2026-01-01T00:00:01Z",
        refused_member: None,
    },
    Window {
        case: "an hour-long window",
        start: "2026-03-14T09:00:00Z",
        end: "2026-03-14T10:00:00Z",
        refused_member: None,
    },
    Window {
        case: "a window spanning a month boundary",
        start: "2026-01-31T23:00:00Z",
        end: "2026-02-01T01:00:00Z",
        refused_member: None,
    },
    Window {
        case: "a window whose bounds carry fractional seconds",
        start: "2026-01-01T00:00:00.250Z",
        end: "2026-01-01T00:00:30.750Z",
        refused_member: None,
    },
    Window {
        case: "a window whose bounds carry a numeric UTC offset",
        start: "2026-02-03T10:15:00+00:00",
        end: "2026-02-03T10:45:30+00:00",
        refused_member: None,
    },
    Window {
        case: "an end later than the start once its offset is applied",
        start: "2026-01-01T00:00:00Z",
        end: "2026-01-01T03:00:00+02:00",
        refused_member: None,
    },
    Window {
        case: "two spellings of the same instant, one with an offset",
        start: "2026-01-01T02:00:00+02:00",
        end: "2026-01-01T00:00:00Z",
        refused_member: Some("endTime"),
    },
    Window {
        case: "an end earlier than the start once its offset is applied",
        start: "2026-01-01T00:00:00Z",
        end: "2025-12-31T23:00:00+02:00",
        refused_member: Some("endTime"),
    },
    Window {
        case: "a sub-second window",
        start: "2026-01-01T00:00:00.000Z",
        end: "2026-01-01T00:00:00.500Z",
        refused_member: None,
    },
    Window {
        case: "bounds carrying no zone suffix, both read as UTC",
        start: "2026-01-01T00:00:00",
        end: "2026-01-01T01:00:00",
        refused_member: None,
    },
    Window {
        case: "an inverted window whose bounds carry no zone suffix",
        start: "2026-01-01T01:00:00",
        end: "2026-01-01T00:00:00",
        refused_member: Some("endTime"),
    },
    Window {
        case: "an empty window",
        start: "2026-01-01T00:00:00Z",
        end: "2026-01-01T00:00:00Z",
        refused_member: Some("endTime"),
    },
    Window {
        case: "an empty window whose bounds are spelled differently",
        start: "2026-01-01T00:00:00Z",
        end: "2026-01-01T00:00:00.000Z",
        refused_member: Some("endTime"),
    },
    Window {
        case: "an end one second before the start",
        start: "2026-01-01T00:00:01Z",
        end: "2026-01-01T00:00:00Z",
        refused_member: Some("endTime"),
    },
    Window {
        case: "an end a minute before the start, as the issue reported",
        start: "2026-01-01T12:00:00Z",
        end: "2026-01-01T11:59:00Z",
        refused_member: Some("endTime"),
    },
    Window {
        case: "an end a day before the start",
        start: "2026-01-02T00:00:00Z",
        end: "2026-01-01T00:00:00Z",
        refused_member: Some("endTime"),
    },
    Window {
        case: "a start that is not an instant",
        start: "yesterday",
        end: "2026-01-01T00:00:00Z",
        refused_member: Some("startTime"),
    },
    Window {
        case: "an end that is not an instant",
        start: "2026-01-01T00:00:00Z",
        end: "1767225600",
        refused_member: Some("endTime"),
    },
    // The rows a byte-indexed zone reader would have crashed on rather than refused: the
    // members are modelled `__string`, so arbitrary UTF-8 reaches the handler.
    Window {
        case: "a non-ASCII character where the offset goes",
        start: "2026-01-01T00:00:00+aéb",
        end: "2026-01-01T01:00:00Z",
        refused_member: Some("startTime"),
    },
    Window {
        case: "a multi-byte character inside an otherwise well-formed offset",
        start: "2026-01-01T00:00:00Z",
        end: "2026-01-01T01:00:00+0€",
        refused_member: Some("endTime"),
    },
    Window {
        case: "a window of nothing but multi-byte characters",
        start: "日時",
        end: "2026-01-01T01:00:00Z",
        refused_member: Some("startTime"),
    },
];

#[tokio::test]
async fn a_window_is_harvestable_only_when_its_end_is_later_than_its_start() {
    let router = emulator();
    provision(&router).await;
    let role = create_role(&router, "mediapackage-harvest-window", &assumable()).await;

    for (index, window) in WINDOWS.iter().enumerate() {
        let id = format!("window-{index}");
        let (status, answered) =
            submit(&router, &submission(&id, window.start, window.end, &role)).await;

        match window.refused_member {
            Some(member) => {
                assert_unprocessable(window.case, status, &answered, member);
                assert_not_stored(&router, window.case, &id).await;
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected 200, got {status} {answered:?}",
                    window.case
                );
                // The window's bounds are reported exactly as the request spelled them.
                for (member, sent) in [("startTime", window.start), ("endTime", window.end)] {
                    assert_eq!(
                        answered.get(member),
                        Some(&json!(sent)),
                        "case {:?}: {member} must be reported as sent, got {answered:?}",
                        window.case
                    );
                }
                // Persisted: the read renders the record the submission wrote.
                let (status, described) = describe(&router, &id).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: DescribeHarvestJob expected 200, got {status} {described:?}",
                    window.case
                );
                assert_eq!(
                    described, answered,
                    "case {:?}: the read must render the stored record, got {described:?}",
                    window.case
                );
            }
        }
    }
}

/// Which role a destination-role row names.
enum Named {
    /// A role this account HOLDS, created by `CreateRole` with this trust policy.
    Held(String),
    /// A role ARN of THIS account that no `CreateRole` ever created, so the account's IAM
    /// records hold no role for it.
    AbsentFromThisAccount,
    /// A role ARN of ANOTHER account, whose roles this emulator holds no record of.
    AnotherAccount,
}

/// One destination-role row: which role the submission names, and whether it is accepted.
struct Role {
    case: &'static str,
    named: Named,
    accepted: bool,
}

/// Rows spanning the forms a trust policy takes around ONE rule — whether it grants this
/// principal `sts:AssumeRole` — and the two ways an ARN names no role this emulator holds.
/// The reading of a policy document is property-tested where it lives
/// (`services/common/src/iam_trust_policy.rs`); these rows pin what MediaPackage DOES with
/// each answer.
fn roles() -> Vec<Role> {
    vec![
        Role {
            case: "a policy naming the principal and the action",
            named: Named::Held(assumable()),
            accepted: true,
        },
        Role {
            case: "a policy naming the principal among several",
            named: Named::Held(policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                &format!("[\"s3.amazonaws.com\",\"{PRINCIPAL}\"]"),
            )),
            accepted: true,
        },
        Role {
            case: "a policy allowing the principal every action",
            named: Named::Held(policy("Allow", "\"*\"", &format!("\"{PRINCIPAL}\""))),
            accepted: true,
        },
        Role {
            case: "a policy naming another service's principal only",
            named: Named::Held(policy(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"mediaconvert.amazonaws.com\"",
            )),
            accepted: false,
        },
        Role {
            case: "a policy explicitly denying the principal",
            named: Named::Held(policy(
                "Deny",
                "\"sts:AssumeRole\"",
                &format!("\"{PRINCIPAL}\""),
            )),
            accepted: false,
        },
        Role {
            case: "a policy naming an action that is not assuming the role",
            named: Named::Held(policy(
                "Allow",
                "\"s3:PutObject\"",
                &format!("\"{PRINCIPAL}\""),
            )),
            accepted: false,
        },
        Role {
            case: "a policy carrying no statement at all",
            named: Named::Held("{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string()),
            accepted: false,
        },
        Role {
            case: "a role ARN of this account that the IAM store holds no role for",
            named: Named::AbsentFromThisAccount,
            accepted: false,
        },
        Role {
            case: "a role ARN of another account",
            named: Named::AnotherAccount,
            accepted: true,
        },
    ]
}

#[tokio::test]
async fn a_destination_role_the_service_cannot_assume_is_refused_and_persists_nothing() {
    let router = emulator();
    provision(&router).await;

    for (index, role) in roles().into_iter().enumerate() {
        let role_arn = match &role.named {
            Named::Held(document) => {
                create_role(&router, &format!("mediapackage-harvest-{index}"), document).await
            }
            Named::AbsentFromThisAccount => {
                format!("arn:aws:iam::{ACCOUNT}:role/unknown-to-iam-{index}")
            }
            Named::AnotherAccount => {
                format!("arn:aws:iam::{OTHER_ACCOUNT}:role/unknown-to-iam-{index}")
            }
        };
        let id = format!("role-{index}");
        let (status, answered) = submit(
            &router,
            &submission(
                &id,
                "2026-01-01T00:00:00Z",
                "2026-01-01T01:00:00Z",
                &role_arn,
            ),
        )
        .await;

        if role.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: expected 200, got {status} {answered:?}",
                role.case
            );
            assert_eq!(
                answered.pointer("/s3Destination/roleArn"),
                Some(&json!(role_arn)),
                "case {:?}: the destination must be stored as sent, got {answered:?}",
                role.case
            );
            let (status, described) = describe(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {:?}: DescribeHarvestJob expected 200, got {status} {described:?}",
                role.case
            );
        } else {
            assert_unprocessable(role.case, status, &answered, "s3Destination.roleArn");
            assert_not_stored(&router, role.case, &id).await;
        }
    }
}
