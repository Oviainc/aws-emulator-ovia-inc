// SPDX-License-Identifier: CC0-1.0

//! What this service's refusals SAY, end to end over HTTP: the status, the
//! `code` a client branches on and the message AWS Signer words for each error
//! class.
//!
//! Every assertion goes through the router, so each case exercises routing, the
//! `@httpLabel` / `@httpQuery` bindings, deserialization, the handler, the
//! persisted state and serialization — the path a client takes. The rules under
//! test, each driven over a TABLE of inputs rather than one recorded request:
//!
//! * a refusal reproduces the service's own error contract — one sentence per
//!   condition, its variable parts taken from the request and from stored state,
//!   and a `code` naming the condition rather than the modelled shape;
//! * which actions a cross-account grant may carry is a fact about the PROFILE'S
//!   signing platform: the signing operation of the workflow a platform is not
//!   signed through is not grantable on its profiles;
//! * a request is judged on its own members BEFORE the resources it names are
//!   resolved, so a request that is both malformed and names a missing resource
//!   is refused as malformed;
//! * a profile's resource policy exists only while it holds a statement, so a
//!   read of a policy with no statements is a missing resource;
//! * a profile whose signatures have been revoked cannot be canceled.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A SigV4 credential scope naming this service for [`REGION`]: what routes a
/// request to signer.
const SIGNED_FOR: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account id every generated emulator presents.
const ACCOUNT_ID: &str = "000000000000";

/// An ACM certificate ARN a create names as its signing material.
const CERTIFICATE_ARN: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/12345678-1234-1234-1234-123456789012";

/// A published platform whose profiles need no `signingMaterial`, so a seeded
/// profile is one `PutSigningProfile` call.
const LAMBDA_PLATFORM: &str = "AWSLambda-SHA384-ECDSA";

/// The published CONTAINER-IMAGE platform — the other signing workflow, which
/// this issue's recorded requests never named.
const CONTAINER_PLATFORM: &str = "Notation-OCI-SHA384-ECDSA";

/// The `code` a read of a profile that holds no permission policy carries.
const ERROR_CODE_POLICY_NOT_FOUND: &str = "PolicyNotFound";

/// The `code` a write that lost the policy's optimistic-concurrency check
/// carries.
const ERROR_CODE_REVISION_MISMATCH: &str = "PolicyRevisionIdMismatch";

/// The `code` a refusal of a request's own values carries — the modelled shape's
/// own name.
const ERROR_CODE_VALIDATION: &str = "ValidationException";

/// The `code` a tagging call carrying something that is not a signing-profile ARN
/// carries — the modelled shape's own name.
const ERROR_CODE_BAD_REQUEST: &str = "BadRequestException";

/// The `code` a 404 for a signing job carries.
const ERROR_CODE_JOB_NOT_FOUND: &str = "JobNotFound";

/// The `code` a 404 for a platform the catalog does not publish carries.
const ERROR_CODE_PLATFORM_NOT_FOUND: &str = "PlatformNotFound";

/// The `code` the refusal of a cancel of a revoked profile carries: `Profile`
/// followed by the status that forbade the call.
const ERROR_CODE_PROFILE_REVOKED: &str = "ProfileRevoked";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the status, the body as JSON and the
/// `x-amzn-errortype` header restJson1 resolves an error SHAPE from.
struct Answer {
    status: u16,
    body: Value,
    error_type: Option<String>,
}

impl Answer {
    /// The modelled `code` member, or `None` when the body carries none.
    fn code(&self) -> Option<&str> {
        self.body["code"].as_str()
    }

    /// The modelled `message` member, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// `value` percent-encoded for ONE URI segment or query value: the members under
/// test are `@httpLabel`s and `@httpQuery`s admitting characters a URI cannot
/// hold literally, so everything outside the unreserved set is encoded, the way
/// an SDK sends it. Spelled out the way this crate's other suites spell it, so a
/// suite takes no dependency of its own.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header("authorization", SIGNED_FOR);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        body,
        error_type,
    }
}

/// A profile of `name` on `platform`, and the version `PutSigningProfile` minted.
async fn seed_profile(router: &MultiServiceRouter, name: &str, platform: &str) -> String {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({
            "platformId": platform,
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name} on {platform}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("profileVersion")
}

async fn add_permission(router: &MultiServiceRouter, name: &str, request: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/signing-profiles/{name}/permissions"),
        Some(request),
    )
    .await
}

