// SPDX-License-Identifier: CC0-1.0

//! A REFUSED `UpdatePackagingGroup` leaves the CDN authorization the group ALREADY
//! HOLDS exactly as it was.
//!
//! `arn_documented_members.rs` states the ARN rule itself — a member documented as
//! the ARN of a named service (and resource kind) is accepted only as an ARN of that
//! service — over every member that carries one, with the addressed group holding no
//! authorization. This suite pins the other half of the same invariant, the half a
//! client of `UpdatePackagingGroup` loses if the refusal is not atomic: a group that
//! HOLDS a well-formed `Authorization` still reports the identical block after a
//! refused update (the junk block is not stored, and the block that was there is not
//! erased), its system-generated members are untouched, and the next accepted update
//! still replaces it.
//!
//! Asserted over a table: four starting states (no authorization, and three
//! well-formed blocks differing in partition, Region, account and name form) crossed
//! with the values a documented-ARN member refuses — not an ARN at all, an ARN of
//! another service, an ARN of the right service but the wrong resource kind, an empty
//! value — on each of the block's two members and on both at once. The constraint
//! phrases are read from the requirements the service states
//! (`state::SECRETS_MANAGER_SECRET_ARN`, `state::IAM_ROLE_ARN`) rather than spelled
//! again here.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagevod_mediapackagevod as vod;
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for; it only has to be a legal credential scope.
const REGION: &str = "us-west-2";

/// The group every case addresses. Each case runs on its own engine, so one id is
/// enough.
const GROUP: &str = "vod-group";

/// A well-formed Secrets Manager secret ARN, for the member a case is not varying.
const GOOD_SECRET: &str = "arn:aws:secretsmanager:us-west-2:111122223333:secret:cdn-AbCdEf";

/// A well-formed IAM role ARN, for the member a case is not varying.
const GOOD_IAM_ROLE: &str = "arn:aws:iam::111122223333:role/MediaPackageVodCdnAuth";

/// The modelled refusal of an invalid request parameter: error code and `httpError`
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")` — `errorKind
/// client`, `httpError 422`).
const UNPROCESSABLE: (&str, u16) = ("UnprocessableEntityException", 422);

/// What one answer says about itself: the status, the error shape
/// `x-amzn-errortype` names, and the parsed body.
struct Answer {
    status: u16,
    errortype: String,
    body: Value,
}

impl Answer {
    /// The per-attribute `errors` map, or `Value::Null` when the body carries none.
    fn errors(&self) -> Value {
        self.body.get("errors").cloned().unwrap_or(Value::Null)
    }

    /// The error shape the body itself names (restJson1's `__type`).
    fn json_type(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }

    /// The `authorization` the body reports, or `Null` when it carries none — which
    /// is how a group with no CDN authorization reports itself.
    fn authorization(&self) -> Value {
        self.body.get("authorization").cloned().unwrap_or(Value::Null)
    }

    /// The members no update may change, as one comparable value.
    fn immutable_members(&self) -> Value {
        json!({
            "id": self.body.get("id").cloned().unwrap_or(Value::Null),
            "arn": self.body.get("arn").cloned().unwrap_or(Value::Null),
            "createdAt": self.body.get("createdAt").cloned().unwrap_or(Value::Null),
            "domainName": self.body.get("domainName").cloned().unwrap_or(Value::Null),
        })
    }
}

/// A router carrying exactly this service over one shared engine.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(vod::EmulatedService::new(Arc::new(
        engine::InMemoryEngine::new(),
    )));
    router
}

/// Send one request and report what it answered.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "<unset>".to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        errortype,
        body,
    }
}

/// The phrase a `CdnIdentifierSecret` violation carries, read from the requirement
/// the service states for that member rather than spelled again here.
fn secret_phrase() -> String {
    vod::state::SECRETS_MANAGER_SECRET_ARN.phrase()
}

/// The phrase a `SecretsRoleArn` violation carries, from the same source.
fn role_phrase() -> String {
    vod::state::IAM_ROLE_ARN.phrase()
}

/// The authorization blocks a group may ALREADY hold, as the starting state of a
/// case: none, and three well-formed blocks differing in partition, Region, account
/// and name form.
fn starting_blocks() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no authorization", None),
        (
            "a secret and a role in this region",
            Some(json!({
                "cdnIdentifierSecret": GOOD_SECRET,
                "secretsRoleArn": GOOD_IAM_ROLE,
            })),
        ),
        (
            "another partition",
            Some(json!({
                "cdnIdentifierSecret":
                    "arn:aws-cn:secretsmanager:cn-north-1:444455556666:secret:cdn-9z8y7x",
                "secretsRoleArn": "arn:aws-cn:iam::444455556666:role/VodCdn",
            })),
        ),
        (
            "another account, a slashed secret name and a role path",
            Some(json!({
                "cdnIdentifierSecret":
                    "arn:aws:secretsmanager:ap-southeast-2:777788889999:secret:team/cdn/auth-0",
                "secretsRoleArn": "arn:aws:iam::777788889999:role/path/to/Role_Name-1",
            })),
        ),
    ]
}

