// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the GLOBAL condition keys this service answers for every request it
//! authorizes.
//!
//! The rule under test: **a statement's `Condition` over a global condition key whose value
//! this service holds for every request — the ARN of the identity that signed it
//! (`aws:PrincipalArn`), the account that identity belongs to (`aws:PrincipalAccount`) and
//! the account that owns the resource addressed (`aws:ResourceAccount`) — DECIDES that
//! statement.** So a grant whose clause holds authorizes the call, a grant whose clause does
//! not hold authorizes nothing, and a deny whose clause holds refuses the call — while a
//! clause over a key this service does not answer leaves its statement undecided, which
//! neither grants nor refuses.
//!
//! Why it matters beyond the keys themselves: an identity policy that scopes a broad grant to
//! the account owning the resource (the shape a bootstrap or deployment role is written in)
//! is the ONLY grant such a principal carries, so a clause left undecided is a principal
//! refused every call its policy was written to allow.
//!
//! Every assertion drives real HTTP requests through a router holding S3 beside IAM over one
//! shared engine: the user, its access key and its policy are created through IAM's own API,
//! and each S3 request presents that user's access key id, which is where the emulator reads
//! the signing identity from.
//!
//! The rows differ along what the rule generalises over: WHICH key the clause names, which
//! operator compares it (equality, negated equality, an ARN pattern, a set-membership list,
//! an existence test, an `IfExists` spelling), whether the compared value matches, whether
//! the conditioned statement is the `Allow` or a `Deny` over a broad `Allow`, and which
//! operation the call is — a bucket listing, an object read and an object write, so the rule
//! is pinned on bucket-scoped and object-scoped resources alike. The last row is the
//! boundary: a grant conditioned on a key this service does NOT answer stays undecided and
//! authorizes nothing.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The bucket every row addresses.
const BUCKET: &str = "condition-keys-subject";

/// The key every row's setup writes, which the object rows read and overwrite.
const KEY: &str = "reports/held.csv";

/// The bytes that key holds.
const BODY: &[u8] = b"global-condition-key-subject";

/// The code S3 answers an authorization refusal with.
const ACCESS_DENIED: &str = "AccessDenied";

/// A router serving S3 beside IAM over ONE shared engine, which is what lets a request
/// signed with a key `iam:CreateAccessKey` vended be judged on the policy
/// `iam:PutUserPolicy` wrote.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Neither the key id nor the signature is a credential here: the suite turns request
    // authentication off, exactly as the other suites of this crate do, so the access key id
    // on the wire is read as the signing identity and nothing is verified.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM, answered with the HTTP status and the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed IAM request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// Percent-encode one `application/x-www-form-urlencoded` component, through the crate this
/// service already percent-encodes with rather than by hand.
fn form_encode(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The text of the first `<tag>` element of a response body.
fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let (_, rest) = body
        .split_once(&open)
        .unwrap_or_else(|| panic!("the response must carry <{tag}>, got {body}"));
    let (text, _) = rest
        .split_once(&close)
        .unwrap_or_else(|| panic!("<{tag}> must be closed, got {body}"));
    text.to_string()
}

/// One S3 request, presented as signed by `akid` when one is given.
async fn s3(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, String) {
    let host = "s3.us-east-1.amazonaws.com";
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("content-type", "application/octet-stream");
    if let Some(akid) = akid {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/us-east-1/s3/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    }
    let request = builder
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    (
        parts.status.as_u16(),
        String::from_utf8_lossy(&body).to_string(),
    )
}

/// The account this emulator serves, which owns every bucket it holds.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of the IAM user `name` of this account: the spelling `aws:PrincipalArn` answers
/// with for a request signed by that user's long-term key.
fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", account())
}

/// The bucket and object ARNs a grant for [`BUCKET`] names.
fn bucket_resources() -> String {
    format!("[\"arn:aws:s3:::{BUCKET}\",\"arn:aws:s3:::{BUCKET}/*\"]")
}