async fn remove_permission(
    router: &MultiServiceRouter,
    name: &str,
    statement_id: &str,
    revision_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!(
            "/signing-profiles/{name}/permissions/{}?revisionId={}",
            encode(statement_id),
            encode(revision_id)
        ),
        None,
    )
    .await
}

async fn list_permissions(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/signing-profiles/{name}/permissions"),
        None,
    )
    .await
}

async fn cancel_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "DELETE", &format!("/signing-profiles/{name}"), None).await
}

async fn get_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/signing-profiles/{name}"), None).await
}

/// Which actions a grant on a profile of each published platform may carry.
///
/// Source: `AddProfilePermissionRequest$action` — "You can designate the
/// following actions to an account" and then four actions, of which
/// `signer:StartSigningJob` "isn't supported for container image workflows" and
/// `signer:SignPayload` "isn't supported for AWS Lambda workflows"
/// (`smithy_query(query="doc", shape="AddProfilePermissionRequest",
/// member="action")`). So each platform admits the signing action of ITS workflow
/// plus the two that sign nothing, and the rows below cover every platform the
/// catalog publishes — the container-image one included, which the recorded
/// requests never named.
const PLATFORM_ACTIONS: &[(&str, &str, [&str; 3])] = &[
    (
        "AWSLambda-SHA384-ECDSA",
        "signer:SignPayload",
        [
            "signer:StartSigningJob",
            "signer:GetSigningProfile",
            "signer:RevokeSignature",
        ],
    ),
    (
        "AWSIoTDeviceManagement-SHA256-ECDSA",
        "signer:SignPayload",
        [
            "signer:StartSigningJob",
            "signer:GetSigningProfile",
            "signer:RevokeSignature",
        ],
    ),
    (
        "AmazonFreeRTOS-Default",
        "signer:SignPayload",
        [
            "signer:StartSigningJob",
            "signer:GetSigningProfile",
            "signer:RevokeSignature",
        ],
    ),
    (
        "AmazonFreeRTOS-TI-CC3220SF",
        "signer:SignPayload",
        [
            "signer:StartSigningJob",
            "signer:GetSigningProfile",
            "signer:RevokeSignature",
        ],
    ),
    (
        "Notation-OCI-SHA384-ECDSA",
        "signer:StartSigningJob",
        [
            "signer:SignPayload",
            "signer:GetSigningProfile",
            "signer:RevokeSignature",
        ],
    ),
];

