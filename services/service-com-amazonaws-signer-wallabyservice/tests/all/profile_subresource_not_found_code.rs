// SPDX-License-Identifier: CC0-1.0

//! Which `code` a Signer 404 carries: the profile itself, or a sub-resource of
//! it.
//!
//! Every Signer error body models `code` beside `message`, and `code` is the
//! machine-readable discriminator a client branches on. The rule under test is
//! that it names the resource the endpoint could not resolve:
//!
//! * a request whose PROFILE does not exist is refused with the profile-not-found
//!   code, whichever operation refused it;
//! * a request whose profile WAS resolved and whose named sub-resource — a
//!   version of that profile, one of its permission statements — does not exist is
//!   refused with the modelled error shape's own name, so a client is never told a
//!   profile that is present and `Active` is absent.
//!
//! Both halves run through the router, so each case exercises routing, the
//! `@httpLabel` bindings, deserialization, the handler and serialization — the
//! path a client takes. The cases are tables: a row per input class the rule
//! generalises over (the character make-up of the unknown version, a version that
//! exists but belongs to another profile, which operation named it, and which
//! profile name no profile exists under).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: what routes a request to signer.
const SIGNED_FOR_US_WEST_2: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/signer/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The `code` a refusal whose PROFILE does not exist carries. Spelled out here
/// rather than imported so a wrong code in a handler fails this suite.
const ERROR_CODE_PROFILE_NOT_FOUND: &str = "ProfileNotFound";

/// The `code` a 404 about a sub-resource of a resolved profile carries — the
/// modelled shape's own name, which is all a 404 Signer publishes no resource
/// code for can say.
const ERROR_CODE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `code` a read of a profile that holds no permission policy carries: the
/// policy is a resource of its own, and Signer publishes a code for it.
const ERROR_CODE_POLICY_NOT_FOUND: &str = "PolicyNotFound";

/// A platform whose profiles need no `signingMaterial`, so a seeded profile is
/// one `PutSigningProfile` call.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// An action a cross-account grant may carry. Source:
/// `AddProfilePermissionRequest$action` (`smithy_query(query="doc",
/// shape="AddProfilePermissionRequest", member="action")`).
const GRANTABLE_ACTION: &str = "signer:GetSigningProfile";

/// The account a grant is written for: `principal` is an unconstrained string an
/// account id is a valid value of.
const PRINCIPAL: &str = "123456789012";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: status and JSON body.
struct Answer {
    status: u16,
    body: Value,
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

    fn str_member(&self, member: &str) -> &str {
        self.body[member].as_str().unwrap_or_default()
    }
}

/// `value` percent-encoded for ONE URI segment: a `statementId` is an
/// `@httpLabel`, so an SDK encodes everything outside the unreserved set.
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
        .header("host", "signer.us-west-2.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_WEST_2);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
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
    Answer { status, body }
}

/// A profile of `name`, and the version `PutSigningProfile` minted for it.
async fn seed_profile(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({ "platformId": PLATFORM_ID })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("profileVersion").to_string()
}

async fn revoke_profile(router: &MultiServiceRouter, name: &str, profile_version: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}/revoke"),
        Some(json!({
            "profileVersion": profile_version,
            "reason": "because",
            "effectiveTime": 1_700_000_000,
        })),
    )
    .await
}

async fn add_permission(
    router: &MultiServiceRouter,
    name: &str,
    profile_version: Option<&str>,
    statement_id: &str,
) -> Answer {
    let mut request = json!({
        "action": GRANTABLE_ACTION,
        "principal": PRINCIPAL,
        "statementId": statement_id,
    });
    if let Some(version) = profile_version {
        request["profileVersion"] = json!(version);
    }
    call(
        router,
        "POST",
        &format!("/signing-profiles/{name}/permissions"),
        Some(request),
    )
    .await
}