/// Create [`BUCKET`] and write [`KEY`] into it, as the account's own credential.
async fn seeded_bucket(router: &MultiServiceRouter) {
    let (status, body) = s3(router, None, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(status, 200, "the bucket must be created, got {status}: {body}");
    let (status, body) = s3(router, None, "PUT", &format!("/{BUCKET}/{KEY}"), BODY).await;
    assert_eq!(status, 200, "the object must be written, got {status}: {body}");
}

/// An IAM user carrying exactly `document`, and the access key id a request speaks for it
/// with.
async fn user_with_document(router: &MultiServiceRouter, name: &str, document: &str) -> String {
    let (status, body) = iam(router, "CreateUser", &[("UserName", name)]).await;
    assert_eq!(status, 200, "the user {name:?} must be created, got {body}");
    let (status, body) = iam(
        router,
        "PutUserPolicy",
        &[
            ("UserName", name),
            ("PolicyName", "scoped"),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the policy of {name:?} must be stored, got {body}"
    );
    let (status, body) = iam(router, "CreateAccessKey", &[("UserName", name)]).await;
    assert_eq!(
        status, 200,
        "an access key for {name:?} must be vended, got {body}"
    );
    element(&body, "AccessKeyId")
}

/// WHICH call a row makes, which is also the IAM action it is authorized under and the
/// resource that decision is taken on (the bucket for the listing, the key for the two
/// object calls).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    /// `ListObjectsV2`, authorized under `s3:ListBucket` on the bucket.
    ListBucket,
    /// `GetObject`, authorized under `s3:GetObject` on the key.
    GetObject,
    /// `PutObject`, authorized under `s3:PutObject` on the key.
    PutObject,
}

impl Call {
    /// The IAM action a grant has to name for this call.
    fn action(self) -> &'static str {
        match self {
            Self::ListBucket => "s3:ListBucket",
            Self::GetObject => "s3:GetObject",
            Self::PutObject => "s3:PutObject",
        }
    }

    /// Make the call as `akid`.
    async fn drive(self, router: &MultiServiceRouter, akid: &str) -> (u16, String) {
        match self {
            Self::ListBucket => {
                s3(
                    router,
                    Some(akid),
                    "GET",
                    &format!("/{BUCKET}?list-type=2"),
                    b"",
                )
                .await
            }
            Self::GetObject => {
                s3(router, Some(akid), "GET", &format!("/{BUCKET}/{KEY}"), b"").await
            }
            Self::PutObject => {
                s3(
                    router,
                    Some(akid),
                    "PUT",
                    &format!("/{BUCKET}/{KEY}"),
                    b"rewritten",
                )
                .await
            }
        }
    }
}

/// One row: the conditioned statement a user's only policy carries, and whether the call it
/// is written about is authorized.
struct Row {
    /// What to call the case in a failure.
    case: &'static str,
    /// The `Effect` of the CONDITIONED statement.
    effect: &'static str,
    /// Its `Condition` object, built from the caller's own ARN and this account's id so no
    /// row carries a value copied out of one run.
    condition: fn(&str) -> String,
    /// Whether the document also carries a broad unconditioned `Allow`, which is what makes a
    /// conditioned `Deny` row a decision about the deny rather than about an absent grant.
    broad_allow: bool,
    /// The call the row makes.
    call: Call,
    /// Whether that call must be authorized.
    allowed: bool,
}

