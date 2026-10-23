// SPDX-License-Identifier: CC0-1.0

//! A Transfer Family user's `Role` is an ARN ANOTHER service mints, so the two
//! services have to work together for a user to exist at all.
//!
//! RULE: a client that creates an IAM role and then creates a Transfer Family user
//! with that role's ARN gets a user that every user-scoped path resolves — the create
//! answers its key, `DescribeUser` answers the stored record carrying that very ARN,
//! `ListUsers` enumerates it, and `DeleteUser` releases it — and the role it was built
//! on is released by `DeleteRole` afterwards.
//!
//! This needs BOTH services, so neither service's own test target can see it: a
//! `CreateRole` that answers 501 gives a client no ARN to pass to `CreateUser`, and the
//! user is never created — which is exactly how "the user is invisible to every
//! user-scoped read" is produced without any transfer handler being wrong.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request in this suite is signed for, and so the region the ARNs
/// the services build carry.
const REGION: &str = "us-east-1";

/// A trust policy that lets Transfer Family assume the role — the document a client
/// creating this role really sends.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"transfer.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// A SigV4 `Authorization` header whose credential scope names `signing_name`; the
/// signature is never checked, because the router here runs with request
/// authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// One awsQuery call to IAM: `Action` plus form parameters, answered with the status
/// and the XML body.
async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "emulator.local")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed IAM request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only
/// RFC 3986's unreserved characters literal.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the single `<tag>` leaf element an IAM response carries.
fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("no <{tag}> in {body}"));
    let rest = &body[start + open.len()..];
    let end = rest
        .find(&close)
        .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"));
    rest[..end].to_string()
}

/// One awsJson1_1 call to Transfer Family, answered with the status and the decoded
/// JSON body.
async fn transfer(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "emulator.local")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TransferService.{operation}"))
        .header("authorization", authorization("transfer"))
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed Transfer request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation}: the response body is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, decoded)
}

/// A member of a JSON object, or `Null` when it is absent.
fn member(value: &serde_json::Value, name: &str) -> serde_json::Value {
    value.get(name).cloned().unwrap_or(serde_json::Value::Null)
}

#[test]
fn a_transfer_user_built_on_a_created_iam_role_resolves_through_every_path() {
    std::thread::Builder::new()
        .name("transfer-user-iam-role".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // Rows: the role's path (the account root and a nested service-role path,
            // which the role ARN is built from) and the user's landing directory
            // (none, the root, a bucket path) — what the chain generalises over.
            let rows: [(&str, &str, Option<&str>, Option<&str>); 3] = [
                ("root path, no landing directory", "probe-role-1", None, None),
                ("root path, root landing directory", "probe-role-2", None, Some("/")),
                (
                    "service-role path, bucket landing directory",
                    "probe-role-3",
                    Some("/service-role/"),
                    Some("/bucket/probe"),
                ),
            ];

            runtime.block_on(async {
                for (case, role_name, role_path, home_directory) in rows {
                    // 1. The role the user will be built on.
                    let mut params = vec![
                        ("RoleName", role_name),
                        ("AssumeRolePolicyDocument", TRUST_POLICY),
                    ];
                    if let Some(path) = role_path {
                        params.push(("Path", path));
                    }
                    let (status, created_role) = iam(&router, "CreateRole", &params).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected CreateRole 200, got {status}: {created_role}"
                    );
                    let role_arn = element(&created_role, "Arn");
                    let expected_arn =
                        format!("role{}{role_name}", role_path.unwrap_or("/"));
                    assert!(
                        role_arn.starts_with("arn:aws:iam::") && role_arn.ends_with(&expected_arn),
                        "case {case:?}: expected an IAM role ARN ending in {expected_arn:?}, \
                         got {role_arn:?}"
                    );

                    // 2. The server the user is attached to.
                    let (status, created_server) =
                        transfer(&router, "CreateServer", serde_json::json!({})).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected CreateServer 200, got {status}: {created_server}"
                    );
                    let server_id = member(&created_server, "ServerId")
                        .as_str()
                        .unwrap_or_default()
                        .to_string();

                    // 3. The user, carrying the role ARN IAM just minted.
                    let user_name = format!("probe-user-{role_name}");
                    let mut create = serde_json::json!({
                        "ServerId": &server_id,
                        "UserName": &user_name,
                        "Role": &role_arn,
                    });
                    if let Some(directory) = home_directory {
                        create["HomeDirectory"] = serde_json::json!(directory);
                    }
                    let (status, created_user) = transfer(&router, "CreateUser", create).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected CreateUser 200, got {status}: {created_user}"
                    );

                    // 4. Every user-scoped read resolves it, carrying that ARN.
                    let (status, described) = transfer(
                        &router,
                        "DescribeUser",
                        serde_json::json!({"ServerId": &server_id, "UserName": &user_name}),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected DescribeUser 200, got {status}: {described}"
                    );
                    let user = member(&described, "User");
                    assert_eq!(
                        member(&user, "Role"),
                        serde_json::json!(&role_arn),
                        "case {case:?}: expected the user to report the created role ARN, \
                         got {user}"
                    );
                    assert_eq!(
                        member(&user, "HomeDirectory"),
                        home_directory.map_or(serde_json::Value::Null, |directory| {
                            serde_json::json!(directory)
                        }),
                        "case {case:?}: expected the landing directory the create supplied, \
                         got {user}"
                    );

                    let (status, listing) = transfer(
                        &router,
                        "ListUsers",
                        serde_json::json!({"ServerId": &server_id}),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected ListUsers 200, got {status}: {listing}"
                    );
                    let listed = member(&listing, "Users");
                    let names: Vec<&str> = listed
                        .as_array()
                        .map(|users| {
                            users
                                .iter()
                                .filter_map(|entry| entry.get("UserName").and_then(|n| n.as_str()))
                                .collect()
                        })
                        .unwrap_or_default();
                    assert_eq!(
                        names,
                        vec![user_name.as_str()],
                        "case {case:?}: expected ListUsers to enumerate the created user, \
                         got {listed}"
                    );

                    // 5. Teardown releases the user and then the role it was built on.
                    let (status, deleted) = transfer(
                        &router,
                        "DeleteUser",
                        serde_json::json!({"ServerId": &server_id, "UserName": &user_name}),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected DeleteUser 200, got {status}: {deleted}"
                    );
                    let (status, gone) = transfer(
                        &router,
                        "DescribeUser",
                        serde_json::json!({"ServerId": &server_id, "UserName": &user_name}),
                    )
                    .await;
                    assert!(
                        (400..500).contains(&status),
                        "case {case:?}: expected DescribeUser to refuse a deleted user with a \
                         4xx, got {status}: {gone}"
                    );

                    let (status, body) =
                        iam(&router, "DeleteRole", &[("RoleName", role_name)]).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected DeleteRole 200, got {status}: {body}"
                    );
                    let (status, body) = iam(&router, "GetRole", &[("RoleName", role_name)]).await;
                    assert_eq!(
                        status, 404,
                        "case {case:?}: expected GetRole 404 for a deleted role, got {status}: \
                         {body}"
                    );
                }
            });
        })
        .expect("spawn the sized test thread")
        .join()
        .expect("the test thread must not panic");
}