/// The actions a grant may carry are the ones the profile's signing platform
/// admits, and an action outside that set is refused 400 with the platform and
/// the set named.
///
/// One row per published platform: every action the platform admits is accepted,
/// and the signing action of the OTHER workflow is refused on it — so the set is
/// the platform's rather than one global list.
#[tokio::test]
async fn a_grant_carries_only_the_actions_its_platform_admits() {
    for (ordinal, (platform, ungrantable, grantable)) in PLATFORM_ACTIONS.iter().enumerate() {
        let router = emulator();
        let name = format!("Action_Platform_{ordinal}");
        seed_profile(&router, &name, platform).await;

        for (index, action) in grantable.iter().enumerate() {
            let answer = add_permission(
                &router,
                &name,
                json!({
                    "statementId": format!("grant-{index}"),
                    "action": action,
                    "principal": ACCOUNT_ID,
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {platform} granting {action}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
        }

        let refused = add_permission(
            &router,
            &name,
            json!({
                "statementId": "refused",
                "action": ungrantable,
                "principal": ACCOUNT_ID,
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {platform} granting {ungrantable}: expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            Some(ERROR_CODE_VALIDATION),
            "case {platform} granting {ungrantable}: expected code {ERROR_CODE_VALIDATION}, \
             got {:?}",
            refused.body
        );
        let expected = format!(
            "Action {ungrantable} not supported for platform {platform}. \
             Action must be one of [{}]",
            grantable.join(", ")
        );
        assert_eq!(
            refused.message(),
            expected,
            "case {platform} granting {ungrantable}: expected {expected:?}, got {:?}",
            refused.message()
        );

        // Nothing the refusal named was written: the policy holds exactly the
        // grants the platform admitted.
        let listed = list_permissions(&router, &name).await;
        assert_eq!(
            listed.body["permissions"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            grantable.len(),
            "case {platform}: expected one statement per admitted action, got {:?}",
            listed.body
        );
    }
}

/// A write naming a `revisionId` the policy is not at is refused 409 with the
/// condition as its `code` and a message carrying BOTH the revision the request
/// supplied and the one the policy is at.
///
/// Rows differ in how the supplied revision is wrong (a value the service never
/// issued, a superseded one, an empty one) and in which write carried it.
#[tokio::test]
async fn a_revision_the_policy_is_not_at_is_refused_with_both_revisions_named() {
    let router = emulator();
    seed_profile(&router, "Revision_Profile", LAMBDA_PLATFORM).await;

    let first = add_permission(
        &router,
        "Revision_Profile",
        json!({
            "statementId": "first",
            "action": "signer:GetSigningProfile",
            "principal": ACCOUNT_ID,
        }),
    )
    .await;
    let superseded = first.str_member("revisionId");
    let second = add_permission(
        &router,
        "Revision_Profile",
        json!({
            "statementId": "second",
            "action": "signer:RevokeSignature",
            "principal": ACCOUNT_ID,
        }),
    )
    .await;
    let current = second.str_member("revisionId");
    assert_ne!(
        current, superseded,
        "a second grant must move the policy to a fresh revision, got {current:?} twice"
    );

    let stale_rows: Vec<(&str, String)> = vec![
        ("a revision the service never issued", "stale-revision-id".to_string()),
        ("a superseded revision", superseded.clone()),
        ("an empty revision", String::new()),
        ("a revision of another shape", "0".repeat(64)),
    ];

    for (case, supplied) in &stale_rows {
        let expected = format!(
            "Specified revisionId ({supplied}) does not match the revisionId of the existing \
             policy ({current})"
        );
        let added = add_permission(
            &router,
            "Revision_Profile",
            json!({
                "statementId": "third",
                "action": "signer:GetSigningProfile",
                "principal": ACCOUNT_ID,
                "revisionId": supplied,
            }),
        )
        .await;
        let removed = remove_permission(&router, "Revision_Profile", "first", supplied).await;
        for (operation, answer) in [("add", &added), ("remove", &removed)] {
            assert_eq!(
                answer.status, 409,
                "case {case:?} on {operation}: expected 409, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                Some(ERROR_CODE_REVISION_MISMATCH),
                "case {case:?} on {operation}: expected code {ERROR_CODE_REVISION_MISMATCH}, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?} on {operation}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }

    // The refused writes changed nothing: the policy is still at the revision
    // every refusal reported.
    let listed = list_permissions(&router, "Revision_Profile").await;
    assert_eq!(
        listed.str_member("revisionId"),
        current,
        "expected the refused writes to leave the policy at {current}, got {:?}",
        listed.body
    );
}

/// A profile's resource policy exists only while it holds a statement: a read of
/// a profile that has none is refused 404 `PolicyNotFound`, whether nothing was
/// ever granted or every statement has since been removed — and the same read is
/// served again once a statement is granted.
///
/// Rows differ in how many statements the policy held before they were removed,
/// which covers "nothing was ever granted" as the zero row.
#[tokio::test]
async fn a_policy_with_no_statements_does_not_exist() {
    for statements in [0usize, 1, 3] {
        let router = emulator();
        let name = format!("Policy_Lifecycle_{statements}");
        seed_profile(&router, &name, LAMBDA_PLATFORM).await;

        let mut revision = String::new();
        let ids: Vec<String> = (0..statements)
            .map(|index| format!("statement-{index}"))
            .collect();
        for id in &ids {
            let added = add_permission(
                &router,
                &name,
                json!({
                    "statementId": id,
                    "action": "signer:GetSigningProfile",
                    "principal": ACCOUNT_ID,
                }),
            )
            .await;
            assert_eq!(
                added.status, 200,
                "case {statements} statements: granting {id} expected 200, got {} {:?}",
                added.status, added.body
            );
            revision = added.str_member("revisionId");
        }

        // While the policy holds a statement, the read is served.
        if statements > 0 {
            let listed = list_permissions(&router, &name).await;
            assert_eq!(
                listed.status, 200,
                "case {statements} statements: expected the read to be served, got {} {:?}",
                listed.status, listed.body
            );
        }

        for id in &ids {
            let removed = remove_permission(&router, &name, id, &revision).await;
            assert_eq!(
                removed.status, 200,
                "case {statements} statements: removing {id} expected 200, got {} {:?}",
                removed.status, removed.body
            );
            // A removal hands back no revision, so the next write reads the one
            // the policy is at from the policy itself — which is also the last
            // read that can report one.
            assert!(
                removed.body["revisionId"].is_null(),
                "case {statements} statements: removing {id} must report no revisionId, got {:?}",
                removed.body
            );
            let listed = list_permissions(&router, &name).await;
            revision = listed.str_member("revisionId");
        }

        // With no statement left there is no policy, so the read is a missing
        // resource rather than an empty answer.
        let listed = list_permissions(&router, &name).await;
        let expected = format!("No policies associated with profile {name}");
        assert_eq!(
            listed.status, 404,
            "case {statements} statements: expected 404 once the policy is empty, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.code(),
            Some(ERROR_CODE_POLICY_NOT_FOUND),
            "case {statements} statements: expected code {ERROR_CODE_POLICY_NOT_FOUND}, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.message(),
            expected,
            "case {statements} statements: expected {expected:?}, got {:?}",
            listed.message()
        );
        assert_eq!(
            listed.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {statements} statements: expected x-amzn-errortype on the refusal, got {:?}",
            listed.error_type
        );

        // A grant re-creates the policy, and the read is served again: the
        // refusal is about the policy's state, not about the profile.
        let regranted = add_permission(
            &router,
            &name,
            json!({
                "statementId": "regranted",
                "action": "signer:GetSigningProfile",
                "principal": ACCOUNT_ID,
            }),
        )
        .await;
        assert_eq!(
            regranted.status, 200,
            "case {statements} statements: re-granting expected 200, got {} {:?}",
            regranted.status, regranted.body
        );
        let listed = list_permissions(&router, &name).await;
        assert_eq!(
            listed.status, 200,
            "case {statements} statements: expected the re-created policy to be readable, \
             got {} {:?}",
            listed.status, listed.body
        );
    }
}

/// `PutSigningProfile` judges the request's own members before it resolves the
/// platform the request names: a create that is both missing `signingMaterial`
/// and naming an unknown platform is the 400 about the member, never the 404
/// about the platform.
///
/// Rows span the two axes: whether the platform is published (and, where it is,
/// whether it issues its own certificate) and whether the request carries the
/// certificate.
#[tokio::test]
async fn the_request_is_judged_before_the_platform_it_names_is_resolved() {
    let router = emulator();

    // (case, platformId, carries signingMaterial, expected status, expected code)
    let rows: Vec<(&str, &str, bool, u16, &str)> = vec![
        (
            "an unknown platform and no signing material",
            "NoSuchPlatform-Probe",
            false,
            400,
            ERROR_CODE_VALIDATION,
        ),
        (
            "an unknown platform with signing material",
            "NoSuchPlatform-Probe",
            true,
            404,
            ERROR_CODE_PLATFORM_NOT_FOUND,
        ),
        (
            "a platform whose profiles hold the certificate, with none supplied",
            "AWSIoTDeviceManagement-SHA256-ECDSA",
            false,
            400,
            ERROR_CODE_VALIDATION,
        ),
        (
            "a platform AWS Signer issues the certificate for, with none supplied",
            LAMBDA_PLATFORM,
            false,
            200,
            "",
        ),
        (
            "the container-image platform, with none supplied",
            CONTAINER_PLATFORM,
            false,
            200,
            "",
        ),
    ];

    for (ordinal, (case, platform, with_material, status, code)) in rows.iter().enumerate() {
        let name = format!("Precedence_Profile_{ordinal}");
        let mut request = json!({ "platformId": platform });
        if *with_material {
            request["signingMaterial"] = json!({ "certificateArn": CERTIFICATE_ARN });
        }
        let answer = call(
            &router,
            "PUT",
            &format!("/signing-profiles/{name}"),
            Some(request),
        )
        .await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        if code.is_empty() {
            continue;
        }
        assert_eq!(
            answer.code(),
            Some(*code),
            "case {case:?}: expected code {code}, got {:?}",
            answer.body
        );
        if *code == ERROR_CODE_VALIDATION {
            assert_eq!(
                answer.message(),
                "signing material cannot be null.",
                "case {case:?}: expected the refusal to name the absent member, got {:?}",
                answer.message()
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected x-amzn-errortype ValidationException, got {:?}",
                answer.error_type
            );
        } else {
            let expected = format!("Signing platform not found for ID {platform}");
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }

        // A refused create stores nothing.
        let read_back = get_profile(&router, &name).await;
        assert_eq!(
            read_back.status, 404,
            "case {case:?}: a refused create must store no profile, got {} {:?}",
            read_back.status, read_back.body
        );
    }
}

/// A `platformId` the catalog publishes no platform for is reported the one way,
/// by every reader that resolves one.
///
/// Rows differ in how the id misses the catalog: an unpublished partner, a
/// published id in the wrong case, a near-miss on the algorithm suffix, and the
/// shortest id the published character class admits.
#[tokio::test]
async fn an_unpublished_platform_is_reported_the_same_way_by_every_reader() {
    let router = emulator();

    for (ordinal, platform) in [
        "NoSuchPlatform-Probe",
        "awslambda-sha384-ecdsa",
        "Notation-OCI-SHA256-ECDSA",
        "x",
    ]
    .iter()
    .enumerate()
    {
        let expected = format!("Signing platform not found for ID {platform}");

        let got = call(
            &router,
            "GET",
            &format!("/signing-platforms/{}", encode(platform)),
            None,
        )
        .await;
        assert_eq!(
            got.status, 404,
            "case {platform:?}: expected GetSigningPlatform 404, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.code(),
            Some(ERROR_CODE_PLATFORM_NOT_FOUND),
            "case {platform:?}: expected code {ERROR_CODE_PLATFORM_NOT_FOUND}, got {:?}",
            got.body
        );
        assert_eq!(
            got.message(),
            expected,
            "case {platform:?}: expected {expected:?}, got {:?}",
            got.message()
        );

        // The create that resolves the same catalog words the same condition the
        // same way.
        let created = call(
            &router,
            "PUT",
            &format!("/signing-profiles/Platform_Probe_{ordinal}"),
            Some(json!({
                "platformId": platform,
                "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
            })),
        )
        .await;
        assert_eq!(
            created.message(),
            expected,
            "case {platform:?}: expected PutSigningProfile to word the condition alike, got {:?}",
            created.message()
        );
    }
}

/// A `jobId` this region holds no signing job for is refused 404 with the
/// job-specific `code` by both the reader of a job and the write that revokes
/// its signature, each wording the condition the way AWS words it for THAT
/// operation.
///
/// Rows differ in the form of the id: a UUID nothing was minted under, a short
/// value, one carrying characters a label has to encode, and a long one.
#[tokio::test]
async fn an_unknown_signing_job_is_refused_by_every_operation_that_resolves_one() {
    let router = emulator();

    for job_id in [
        "00000000-0000-0000-0000-000000000000",
        "x",
        "not a job/id",
        &"j".repeat(300),
    ] {
        let described = call(
            &router,
            "GET",
            &format!("/signing-jobs/{}", encode(job_id)),
            None,
        )
        .await;
        let revoked = call(
            &router,
            "PUT",
            &format!("/signing-jobs/{}/revoke", encode(job_id)),
            Some(json!({ "reason": "a job that does not exist" })),
        )
        .await;

        for (operation, answer, expected) in [
            (
                "DescribeSigningJob",
                &described,
                format!("Signing job not found for id {job_id}"),
            ),
            (
                "RevokeSignature",
                &revoked,
                format!("Signing Job with id {job_id} does not exist."),
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {job_id:?} on {operation}: expected 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                Some(ERROR_CODE_JOB_NOT_FOUND),
                "case {job_id:?} on {operation}: expected code {ERROR_CODE_JOB_NOT_FOUND}, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {job_id:?} on {operation}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }
}

/// A tagging call whose `resourceArn` is not a signing-profile ARN is refused
/// 400 with the one fixed sentence the service states for bad ARN syntax — and
/// the refusal carries no part of the caller's value.
///
/// Rows differ in how the value fails to parse: text that is no ARN at all,
/// another service's ARN, an ARN with fields missing, one naming no region or no
/// account, and a long value.
#[tokio::test]
async fn a_tagging_arn_that_does_not_parse_is_refused_without_echoing_it() {
    let router = emulator();
    seed_profile(&router, "Tagging_Profile", LAMBDA_PLATFORM).await;

    let long = format!("not-an-arn-{}", "z".repeat(500));
    let malformed: Vec<(&str, String)> = vec![
        ("text that is no ARN at all", "not-a-valid-signer-arn".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:s3:{REGION}:{ACCOUNT_ID}:bucket/key"),
        ),
        ("an ARN with fields missing", "arn:aws:signer".to_string()),
        (
            "an ARN naming no region",
            format!("arn:aws:signer::{ACCOUNT_ID}:/signing-profiles/Tagging_Profile"),
        ),
        (
            "an ARN naming no account",
            format!("arn:aws:signer:{REGION}::/signing-profiles/Tagging_Profile"),
        ),
        ("a long value", long),
    ];

    for (case, resource_arn) in &malformed {
        let read = call(
            &router,
            "GET",
            &format!("/tags/{}", encode(resource_arn)),
            None,
        )
        .await;
        let written = call(
            &router,
            "POST",
            &format!("/tags/{}", encode(resource_arn)),
            Some(json!({ "tags": { "owner": "signing" } })),
        )
        .await;
        let removed = call(
            &router,
            "DELETE",
            &format!("/tags/{}?tagKeys=owner", encode(resource_arn)),
            None,
        )
        .await;

        for (operation, answer) in [
            ("ListTagsForResource", &read),
            ("TagResource", &written),
            ("UntagResource", &removed),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?} on {operation}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                Some(ERROR_CODE_BAD_REQUEST),
                "case {case:?} on {operation}: expected code {ERROR_CODE_BAD_REQUEST}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                "Invalid input resource arn: bad syntax",
                "case {case:?} on {operation}: expected the fixed sentence, got {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains(resource_arn.as_str()),
                "case {case:?} on {operation}: expected the refusal to quote none of the value, \
                 got {:?}",
                answer.message()
            );
        }
    }
}

/// A revoked signing profile cannot be canceled: the call is refused 400 with the
/// `code` naming the status that forbade it, the profile stays `Revoked`, and a
/// profile in any other status is canceled.
///
/// Rows differ in the platform the revoked profile is on — among the platforms
/// whose catalog entry publishes the revocation capability — and in the reason the
/// revocation carried, so the refusal is about the status rather than about one
/// recorded profile.
#[tokio::test]
async fn a_revoked_profile_cannot_be_canceled() {
    for (ordinal, (platform, reason)) in [
        (LAMBDA_PLATFORM, "key-compromise"),
        (CONTAINER_PLATFORM, "r"),
        (LAMBDA_PLATFORM, "rotate-signing-key"),
    ]
    .iter()
    .enumerate()
    {
        let router = emulator();
        let name = format!("Revoked_Cancel_{ordinal}");
        let version = seed_profile(&router, &name, platform).await;
        let revoked = call(
            &router,
            "PUT",
            &format!("/signing-profiles/{name}/revoke"),
            Some(json!({
                "profileVersion": version,
                "reason": reason,
                "effectiveTime": 1_700_000_000,
            })),
        )
        .await;
        assert_eq!(
            revoked.status, 204,
            "case {platform}: the revoke must succeed, got {} {:?}",
            revoked.status, revoked.body
        );

        let answer = cancel_profile(&router, &name).await;
        let expected = format!(
            "Signing Profile {name} cannot be canceled because it has been revoked. \
             A revoked profile cannot be canceled."
        );
        assert_eq!(
            answer.status, 400,
            "case {platform}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            Some(ERROR_CODE_PROFILE_REVOKED),
            "case {platform}: expected code {ERROR_CODE_PROFILE_REVOKED}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {platform}: expected {expected:?}, got {:?}",
            answer.message()
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ValidationException"),
            "case {platform}: expected x-amzn-errortype on the refusal, got {:?}",
            answer.error_type
        );

        // The refusal moved nothing: the profile is still the revoked one.
        let got = get_profile(&router, &name).await;
        assert_eq!(
            got.str_member("status"),
            "Revoked",
            "case {platform}: expected the profile to stay Revoked, got {:?}",
            got.body["status"]
        );

        // A profile on the same platform that was never revoked is canceled, so
        // the refusal is the status's and not the operation's.
        let cancelable = format!("Active_Cancel_{ordinal}");
        seed_profile(&router, &cancelable, platform).await;
        let canceled = cancel_profile(&router, &cancelable).await;
        assert_eq!(
            canceled.status, 204,
            "case {platform}: expected an active profile to be canceled, got {} {:?}",
            canceled.status, canceled.body
        );
        let got = get_profile(&router, &cancelable).await;
        assert_eq!(
            got.str_member("status"),
            "Canceled",
            "case {platform}: expected the cancel to be recorded, got {:?}",
            got.body["status"]
        );
    }
}