/// The table. Each `condition` is handed the ARN of the user the row creates.
const ROWS: &[Row] = &[
    // --- aws:ResourceAccount: the account owning the bucket or key addressed ---
    Row {
        case: "a grant scoped to the account that owns the resource, as a one-entry list",
        effect: "Allow",
        condition: |_user| {
            format!(
                "{{\"StringEquals\":{{\"aws:ResourceAccount\":[\"{}\"]}}}}",
                account()
            )
        },
        broad_allow: false,
        call: Call::ListBucket,
        allowed: true,
    },
    Row {
        case: "the same grant on an object read",
        effect: "Allow",
        condition: |_user| {
            format!(
                "{{\"StringEquals\":{{\"aws:ResourceAccount\":\"{}\"}}}}",
                account()
            )
        },
        broad_allow: false,
        call: Call::GetObject,
        allowed: true,
    },
    Row {
        case: "the same grant on an object write",
        effect: "Allow",
        condition: |_user| {
            format!(
                "{{\"StringEquals\":{{\"aws:ResourceAccount\":\"{}\"}}}}",
                account()
            )
        },
        broad_allow: false,
        call: Call::PutObject,
        allowed: true,
    },
    Row {
        case: "a grant scoped to a DIFFERENT account than the resource's owner",
        effect: "Allow",
        condition: |_user| {
            "{\"StringEquals\":{\"aws:ResourceAccount\":\"111122223333\"}}".to_string()
        },
        broad_allow: false,
        call: Call::ListBucket,
        allowed: false,
    },
    Row {
        case: "a grant that excludes the owner's own account",
        effect: "Allow",
        condition: |_user| {
            format!(
                "{{\"StringNotEquals\":{{\"aws:ResourceAccount\":\"{}\"}}}}",
                account()
            )
        },
        broad_allow: false,
        call: Call::GetObject,
        allowed: false,
    },
    Row {
        case: "a deny over the resource's owning account, above a broad grant",
        effect: "Deny",
        condition: |_user| {
            format!(
                "{{\"StringEquals\":{{\"aws:ResourceAccount\":\"{}\"}}}}",
                account()
            )
        },
        broad_allow: true,
        call: Call::PutObject,
        allowed: false,
    },
    Row {
        case: "a deny over another account, above a broad grant",
        effect: "Deny",
        condition: |_user| {
            "{\"StringEquals\":{\"aws:ResourceAccount\":\"111122223333\"}}".to_string()
        },
        broad_allow: true,
        call: Call::PutObject,
        allowed: true,
    },
    Row {
        case: "a grant demanding the resource's owning account be KNOWN",
        effect: "Allow",
        condition: |_user| "{\"Null\":{\"aws:ResourceAccount\":\"false\"}}".to_string(),
        broad_allow: false,
        call: Call::ListBucket,
        allowed: true,
    },
    Row {
        case: "a grant demanding the resource's owning account be ABSENT",
        effect: "Allow",
        condition: |_user| "{\"Null\":{\"aws:ResourceAccount\":\"true\"}}".to_string(),
        broad_allow: false,
        call: Call::ListBucket,
        allowed: false,
    },
    // --- aws:PrincipalAccount: the account the signing identity belongs to ---
    Row {
        case: "a grant scoped to the account the caller belongs to",
        effect: "Allow",
        condition: |_user| {
            format!(
                "{{\"StringEquals\":{{\"aws:PrincipalAccount\":\"{}\"}}}}",
                account()
            )
        },
        broad_allow: false,
        call: Call::GetObject,
        allowed: true,
    },
    Row {
        case: "a grant scoped to another account than the caller's",
        effect: "Allow",
        condition: |_user| {
            "{\"StringEquals\":{\"aws:PrincipalAccount\":\"111122223333\"}}".to_string()
        },
        broad_allow: false,
        call: Call::GetObject,
        allowed: false,
    },
    Row {
        case: "a grant scoped to a SET the caller's account is in",
        effect: "Allow",
        condition: |_user| {
            format!(
                "{{\"StringEquals\":{{\"aws:PrincipalAccount\":[\"111122223333\",\"{}\"]}}}}",
                account()
            )
        },
        broad_allow: false,
        call: Call::ListBucket,
        allowed: true,
    },
    // --- aws:PrincipalArn: the identity that signed the request ---
    Row {
        case: "a grant naming the caller's own ARN",
        effect: "Allow",
        condition: |user| format!("{{\"StringEquals\":{{\"aws:PrincipalArn\":\"{user}\"}}}}"),
        broad_allow: false,
        call: Call::ListBucket,
        allowed: true,
    },
    Row {
        case: "a grant naming another principal's ARN",
        effect: "Allow",
        condition: |_user| {
            format!(
                "{{\"StringEquals\":{{\"aws:PrincipalArn\":\"arn:aws:iam::{}:user/somebody-else\"}}}}",
                account()
            )
        },
        broad_allow: false,
        call: Call::ListBucket,
        allowed: false,
    },
    Row {
        case: "a grant matching the caller's ARN by pattern",
        effect: "Allow",
        condition: |_user| {
            format!(
                "{{\"ArnLike\":{{\"aws:PrincipalArn\":\"arn:aws:iam::{}:user/*\"}}}}",
                account()
            )
        },
        broad_allow: false,
        call: Call::PutObject,
        allowed: true,
    },
    Row {
        case: "a grant whose IfExists clause the caller's ARN satisfies",
        effect: "Allow",
        condition: |user| {
            format!("{{\"StringEqualsIfExists\":{{\"aws:PrincipalArn\":\"{user}\"}}}}")
        },
        broad_allow: false,
        call: Call::GetObject,
        allowed: true,
    },
    Row {
        case: "a deny naming the caller's own ARN, above a broad grant",
        effect: "Deny",
        condition: |user| format!("{{\"ArnEquals\":{{\"aws:PrincipalArn\":\"{user}\"}}}}"),
        broad_allow: true,
        call: Call::ListBucket,
        allowed: false,
    },
    // --- the boundary: a key this service does NOT answer stays undecided ---
    Row {
        case: "a grant conditioned on a key this service does not answer",
        effect: "Allow",
        condition: |_user| {
            "{\"StringEquals\":{\"s3:ExistingObjectTag/stage\":\"production\"}}".to_string()
        },
        broad_allow: false,
        call: Call::GetObject,
        allowed: false,
    },
];

