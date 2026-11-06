// SPDX-License-Identifier: CC0-1.0

//! The WORDING of a MediaPackage refusal, over every request that can provoke it.
//!
//! RULE under test: the `message` of a refusal is the text MediaPackage itself states for
//! that error class — its fixed part verbatim, and its variable parts taken from the
//! request — so a client that branches on the sentence AWS states recognises the emulator's
//! answer. Two classes carry that rule here, and nothing about either sentence comes from
//! the offending value:
//!
//! 1. A ROLE THIS SERVICE MAY NOT ASSUME. A member naming a role whose trust policy
//!    withholds `sts:AssumeRole` from `mediapackage.amazonaws.com` is refused with
//!    `UnprocessableEntityException` (422,
//!    `smithy_query(query="operation",operation="CreateHarvestJob,CreateOriginEndpoint,UpdateOriginEndpoint")`)
//!    stating `<member> must be assumable by MediaPackage. Please ensure that its trust
//!    policy includes mediapackage.amazonaws.com`. The ONE variable part is the wire path
//!    of the member at fault (`S3Destination$RoleArn` is `@jsonName("roleArn")` inside
//!    `s3Destination`, and so on), so the rows span every member of an accepted request
//!    that carries such a role, on every operation that pre-flights one, crossed with each
//!    form a trust policy takes around that one question.
//! 2. A `nextToken` THIS SERVICE DID NOT ISSUE. Every paginated listing
//!    (`ListChannels`, `ListOriginEndpoints`, `ListHarvestJobs` — each `@paginated` with
//!    `NextToken` bound `@httpQuery("nextToken")`) refuses it with the same 422 stating
//!    `Invalid NextToken`, which is the whole sentence: it names neither the token nor the
//!    listing, so the rows span all three listings crossed with every way a token can fail
//!    to be one that enumeration issued, and each asserts the SAME text.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — over ONE engine carrying both MediaPackage's store and IAM's, so
//! the roles judged are the records `CreateRole` really wrote. The harness is the one
//! `tests/all/channel_lifecycle.rs` and `tests/all/harvest_job_rejections.rs` own.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, error_code, CROSS_ACCOUNT_HARVEST_ROLE, REGION};
use crate::harvest_job_rejections::{create_role, emulator, policy, PRINCIPAL};

/// The channel every endpoint below packages.
const CHANNEL_ID: &str = "wording-channel";

/// A startover window that retains a day of content, so a harvest of the endpoint can be
/// carried out and a `CreateHarvestJob` row reaches its destination-role pre-flight.
const HARVESTABLE_WINDOW_SECONDS: i32 = 86_400;

/// An ARN satisfying `state::SECRETS_MANAGER_SECRET_ARN`, for the rows whose subject is the
/// CDN authorization ROLE beside it (the secret member is judged first and must pass).
const GOOD_SECRET: &str = "arn:aws:secretsmanager:eu-west-1:000000000000:secret:cdn-AbCdEf";

/// The sentence MediaPackage states for a member naming a role it may not act as, for the
/// member at `path`: fixed text, with the wire path of the offending member as its only
/// variable part.
fn unassumable_role_message(path: &str) -> String {
    format!(
        "{path} must be assumable by MediaPackage. Please ensure that its trust policy \
         includes {PRINCIPAL}"
    )
}

/// The whole sentence MediaPackage states for a `nextToken` it did not issue.
const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid NextToken";

/// Assert `refusal` is the modelled 422 whose `message` is exactly `message`.
fn assert_sentence(case: &str, status: u16, refusal: &Value, message: &str) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    assert_eq!(
        refusal.get("message").and_then(Value::as_str),
        Some(message),
        "case {case:?}: expected message {message:?}, got {refusal:?}"
    );
}

/// A SPEKE key provider carrying `role_arn`. `ResourceId`, `RoleArn`, `SystemIds` and `Url`
/// are all required on the shape
/// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#SpekeKeyProvider")`), so
/// the request reaches the role pre-flight instead of being refused for a missing member.
fn speke(role_arn: &str) -> Value {
    json!({
        "resourceId": "wording-endpoint",
        "roleArn": role_arn,
        "systemIds": ["81376844-f976-481e-a84e-cc25d39b0b33"],
        "url": "https://speke.example.com/v1/copyProtection"
    })
}