async fn get_profile(router: &MultiServiceRouter, name: &str) -> Answer {
    call(router, "GET", &format!("/signing-profiles/{name}"), None).await
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

/// A `profileVersion` the profile it is named against has never had, refused by
/// every operation that resolves a version of a profile: 404 with the
/// sub-resource code, and nothing written.
#[tokio::test]
async fn an_unknown_profile_version_is_refused_as_a_sub_resource() {
    let router = emulator();
    let subject_version = seed_profile(&router, "Subject_Profile").await;
    // A second profile, so one row can name a version that EXISTS and belongs to
    // another profile — a real version is still no version of this profile.
    let other_version = seed_profile(&router, "Other_Profile").await;

    // The rows differ along the character make-up of the version (`ProfileVersion`
    // is `@pattern ^[a-zA-Z0-9]{10}$`, `@length 10..10`, so every row is a value
    // the boundary admits) and along whether the version exists elsewhere.
    let unknown_versions: Vec<(&str, String)> = vec![
        ("all upper-case letters", "ZZZZZZZZZZ".to_string()),
        ("all digits", "0123456789".to_string()),
        ("all lower-case letters", "qwertyuiop".to_string()),
        ("letters and digits mixed", "aB3dE5gH7j".to_string()),
        ("another profile's version", other_version.clone()),
    ];

    for (case, version) in &unknown_versions {
        assert_ne!(
            version, &subject_version,
            "case {case:?}: the row must name a version the subject profile does not have, \
             got the minted one {version:?}"
        );

        let revoked = revoke_profile(&router, "Subject_Profile", version).await;
        assert_eq!(
            revoked.status, 404,
            "case {case:?}: RevokeSigningProfile expected HTTP 404, got {} {:?}",
            revoked.status, revoked.body
        );
        assert_eq!(
            revoked.code(),
            Some(ERROR_CODE_NOT_FOUND),
            "case {case:?}: RevokeSigningProfile expected code {ERROR_CODE_NOT_FOUND}, got {:?}",
            revoked.body
        );
        assert!(
            revoked.message().contains("Subject_Profile") && revoked.message().contains(version),
            "case {case:?}: expected the message to name the profile and the version, got {:?}",
            revoked.message()
        );

        // The identical condition on the sibling that also resolves a version:
        // same status, same code, same prose.
        let granted = add_permission(
            &router,
            "Subject_Profile",
            Some(version),
            &format!("stmt-{case}"),
        )
        .await;
        assert_eq!(
            granted.status, revoked.status,
            "case {case:?}: AddProfilePermission expected the same status as \
             RevokeSigningProfile ({}), got {} {:?}",
            revoked.status, granted.status, granted.body
        );
        assert_eq!(
            granted.code(),
            Some(ERROR_CODE_NOT_FOUND),
            "case {case:?}: AddProfilePermission expected code {ERROR_CODE_NOT_FOUND}, got {:?}",
            granted.body
        );
        assert_eq!(
            granted.message(),
            revoked.message(),
            "case {case:?}: expected one condition to read the same from both operations, got {:?}",
            granted.message()
        );
    }

    // And the profile every refusal named is untouched: still Active, still
    // carrying no revocation record, and holding no permission policy.
    let got = get_profile(&router, "Subject_Profile").await;
    assert_eq!(
        got.str_member("status"),
        "Active",
        "expected the refusals to have written nothing, got {:?}",
        got.body
    );
    assert!(
        got.body["revocationRecord"].is_null(),
        "expected the refusals to have recorded no revocation, got {:?}",
        got.body
    );
    let listed = list_permissions(&router, "Subject_Profile").await;
    assert_eq!(
        listed.status, 404,
        "expected the refused grants to have written no statement, so no policy to read, \
         got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.code(),
        Some(ERROR_CODE_POLICY_NOT_FOUND),
        "expected the refused grants to leave no policy, got {:?}",
        listed.body
    );

    // The version the profile DOES have is revocable — the refusals above are
    // about the named version, not about the operation.
    let revoked = revoke_profile(&router, "Subject_Profile", &subject_version).await;
    assert_eq!(
        revoked.status, 204,
        "revoking the minted version: expected HTTP 204, got {} {:?}",
        revoked.status, revoked.body
    );
}

/// A profile name this account holds no profile under keeps the profile-not-found
/// code, whichever operation refused it: the discriminator tells "no such
/// profile" from "no such part of it".
#[tokio::test]
async fn an_unknown_profile_keeps_the_profile_not_found_code() {
    let router = emulator();
    let version = seed_profile(&router, "Present_Profile").await;

    // The rows differ along the make-up of the name (`ProfileName` is
    // `@pattern ^[a-zA-Z0-9_]{2,}$`, `@length 2..64`): the shortest admitted
    // name, an underscored one, a digits-only one and the longest admitted one.
    let absent_names: Vec<(&str, String)> = vec![
        ("the shortest admitted name", "Ab".to_string()),
        ("an underscored name", "No_Such_Profile".to_string()),
        ("a digits-only name", "90210".to_string()),
        ("the longest admitted name", "L".repeat(64)),
    ];

    for (case, name) in &absent_names {
        let revoked = revoke_profile(&router, name, &version).await;
        let granted = add_permission(&router, name, Some(&version), "stmt-absent").await;
        let got = get_profile(&router, name).await;
        let listed = list_permissions(&router, name).await;
        let removed = remove_permission(&router, name, "stmt-absent", "rev").await;

        for (operation, answer) in [
            ("RevokeSigningProfile", &revoked),
            ("AddProfilePermission", &granted),
            ("GetSigningProfile", &got),
            ("ListProfilePermissions", &listed),
            ("RemoveProfilePermission", &removed),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}: {operation} expected HTTP 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                Some(ERROR_CODE_PROFILE_NOT_FOUND),
                "case {case:?}: {operation} expected code {ERROR_CODE_PROFILE_NOT_FOUND}, got {:?}",
                answer.body
            );
        }
    }
}

/// The other sub-resource of a profile — a statement of its permission policy —
/// carries the same sub-resource code, so the rule is the resource's, not one
/// operation's.
#[tokio::test]
async fn an_unknown_statement_id_is_refused_as_a_sub_resource() {
    let router = emulator();
    seed_profile(&router, "Policy_Profile").await;
    let granted = add_permission(&router, "Policy_Profile", None, "stmt-present").await;
    assert_eq!(
        granted.status, 200,
        "seeding a statement: expected 200, got {} {:?}",
        granted.status, granted.body
    );
    let revision = granted.str_member("revisionId").to_string();

    // The rows differ along the form of the statement id: a plain one, one
    // carrying characters an `@httpLabel` has to encode, and one differing from
    // the present statement only in case.
    for (case, statement_id) in [
        ("a plain id the policy lacks", "stmt-absent"),
        ("an id needing URI encoding", "stmt absent/one"),
        ("the present id in another case", "STMT-PRESENT"),
    ] {
        let removed = remove_permission(&router, "Policy_Profile", statement_id, &revision).await;
        assert_eq!(
            removed.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.code(),
            Some(ERROR_CODE_NOT_FOUND),
            "case {case:?}: expected code {ERROR_CODE_NOT_FOUND}, got {:?}",
            removed.body
        );
    }

    // The statement the policy DOES hold is removable with that same revision.
    let removed = remove_permission(&router, "Policy_Profile", "stmt-present", &revision).await;
    assert_eq!(
        removed.status, 200,
        "removing the present statement: expected 200, got {} {:?}",
        removed.status, removed.body
    );
}