/// The document a row's user carries: its conditioned statement over the bucket and its keys,
/// preceded by the broad grant the row asks for.
fn document(row: &Row, user: &str, action: &str) -> String {
    let conditioned = format!(
        "{{\"Effect\":\"{}\",\"Action\":\"{action}\",\"Resource\":{},\"Condition\":{}}}",
        row.effect,
        bucket_resources(),
        (row.condition)(user)
    );
    let statements = if row.broad_allow {
        format!(
            "{{\"Effect\":\"Allow\",\"Action\":\"s3:*\",\"Resource\":{}}},{conditioned}",
            bucket_resources()
        )
    } else {
        conditioned
    };
    format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statements}]}}")
}

/// RULE: a clause over a global condition key this service answers decides its statement —
/// the grant holds or it does not, and the deny refuses or it does not — while a clause over
/// a key it does not answer leaves the statement undecided, granting and refusing nothing.
#[tokio::test]
async fn a_clause_over_an_answered_global_key_decides_its_statement() {
    for (index, row) in ROWS.iter().enumerate() {
        let router = router();
        seeded_bucket(&router).await;
        let name = format!("scoped{index}");
        let arn = user_arn(&name);
        let document = document(row, &arn, row.call.action());
        let akid = user_with_document(&router, &name, &document).await;
        let (status, body) = row.call.drive(&router, &akid).await;
        if row.allowed {
            assert_eq!(
                status, 200,
                "case {:?} ({:?}): expected the call to be authorized, got {status}: {body}",
                row.case, row.call
            );
        } else {
            assert_eq!(
                status, 403,
                "case {:?} ({:?}): expected HTTP 403, got {status}: {body}",
                row.case, row.call
            );
            let code = element(&body, "Code");
            assert_eq!(
                code, ACCESS_DENIED,
                "case {:?} ({:?}): expected {ACCESS_DENIED}, got {code:?}: {body}",
                row.case, row.call
            );
        }
    }
}

/// RULE, on the shape a deployment role is written in: a principal whose ONLY grant is a
/// broad one scoped to the account owning the resource may use every action that grant names,
/// and the same policy pointed at another account's resources authorizes nothing — so the
/// scoping is what the clause really compares, not a value the gate ignores.
#[tokio::test]
async fn a_broad_grant_scoped_to_the_resource_account_authorizes_every_action_it_names() {
    let scoped = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":[\"s3:GetObject*\",\"s3:GetBucket*\",\"s3:List*\",\"s3:PutObject*\",\
         \"s3:DeleteObject*\",\"s3:Abort*\"],\"Resource\":{},\"Condition\":\
         {{\"StringEquals\":{{\"aws:ResourceAccount\":[\"{}\"]}}}}}}]}}",
        bucket_resources(),
        account()
    );
    let elsewhere = scoped.replace(&account(), "111122223333");

    for (case, document, allowed) in [
        ("scoped to the owning account", scoped, true),
        ("scoped to another account", elsewhere, false),
    ] {
        let router = router();
        seeded_bucket(&router).await;
        let akid = user_with_document(&router, "publisher", &document).await;
        for call in [Call::ListBucket, Call::GetObject, Call::PutObject] {
            let (status, body) = call.drive(&router, &akid).await;
            let expected = if allowed { 200 } else { 403 };
            assert_eq!(
                status, expected, 
                "case {case:?} ({call:?}): expected HTTP {expected}, got {status}: {body}"
            );
        }
    }
}
