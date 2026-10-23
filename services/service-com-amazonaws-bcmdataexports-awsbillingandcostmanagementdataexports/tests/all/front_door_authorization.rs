// SPDX-License-Identifier: CC0-1.0

//! This service's AUTHORIZATION, over the awsJson1_1 wire.
//!
//! The rule under test: an operation whose model declares `AccessDeniedException` (HTTP
//! 403) serves only a caller whose identity-based IAM policies allow that operation's
//! action on the resource it names, and refuses every other caller with that modelled
//! error BEFORE reading or writing anything. Those operations are `CreateExport`,
//! `TagResource`, `UntagResource` and `ListTagsForResource` (smithy_query: the only ones
//! whose error list carries `AccessDeniedException`); an operation whose model declares no
//! 403 is not judged, because answering a caller with an error its operation does not
//! model would be a different divergence.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine — `CreateRole` + `PutRolePolicy` + `AssumeRole`,
//! `CreateUser` + `PutUserPolicy` + `CreateAccessKey` — so the documents this service
//! reads are the ones those services wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//! Both principal kinds are covered: an assumed-role session (resolved through STS's
//! store) and an IAM user signing with a long-term key (resolved through IAM's own
//! `access_keys` map). An access key id neither store holds is the account's own published
//! credential, which holds no policies and is allowed — the position AWS account root
//! credentials are in.
//!
//! It is asserted over a TABLE that differs along what the rule generalises over: the kind
//! of principal (the account's own published credential, an IAM user, an assumed-role
//! session), what its policy says (nothing at all, an `Allow` for this action, an `Allow`
//! for a different action, an `Allow` for a different resource, a wildcard `Allow`, and an
//! explicit `Deny` beside an `Allow`), and the operation asked for.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::export_lifecycle::{cases, error_code};

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: STS resolves an
/// id it never minted to the account's ambient identity, which holds no IAM policies and
/// is allowed, exactly as AWS account root credentials are.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the principals and their policies are made with — the same set
/// `core/router`'s `build_router` assembles, narrowed to what this rule touches.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One call to this service, with `access_key_id` in the SigV4 credential scope.
    async fn call(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSBillingAndCostManagementDataExports.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20261021/{REGION}/\
                     bcm-data-exports/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One awsQuery call to IAM or STS.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20261021/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create an IAM role with `policy` as its inline policy (when one is given), assume
    /// it through STS, and answer the session's access key id.
    async fn assumed_role_key(&self, role_name: &str, policy: Option<&str>) -> String {
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}",
            ::engine::emulator_account_id()
        );
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "putting the inline policy of {role_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!(
                            "arn:aws:iam::{}:role/{role_name}",
                            ::engine::emulator_account_id()
                        ),
                    ),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "assuming {role_name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {body}"))
    }

    /// Create an IAM user with `policy` as its inline policy (when one is given) and
    /// answer the access key id of a long-term key minted for it.
    async fn user_key(&self, user_name: &str, policy: Option<&str>) -> String {
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the user {user_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "PutUserPolicy",
                    &[
                        ("UserName", user_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "putting the inline policy of {user_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "minting a key for {user_name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey must answer an AccessKeyId, got {body}"))
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A policy document allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// A policy document allowing every data-exports action on everything, and explicitly
/// denying `denied`.
fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"bcm-data-exports:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// The `CreateExport` request for the export called `name`.
fn create_request(name: &str) -> Value {
    let mut request = cases().remove(0).create_request();
    request["Export"]
        .as_object_mut()
        .expect("Export is a JSON object")
        .insert("Name".to_string(), json!(name));
    request
}

/// The ARN this service mints for the export called `name` in [`REGION`].
fn export_arn(name: &str) -> String {
    ::engine::emulator_arn("bcm-data-exports", REGION, &format!("export/{name}"))
}

#[tokio::test]
async fn a_create_is_served_only_to_a_principal_whose_policy_allows_it() {
    let emulator = Emulator::new();

    // One row per kind of principal and per thing its policy can say. `Some(name)` is the
    // export the row creates, so each allowed row writes its own.
    let rows: Vec<(&str, Option<String>, bool)> = vec![
        // The account's own credential holds no IAM policies and is allowed, like AWS
        // account root credentials.
        ("the account's own credential", None, true),
        // An assumed-role session with no policy at all: nothing allows the action.
        (
            "an assumed-role session with no policy",
            Some(
                emulator
                    .assumed_role_key("bcmde-no-policy", None)
                    .await,
            ),
            false,
        ),
        // The same session shape, with a policy that allows exactly this action on
        // exactly this export.
        (
            "an assumed-role session allowed this action",
            Some(
                emulator
                    .assumed_role_key(
                        "bcmde-allowed",
                        Some(&allow(
                            "bcm-data-exports:CreateExport",
                            &export_arn("role_allowed_export"),
                        )),
                    )
                    .await,
            ),
            true,
        ),
        // A wildcard grant over the service's actions and resources.
        (
            "an assumed-role session with a wildcard grant",
            Some(
                emulator
                    .assumed_role_key(
                        "bcmde-wildcard",
                        Some(&allow("bcm-data-exports:*", "*")),
                    )
                    .await,
            ),
            true,
        ),
        // A grant for a DIFFERENT action of the same service.
        (
            "an assumed-role session allowed another action",
            Some(
                emulator
                    .assumed_role_key(
                        "bcmde-other-action",
                        Some(&allow("bcm-data-exports:GetExport", "*")),
                    )
                    .await,
            ),
            false,
        ),
        // A grant for this action on a DIFFERENT resource.
        (
            "an assumed-role session allowed another resource",
            Some(
                emulator
                    .assumed_role_key(
                        "bcmde-other-resource",
                        Some(&allow(
                            "bcm-data-exports:CreateExport",
                            &export_arn("somebody_elses_export"),
                        )),
                    )
                    .await,
            ),
            false,
        ),
        // An explicit deny beside a wildcard allow: the deny wins.
        (
            "an assumed-role session explicitly denied",
            Some(
                emulator
                    .assumed_role_key(
                        "bcmde-denied",
                        Some(&allow_all_but("bcm-data-exports:CreateExport")),
                    )
                    .await,
            ),
            false,
        ),
        // An IAM USER signing with a long-term key IAM minted for it: judged on its own
        // inline policies, with and without a grant.
        (
            "an IAM user with no policy",
            Some(emulator.user_key("bcmde-user-bare", None).await),
            false,
        ),
        (
            "an IAM user allowed this action",
            Some(
                emulator
                    .user_key(
                        "bcmde-user-allowed",
                        Some(&allow("bcm-data-exports:CreateExport", "*")),
                    )
                    .await,
            ),
            true,
        ),
        (
            "an IAM user explicitly denied",
            Some(
                emulator
                    .user_key(
                        "bcmde-user-denied",
                        Some(&allow_all_but("bcm-data-exports:CreateExport")),
                    )
                    .await,
            ),
            false,
        ),
        // An access key id neither STS nor IAM holds a record for: the account's own
        // published credential, which holds no policies and is allowed.
        (
            "an id no store holds",
            Some("AKIDNOBODYHOLDS".to_string()),
            true,
        ),
    ];

    let mut created = 0;
    for (index, (label, key, allowed)) in rows.iter().enumerate() {
        let access_key_id = key.clone().unwrap_or_else(|| ACCOUNT_KEY.to_string());
        let name = if *label == "an assumed-role session allowed this action" {
            "role_allowed_export".to_string()
        } else {
            format!("authorized_export_{index}")
        };
        let (status, body) = emulator
            .call(&access_key_id, "CreateExport", &create_request(&name))
            .await;
        if *allowed {
            assert_eq!(
                status, 200,
                "case {label:?}: an allowed caller must be served, got {status} with {body}"
            );
            created += 1;
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {label:?}: a caller with no grant must be refused, got {status} with {body}"
            );
            // The refusal names the principal, the action and the resource it was decided
            // about — what a caller has to fix its policy from.
            let message = body["Message"]
                .as_str()
                .or_else(|| body["message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains("bcm-data-exports:CreateExport")
                    && message.contains("not authorized to perform"),
                "case {label:?}: the refusal must name the action it was decided about, got {message:?}"
            );
        }
    }

    // And nothing a refused caller asked for happened: exactly the allowed rows' exports
    // exist.
    let (status, listed) = emulator.call(ACCOUNT_KEY, "ListExports", &json!({})).await;
    assert_eq!(status, 200, "ListExports must be served, got {listed}");
    assert_eq!(
        listed["Exports"].as_array().map(Vec::len).unwrap_or_default(),
        created,
        "a refused create must store nothing, got {listed}"
    );
}

#[tokio::test]
async fn the_tagging_operations_are_authorized_on_their_own_actions() {
    let emulator = Emulator::new();

    // An export of this account, created by the account's own credential.
    let (status, created) = emulator
        .call(ACCOUNT_KEY, "CreateExport", &create_request("tagged_export"))
        .await;
    assert_eq!(status, 200, "the setup create must succeed, got {created}");
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // A session granted ONE tagging action is served that one and refused the other, so
    // each operation is judged on its OWN action rather than on a service-wide grant.
    let read_only = emulator
        .assumed_role_key(
            "bcmde-tag-reader",
            Some(&allow("bcm-data-exports:ListTagsForResource", "*")),
        )
        .await;
    let (status, body) = emulator
        .call(
            &read_only,
            "ListTagsForResource",
            &json!({ "ResourceArn": arn }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the granted tagging read must be served, got {status} with {body}"
    );
    let (status, body) = emulator
        .call(
            &read_only,
            "TagResource",
            &json!({ "ResourceArn": arn, "ResourceTags": [{ "Key": "env", "Value": "dev" }] }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "the ungranted tagging write must be refused, got {status} with {body}"
    );

    // The write really did not happen.
    let (status, tags) = emulator
        .call(ACCOUNT_KEY, "ListTagsForResource", &json!({ "ResourceArn": arn }))
        .await;
    assert_eq!(status, 200, "the account's own read must be served, got {tags}");
    assert!(
        !serde_json::to_string(&tags["ResourceTags"])
            .unwrap_or_default()
            .contains("\"env\""),
        "a refused TagResource must not have tagged the export, got {tags}"
    );

    // Authorization precedes the ARN and parameter checks: a refused caller is told it is
    // refused, not that its ARN is unusable.
    let (status, body) = emulator
        .call(
            &read_only,
            "TagResource",
            &json!({
                "ResourceArn": "arn:aws:bcm-data-exports:us-east-1:111122223333:export/other",
                "ResourceTags": [{ "Key": "env", "Value": "dev" }],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "authorization comes before the ARN check, got {status} with {body}"
    );
}

/// An operation whose model declares no `AccessDeniedException` is not judged by this
/// layer: answering a caller with an error its operation does not model would be its own
/// divergence. Those operations are served from state for every principal.
#[tokio::test]
async fn an_operation_that_models_no_403_is_not_judged() {
    let emulator = Emulator::new();

    let (status, created) = emulator
        .call(ACCOUNT_KEY, "CreateExport", &create_request("unjudged_export"))
        .await;
    assert_eq!(status, 200, "the setup create must succeed, got {created}");
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    let bare = emulator.assumed_role_key("bcmde-unjudged", None).await;
    for (label, operation, body) in [
        ("a read", "GetExport", json!({ "ExportArn": arn })),
        ("a list", "ListExports", json!({})),
        (
            "a catalog read",
            "GetTable",
            json!({ "TableName": "COST_AND_USAGE_REPORT" }),
        ),
        ("a catalog list", "ListTables", json!({})),
    ] {
        let (status, answer) = emulator.call(&bare, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: {operation} models no AccessDeniedException, so it must answer \
             from state, got {status} with {answer}"
        );
    }
}