/// `(case, the junk block, the `errors` map the refusal must carry)` — one row per
/// way a documented-ARN member can fail, on each member of the block and on both at
/// once. Every other member of a row is well formed, so the attribution can only be
/// about the member the row varies.
fn refused_blocks() -> Vec<(String, Value, Value)> {
    // (case, the value, for `cdnIdentifierSecret`) — a value that is not an ARN, an
    // ARN of another service, a Secrets Manager resource of another kind, and the
    // degenerate short values a lenient handler takes.
    let secrets: Vec<(&str, &str)> = vec![
        ("not an ARN at all", "not-an-arn"),
        ("the empty string", ""),
        ("a single character", "a"),
        ("an IAM role ARN", GOOD_IAM_ROLE),
        (
            "a Secrets Manager resource of another kind",
            "arn:aws:secretsmanager:us-west-2:111122223333:secret-version:v1",
        ),
        (
            "an ARN with an empty resource",
            "arn:aws:secretsmanager:us-west-2:111122223333:",
        ),
    ];
    // The same for `secretsRoleArn`, whose requirement names the `role` resource
    // type as well as the vendor.
    let roles: Vec<(&str, &str)> = vec![
        ("not an ARN at all", "nope"),
        ("the empty string", ""),
        ("a URL", "https://roles.example.com/VodCdn"),
        ("a Secrets Manager secret ARN", GOOD_SECRET),
        (
            "an IAM resource kind that only prefixes role",
            "arn:aws:iam::111122223333:role-policy/VodCdn",
        ),
    ];

    let mut rows: Vec<(String, Value, Value)> = Vec::new();
    for (case, value) in secrets {
        rows.push((
            format!("cdnIdentifierSecret is {case}"),
            json!({ "cdnIdentifierSecret": value, "secretsRoleArn": GOOD_IAM_ROLE }),
            json!({ "authorization.cdnIdentifierSecret": [secret_phrase()] }),
        ));
    }
    for (case, value) in roles {
        rows.push((
            format!("secretsRoleArn is {case}"),
            json!({ "cdnIdentifierSecret": GOOD_SECRET, "secretsRoleArn": value }),
            json!({ "authorization.secretsRoleArn": [role_phrase()] }),
        ));
    }
    // Both members wrong: one refusal reports both attributes, so the caller does
    // not fix one to discover the other.
    rows.push((
        "both members are junk".to_string(),
        json!({ "cdnIdentifierSecret": "not-an-arn", "secretsRoleArn": "nope" }),
        json!({
            "authorization.cdnIdentifierSecret": [secret_phrase()],
            "authorization.secretsRoleArn": [role_phrase()],
        }),
    ));
    rows
}

/// Create `GROUP` carrying `authorization` when it is `Some`, asserting the setup
/// succeeded, and report what the create answered.
async fn create_group(router: &MultiServiceRouter, authorization: Option<&Value>) -> Answer {
    let body = match authorization {
        Some(block) => json!({ "id": GROUP, "authorization": block.clone() }),
        None => json!({ "id": GROUP }),
    };
    let answer = call(router, "POST", "/packaging_groups", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "setup: CreatePackagingGroup should succeed, got {} body {}",
        answer.status, answer.body
    );
    answer
}

/// `UpdatePackagingGroup` for `GROUP`, carrying `authorization`.
async fn update_group(router: &MultiServiceRouter, authorization: &Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/packaging_groups/{GROUP}"),
        Some(json!({ "authorization": authorization })),
    )
    .await
}

/// A refused update is a no-op: the group keeps the authorization it already held,
/// keeps its system-generated members, and still accepts a well-formed replacement
/// afterwards.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_authorization_untouched() {
    for (start_case, start) in starting_blocks() {
        for (refusal_case, junk, expected_errors) in refused_blocks() {
            let case = format!("{start_case}, then {refusal_case}");
            let router = router();
            let created = create_group(&router, start.as_ref()).await;
            let held = start.clone().unwrap_or(Value::Null);
            assert_eq!(
                created.authorization(),
                held,
                "setup {case:?}: the created group must hold {held}, got {}",
                created.authorization()
            );
            let baseline = created.immutable_members();

            let (code, status) = UNPROCESSABLE;
            let answer = update_group(&router, &junk).await;
            assert_eq!(
                answer.status, status,
                "case {case:?}: expected HTTP {status}, got {} body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.errortype, code,
                "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
                answer.errortype
            );
            assert_eq!(
                answer.json_type(),
                code,
                "case {case:?}: the body's __type must name the same shape as the header, got {:?}",
                answer.json_type()
            );
            assert_eq!(
                answer.errors(),
                expected_errors,
                "case {case:?}: expected errors {expected_errors}, got {}",
                answer.errors()
            );

            // The refusal stored nothing: the group still reports exactly what it
            // held, and the junk block is nowhere in the projection.
            let described = call(
                &router,
                "GET",
                &format!("/packaging_groups/{GROUP}"),
                None,
            )
            .await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the refused update must leave the group in place, got {} body {}",
                described.status, described.body
            );
            assert_eq!(
                described.authorization(),
                held,
                "case {case:?}: the refused update must leave {held} stored, got {}",
                described.authorization()
            );
            assert_eq!(
                described.immutable_members(),
                baseline,
                "case {case:?}: the refused update must leave {baseline} unchanged, got {}",
                described.immutable_members()
            );

            // And the refusal is about the value, not about the group: a well-formed
            // block still replaces whatever the group held.
            let replacement = json!({
                "cdnIdentifierSecret":
                    "arn:aws:secretsmanager:eu-central-1:111122223333:secret:replacement-AbCdEf",
                "secretsRoleArn": "arn:aws:iam::111122223333:role/Replacement",
            });
            let updated = update_group(&router, &replacement).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: a well-formed replacement must be accepted, got {} body {}",
                updated.status, updated.body
            );
            let described = call(
                &router,
                "GET",
                &format!("/packaging_groups/{GROUP}"),
                None,
            )
            .await;
            assert_eq!(
                described.authorization(),
                replacement,
                "case {case:?}: expected the stored group to report {replacement}, got {}",
                described.authorization()
            );
        }
    }
}