/// The forms a trust policy takes around ONE question — whether it grants this service
/// `sts:AssumeRole`. How a document is READ is tested where that reader lives
/// (`services/common/src/iam_trust_policy.rs`); these rows only need the "no" answer,
/// reached four different ways.
fn withholding_policies() -> Vec<(&'static str, String)> {
    vec![
        (
            "another service's principal only",
            policy("Allow", "\"sts:AssumeRole\"", "\"mediatailor.amazonaws.com\""),
        ),
        (
            "an explicit deny of this principal",
            policy("Deny", "\"sts:AssumeRole\"", &format!("\"{PRINCIPAL}\"")),
        ),
        (
            "no statement at all",
            "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
        ),
        (
            "an action that is not assuming the role",
            policy("Allow", "\"s3:PutObject\"", &format!("\"{PRINCIPAL}\"")),
        ),
    ]
}

/// Create the channel every endpoint below packages.
async fn create_channel(router: &MultiServiceRouter) {
    let (status, body) = call(
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
}

/// Create an origin endpoint of `id` that a harvest can export, asserting it was stored.
async fn create_endpoint(router: &MultiServiceRouter, id: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/origin_endpoints",
        Some(&json!({
            "id": id,
            "channelId": CHANNEL_ID,
            "hlsPackage": {},
            "startoverWindowSeconds": HARVESTABLE_WINDOW_SECONDS
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint {id:?} expected 200, got {status} {body:?}"
    );
}

/// One request that names a role MediaPackage has to assume: the operation it is sent to,
/// and the wire path of the member carrying the role.
struct RoleRequest {
    /// The member's wire path, which is the sentence's one variable part.
    path: &'static str,
    method: &'static str,
    /// The URI the request goes to, given the id this row uses.
    uri: fn(&str) -> String,
    /// The body, given that id and the role ARN under test.
    body: fn(&str, &str) -> Value,
    /// `true` when the row's endpoint must already exist (an update), `false` for a create.
    needs_endpoint: bool,
}

/// Every member of an accepted MediaPackage request that names a role the service itself
/// assumes, on every operation that pre-flights one: the harvest destination role, the CDN
/// authorization role, and the SPEKE key provider's role in two packaging formats — on the
/// create and, for the SPEKE role, on the update as well.
const ROLE_REQUESTS: &[RoleRequest] = &[
    RoleRequest {
        path: "s3Destination.roleArn",
        method: "POST",
        uri: |_| "/harvest_jobs".to_string(),
        body: |id, role| {
            json!({
                "id": format!("job-{id}"),
                "originEndpointId": id,
                "startTime": "2026-01-01T00:00:00Z",
                "endTime": "2026-01-01T01:00:00Z",
                "s3Destination": {
                    "bucketName": "wording-harvests",
                    "manifestKey": "harvests/index.m3u8",
                    "roleArn": role
                }
            })
        },
        needs_endpoint: true,
    },
    RoleRequest {
        path: "authorization.secretsRoleArn",
        method: "POST",
        uri: |_| "/origin_endpoints".to_string(),
        body: |id, role| {
            json!({
                "id": id,
                "channelId": CHANNEL_ID,
                "hlsPackage": {},
                "authorization": { "cdnIdentifierSecret": GOOD_SECRET, "secretsRoleArn": role }
            })
        },
        needs_endpoint: false,
    },
    RoleRequest {
        path: "hlsPackage.encryption.spekeKeyProvider.roleArn",
        method: "POST",
        uri: |_| "/origin_endpoints".to_string(),
        body: |id, role| {
            json!({
                "id": id,
                "channelId": CHANNEL_ID,
                "hlsPackage": { "encryption": { "spekeKeyProvider": speke(role) } }
            })
        },
        needs_endpoint: false,
    },
    RoleRequest {
        path: "dashPackage.encryption.spekeKeyProvider.roleArn",
        method: "POST",
        uri: |_| "/origin_endpoints".to_string(),
        body: |id, role| {
            json!({
                "id": id,
                "channelId": CHANNEL_ID,
                "dashPackage": { "encryption": { "spekeKeyProvider": speke(role) } }
            })
        },
        needs_endpoint: false,
    },
    RoleRequest {
        path: "hlsPackage.encryption.spekeKeyProvider.roleArn",
        method: "PUT",
        uri: |id| format!("/origin_endpoints/{id}"),
        body: |id, role| {
            json!({
                "id": id,
                "hlsPackage": { "encryption": { "spekeKeyProvider": speke(role) } }
            })
        },
        needs_endpoint: true,
    },
];

#[tokio::test]
async fn a_role_this_service_may_not_assume_is_refused_in_the_services_own_words() {
    let router = emulator();
    create_channel(&router).await;

    for (request_index, request) in ROLE_REQUESTS.iter().enumerate() {
        for (policy_index, (label, document)) in withholding_policies().into_iter().enumerate() {
            let id = format!("wording-{request_index}-{policy_index}");
            if request.needs_endpoint {
                create_endpoint(&router, &id).await;
            }
            let role_arn = create_role(
                &router,
                &format!("wording-role-{request_index}-{policy_index}"),
                &document,
            )
            .await;
            let case = format!(
                "{} {} naming a role whose policy grants {label}",
                request.method, request.path
            );
            let (status, refusal) = call(
                &router,
                request.method,
                &(request.uri)(&id),
                Some(&(request.body)(&id, &role_arn)),
            )
            .await;
            assert_sentence(
                &case,
                status,
                &refusal,
                &unassumable_role_message(request.path),
            );
            // The sentence carries nothing the caller sent: not the role, not the id.
            let message = refusal
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !message.contains(&role_arn) && !message.contains(&id),
                "case {case:?}: the sentence is fixed text, so it must name neither \
                 {role_arn:?} nor {id:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_role_the_service_may_assume_is_accepted_so_the_sentence_is_the_only_difference() {
    let router = emulator();
    create_channel(&router).await;
    let granting = policy("Allow", "\"sts:AssumeRole\"", &format!("\"{PRINCIPAL}\""));

    for (request_index, request) in ROLE_REQUESTS.iter().enumerate() {
        let id = format!("wording-granted-{request_index}");
        if request.needs_endpoint {
            create_endpoint(&router, &id).await;
        }
        let role_arn = create_role(
            &router,
            &format!("wording-granted-role-{request_index}"),
            &granting,
        )
        .await;
        let case = format!(
            "{} {} naming a role the service may act as",
            request.method, request.path
        );
        let (status, answered) = call(
            &router,
            request.method,
            &(request.uri)(&id),
            Some(&(request.body)(&id, &role_arn)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {answered:?}"
        );
    }
}

/// One paginated listing of this service: the collection it enumerates and the output
/// member its items arrive in
/// (`smithy_query(query="operation",operation="ListChannels,ListOriginEndpoints,ListHarvestJobs")`).
struct Listing {
    path: &'static str,
    items: &'static str,
}

const LISTINGS: &[Listing] = &[
    Listing {
        path: "/channels",
        items: "channels",
    },
    Listing {
        path: "/origin_endpoints",
        items: "originEndpoints",
    },
    Listing {
        path: "/harvest_jobs",
        items: "harvestJobs",
    },
];

/// The ids every collection below is seeded with — two per collection, so each listing
/// issues a real cursor at a page size of one.
const SEED_IDS: &[&str] = &["alpha", "beta"];

/// Seed two channels, two origin endpoints and two harvest jobs, so every listing has a
/// second page and so a cursor of its own.
async fn seed(router: &MultiServiceRouter) {
    for id in SEED_IDS {
        let (status, body) = call(
            router,
            "POST",
            "/channels",
            Some(&json!({ "id": id })),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding channel {id:?}: expected 200, got {status} {body:?}"
        );
        create_endpoint(router, id).await;
        let (status, body) = call(
            router,
            "POST",
            "/harvest_jobs",
            Some(&json!({
                "id": id,
                "originEndpointId": id,
                "startTime": "2026-01-01T00:00:00Z",
                "endTime": "2026-01-01T01:00:00Z",
                "s3Destination": {
                    "bucketName": "wording-harvests",
                    "manifestKey": "harvests/index.m3u8",
                    // A role of ANOTHER account is one this emulator holds no record of, so
                    // it establishes nothing about it and the submission is accepted (see
                    // `channel_lifecycle::CROSS_ACCOUNT_HARVEST_ROLE`).
                    "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
                }
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding harvest job {id:?}: expected 200, got {status} {body:?}"
        );
    }
}

/// The cursor `path` really handed out: the `nextToken` its first page of one carried.
async fn issued_token(router: &MultiServiceRouter, path: &str) -> String {
    let uri = format!("{path}?maxResults=1");
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {uri:?}: a first page must be served, got {status} {body:?}"
    );
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| panic!("case {uri:?}: a page with more to come carries a nextToken"))
}

#[tokio::test]
async fn a_next_token_the_service_did_not_issue_states_the_services_own_sentence() {
    let router = emulator();
    create_channel(&router).await;
    seed(&router).await;

    for listing in LISTINGS {
        // A cursor another listing really issued: well-formed, and still not one THIS
        // enumeration handed out.
        let foreign = LISTINGS
            .iter()
            .find(|other| other.path != listing.path)
            .expect("every listing has another beside it");
        let foreign_token = issued_token(&router, foreign.path).await;
        let rows: Vec<(String, String)> = vec![
            ("not hex at all".to_string(), "not-a-token".to_string()),
            ("odd-length hex".to_string(), "abc".to_string()),
            (
                "letters outside the hex alphabet".to_string(),
                "zz".to_string(),
            ),
            ("percent noise".to_string(), "%%%".to_string()),
            (
                "hex of a key the collection holds".to_string(),
                hex::encode(SEED_IDS[1]),
            ),
            (
                format!("a cursor {} issued", foreign.path),
                foreign_token.clone(),
            ),
            // Far longer than any key this service stores a record under.
            ("hex far longer than any key".to_string(), "ab".repeat(4_096)),
        ];

        for (label, token) in rows {
            let uri = format!("{}?nextToken={token}", listing.path);
            let case = format!("{}: {label}", listing.path);
            let (status, refusal) = call(&router, "GET", &uri, None).await;
            assert_sentence(&case, status, &refusal, INVALID_NEXT_TOKEN_MESSAGE);
            // The sentence is the WHOLE refusal: no key is reported with it, and the
            // refused token is not echoed into it.
            assert_eq!(
                refusal.get("errors"),
                None,
                "case {case:?}: the sentence states no key, got {refusal:?}"
            );
            assert_eq!(
                refusal.get(listing.items),
                None,
                "case {case:?}: a refused listing must serve no items, got {refusal:?}"
            );
        }

        // The control: the cursor this listing DID issue is served, so the sentence above
        // is a refusal of the token and not of pagination itself.
        let genuine = issued_token(&router, listing.path).await;
        let uri = format!("{}?maxResults=1&nextToken={genuine}", listing.path);
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {uri:?}: an issued cursor must be served, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn every_listing_states_the_same_sentence_in_every_region() {
    let router = emulator();
    create_channel(&router).await;
    seed(&router).await;

    // The sentence is the error class's own text, so it does not vary with the Region a
    // request is served in either: a cursor of one Region's enumeration offered in another
    // is refused with the same words.
    for listing in LISTINGS {
        let genuine = issued_token(&router, listing.path).await;
        let uri = format!("{}?nextToken={genuine}", listing.path);
        let case = format!("{} cursor of {REGION} offered in us-west-2", listing.path);
        let (status, refusal) =
            crate::channel_lifecycle::call_in(&router, "us-west-2", "GET", &uri, None).await;
        assert_sentence(&case, status, &refusal, INVALID_NEXT_TOKEN_MESSAGE);
    }
}
