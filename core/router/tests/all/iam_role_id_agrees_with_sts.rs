// SPDX-License-Identifier: CC0-1.0

//! One role, two services: the unique id IAM issues for a role and the id STS reports
//! for an assumed role have the SAME shape, the one `idType` models.
//!
//! IAM mints `Role.RoleId` from its own counter, and STS derives the `AssumedRoleId` it
//! reports from the role's ARN; the two are minted independently, in two crates that
//! cannot depend on one another. What they must agree on is the FORM of an AWS role
//! unique id — the `AROA` type prefix plus 17 characters of the uppercase base32
//! alphabet, 21 in total, which is what the modelled `idType` (length 16..128, pattern
//! `^[\w]+$`) admits. A client that parses or stores one of these ids sees one shape
//! whichever service produced it, so this is the suite that pins the shape on both sides
//! at once; a crate-local test can only ever pin its own.
//!
//! Cross-service routing for the `iam` signing name the two IAM-family services share is
//! covered by `shared_signing_name_routing.rs`; the legs it does not reach are in
//! `iam_signing_name_routing.rs`.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names IAM's signing name. The
/// signature is never checked: the router here runs with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/iam/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// A trust policy that grants the account root permission to assume a role. It carries
/// none of the characters that are significant in an `application/x-www-form-urlencoded`
/// body (`&`, `=`, `+`, `%`, space), so it needs no encoding here.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// The type prefix AWS gives a role's unique id.
const ROLE_ID_PREFIX: &str = "AROA";

/// How long an AWS unique id is: the four-character type prefix plus 17 more.
const ROLE_ID_LEN: usize = 21;

/// The emulator with IAM mounted, exactly as `core/router/src/lib.rs` mounts it.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// The text of the first `<name>…</name>` element of an XML body. Every element these
/// responses carry is uniquely named, so this reads the member without a document model.
pub(crate) fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Create one role over IAM's awsQuery wire and return its `Arn` and `RoleId`.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) -> (String, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}&Path={path}\
             &AssumeRolePolicyDocument={TRUST_POLICY}"
        )))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {role_name:?}: expected HTTP 200 from CreateRole, got {status} with body {payload}"
    );
    (
        xml_element(&payload, "Arn")
            .unwrap_or_else(|| panic!("case {role_name:?}: no <Arn> in {payload}")),
        xml_element(&payload, "RoleId")
            .unwrap_or_else(|| panic!("case {role_name:?}: no <RoleId> in {payload}")),
    )
}

/// Assert `id` is an AWS role unique id: the `AROA` prefix plus 17 characters, all of
/// them inside `idType`'s `^[\w]+$` pattern.
fn assert_role_id_shape(case: &str, source: &str, id: &str) {
    assert!(
        id.starts_with(ROLE_ID_PREFIX),
        "case {case:?}: expected {source}'s role id to start with {ROLE_ID_PREFIX}, got {id:?}"
    );
    assert_eq!(
        id.chars().count(),
        ROLE_ID_LEN,
        "case {case:?}: expected {source}'s role id to be {ROLE_ID_LEN} characters, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        "case {case:?}: expected {source}'s role id to satisfy idType's ^[\\w]+$, got {id:?}"
    );
}

/// Every role IAM mints, and every id STS reports for the same role's ARN, is an AWS
/// role unique id of the one modelled shape — and IAM never issues one id twice.
#[tokio::test]
async fn iam_and_sts_report_role_ids_of_the_one_modelled_shape() {
    let router = build_router();

    // Rows differ in what goes into the role, and so into its ARN: the name and the path
    // the ARN embeds (`role<path><RoleName>`).
    let rows = [
        ("plain-role", "/"),
        ("plain-role-2", "/"),
        ("pathed-role", "/engineering/"),
        ("deeply-pathed-role", "/engineering/data/"),
        ("punctuated.role-name@example", "/"),
    ];

    let mut seen = Vec::new();
    for (role_name, path) in rows {
        let (arn, role_id) = create_role(&router, role_name, path).await;
        assert_role_id_shape(role_name, "IAM", &role_id);

        let from_sts =
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::role_id(&arn);
        assert_role_id_shape(role_name, "STS", &from_sts);

        seen.push(role_id);
    }

    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        seen.len(),
        "expected one unique id per role, got {seen:?}"
    );
}
