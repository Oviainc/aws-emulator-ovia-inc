// SPDX-License-Identifier: CC0-1.0

//! The tenant surface, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: `CreateTenant` MINTS a tenant — its id, its ARN and its
//! creation instant — persists it with the name, tags and suppression preferences the
//! request carried, and answers from that stored record, so the tenant's own read
//! reports exactly what the create returned; a request whose `TenantName` is outside the
//! documented form, or whose name the account already holds, is refused with the
//! modelled terminal error for that class and stores nothing. The tables below span what
//! the rule generalises over: the name (shortest allowed, at the documented 64-character
//! bound, every documented character class), the tag set (none, one, several, values
//! outside ASCII), the suppression preferences (absent, tenant scope with one reason,
//! tenant scope with both reasons, account scope named explicitly), the signing region,
//! and each refusal leg.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulated ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// The longest `TenantName` SES accepts, taken from the crate's own constant so the
/// bound is stated once: its documentation states the name "can contain up to 64
/// alphanumeric characters, including letters, numbers, hyphens (-) and underscores
/// (_) only" (`smithy_query(query="shape",shape="TenantName")`).
const MAX_TENANT_NAME_CHARS: usize =
    service_com_amazonaws_sesv2_simpleemailservice_v2::state::MAX_TENANT_NAME_CHARS;

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A string member of the body, or `""` when absent.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, path: &str, region: &str, body: Value) -> Reply {
        let payload = bytes::Bytes::from(serde_json::to_vec(&body).expect("serialize body"));
        let host = format!("email.{region}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name, and what names the region an ARN is minted in; the signature itself
            // is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{region}/ses/aws4_request, SignedHeaders=host, Signature=00"),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
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
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create_tenant_in(&self, region: &str, body: Value) -> Reply {
        self.call("/v2/email/tenants", region, body).await
    }

    async fn create_tenant(&self, body: Value) -> Reply {
        self.create_tenant_in("us-east-1", body).await
    }

    async fn get_tenant(&self, name: &str) -> Reply {
        self.call(
            "/v2/email/tenants/get",
            "us-east-1",
            json!({"TenantName": name}),
        )
        .await
    }
}

/// One accepted `CreateTenant`: the request to send and what the stored tenant must
/// report afterwards.
struct CreateCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `TenantName` to create.
    name: String,
    /// `Tags`, in the order the request supplies them.
    tags: Vec<(&'static str, &'static str)>,
    /// The `SuppressionAttributes` the request carries, when it carries any.
    suppression: Option<Value>,
    /// The `SuppressionScope` the stored tenant must report.
    expected_scope: &'static str,
    /// The `SuppressedReasons` the stored tenant must report, in order; empty means the
    /// member must be absent.
    expected_reasons: Vec<&'static str>,
}

/// The accepted-create table: one row per thing the rule generalises over.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "the shortest name the model allows, no tags, no suppression preferences",
            name: "a".to_string(),
            tags: Vec::new(),
            suppression: None,
            // `TenantSuppressionAttributes$SuppressionScope`: "if you don't specify a
            // suppression scope, the tenant defaults to ACCOUNT scope".
            expected_scope: "ACCOUNT",
            expected_reasons: Vec::new(),
        },
        CreateCase {
            case: "a name at the documented 64-character bound",
            name: format!("tenant-{}", "x".repeat(MAX_TENANT_NAME_CHARS - "tenant-".len())),
            tags: Vec::new(),
            suppression: None,
            expected_scope: "ACCOUNT",
            expected_reasons: Vec::new(),
        },
        CreateCase {
            case: "a name using every documented character class",
            name: "Tenant_01-beta".to_string(),
            tags: vec![("team", "growth")],
            suppression: None,
            expected_scope: "ACCOUNT",
            expected_reasons: Vec::new(),
        },
        CreateCase {
            case: "several tags, which must keep the order they were supplied in",
            name: "tagged-tenant".to_string(),
            tags: vec![("team", "growth"), ("env", "prod"), ("owner", "ops")],
            suppression: None,
            expected_scope: "ACCOUNT",
            expected_reasons: Vec::new(),
        },
        CreateCase {
            case: "a tag key and value outside the ASCII range",
            name: "unicode-tags".to_string(),
            tags: vec![("тема", "Ωmega — 日本語")],
            suppression: None,
            expected_scope: "ACCOUNT",
            expected_reasons: Vec::new(),
        },
        CreateCase {
            case: "tenant-level suppression with one reason",
            name: "suppress-bounce".to_string(),
            tags: Vec::new(),
            suppression: Some(json!({
                "SuppressionScope": "TENANT",
                "SuppressedReasons": ["BOUNCE"]
            })),
            expected_scope: "TENANT",
            expected_reasons: vec!["BOUNCE"],
        },
        CreateCase {
            case: "tenant-level suppression with both reasons, in the order supplied",
            name: "suppress-both".to_string(),
            tags: vec![("env", "staging")],
            suppression: Some(json!({
                "SuppressionScope": "TENANT",
                "SuppressedReasons": ["COMPLAINT", "BOUNCE"]
            })),
            expected_scope: "TENANT",
            // `SuppressedReasons` is a list, so it is reported in the order supplied.
            expected_reasons: vec!["COMPLAINT", "BOUNCE"],
        },
        CreateCase {
            case: "account-level suppression named explicitly, with no reasons",
            name: "suppress-account".to_string(),
            tags: Vec::new(),
            suppression: Some(json!({"SuppressionScope": "ACCOUNT"})),
            expected_scope: "ACCOUNT",
            expected_reasons: Vec::new(),
        },
    ]
}

/// The request body one row sends.
fn create_body(case: &CreateCase) -> Value {
    let mut body = json!({"TenantName": case.name});
    if !case.tags.is_empty() {
        body["Tags"] = Value::Array(
            case.tags
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        );
    }
    if let Some(suppression) = &case.suppression {
        body["SuppressionAttributes"] = suppression.clone();
    }
    body
}

/// The `(Key, Value)` pairs a `TagList` member reports, in the order reported.
fn tag_pairs(value: Option<&Value>) -> Vec<(String, String)> {
    value
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    (
                        row.get("Key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        row.get("Value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `SuppressedReasons` a `TenantSuppressionAttributes` member reports, in order.
fn suppressed_reasons(suppression: Option<&Value>) -> Vec<String> {
    suppression
        .and_then(|value| value.get("SuppressedReasons"))
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| row.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_created_tenant_round_trips_through_get_tenant() {
    let ses = Ses::new();
    let mut minted_ids = BTreeSet::new();

    for case in create_cases() {
        let created = ses.create_tenant(create_body(&case)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
            case.case, created.status, created.body
        );
        assert_eq!(
            created.text("TenantName"),
            case.name,
            "case {:?}: CreateTenant must echo the stored name, got {:?}",
            case.case,
            created.body
        );
        let tenant_id = created.text("TenantId");
        assert!(
            !tenant_id.is_empty(),
            "case {:?}: CreateTenant must mint a TenantId, got {:?}",
            case.case,
            created.body
        );
        assert_eq!(
            created.text("TenantArn"),
            format!(
                "arn:aws:ses:us-east-1:{ACCOUNT_ID}:tenant/{}/{tenant_id}",
                case.name
            ),
            "case {:?}: CreateTenant must mint the tenant's ARN over its name and its \
             minted id, got {:?}",
            case.case,
            created.body
        );
        assert!(
            minted_ids.insert(tenant_id.clone()),
            "case {:?}: every minted TenantId must be unique, but {tenant_id:?} was \
             handed out twice",
            case.case
        );
        assert!(
            created
                .body
                .get("CreatedTimestamp")
                .and_then(Value::as_f64)
                .is_some(),
            "case {:?}: CreateTenant must report when the tenant was created, got {:?}",
            case.case,
            created.body
        );
        assert_eq!(
            created.text("SendingStatus"),
            "ENABLED",
            "case {:?}: a new tenant may send, got {:?}",
            case.case,
            created.body
        );
        let expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            tag_pairs(created.body.get("Tags")),
            expected_tags,
            "case {:?}: CreateTenant must report the tags it stored, in order, got {:?}",
            case.case,
            created.body
        );
        let suppression = created.body.get("SuppressionAttributes");
        assert_eq!(
            suppression
                .and_then(|value| value.get("SuppressionScope"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
            case.expected_scope,
            "case {:?}: CreateTenant must report the tenant's suppression scope, got {:?}",
            case.case,
            created.body
        );
        let expected_reasons: Vec<String> = case
            .expected_reasons
            .iter()
            .map(|reason| reason.to_string())
            .collect();
        assert_eq!(
            suppressed_reasons(suppression),
            expected_reasons,
            "case {:?}: CreateTenant must report the suppression reasons it stored, in \
             order, got {:?}",
            case.case,
            created.body
        );

        // The round trip: `CreateTenantResponse` and `Tenant` carry the same members, so
        // the read must answer the create's body member for member — including the
        // minted id and the creation instant, which are read back off the record rather
        // than recomputed.
        let read = ses.get_tenant(&case.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetTenant, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            read.body.get("Tenant"),
            Some(&created.body),
            "case {:?}: GetTenant must report the record CreateTenant persisted, got {:?}",
            case.case,
            read.body
        );

        // Reading does not change what a later caller sees.
        let again = ses.get_tenant(&case.name).await;
        assert_eq!(
            again.body.get("Tenant"),
            read.body.get("Tenant"),
            "case {:?}: two reads of one tenant must agree, got {:?} then {:?}",
            case.case,
            read.body,
            again.body
        );
    }
}

#[tokio::test]
async fn a_tenant_arn_carries_the_region_the_request_was_signed_for() {
    // One row per region a request may be signed for; the ARN's region segment is the
    // credential scope's, never a fixed one.
    for (case, region) in [
        ("the emulator's default region", "us-east-1"),
        ("a region in another continent", "eu-west-1"),
        ("a region with a numbered suffix", "ap-southeast-2"),
    ] {
        let ses = Ses::new();
        let created = ses
            .create_tenant_in(region, json!({"TenantName": "regional"}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("TenantArn"),
            format!(
                "arn:aws:ses:{region}:{ACCOUNT_ID}:tenant/regional/{}",
                created.text("TenantId")
            ),
            "case {case:?}: the ARN must carry the signing region, got {:?}",
            created.body
        );
    }
}

#[tokio::test]
async fn a_tenant_name_outside_the_documented_form_is_refused() {
    let ses = Ses::new();

    // Each row is a name the documented form excludes: too long, or carrying a character
    // that is not a letter, a digit, a hyphen or an underscore.
    for (case, name) in [
        (
            "one character past the documented bound",
            "x".repeat(MAX_TENANT_NAME_CHARS + 1),
        ),
        ("a space", "has space".to_string()),
        ("a dot", "dot.name".to_string()),
        ("a slash", "slash/name".to_string()),
        ("a plus sign", "plus+name".to_string()),
        ("a character outside the ASCII range", "ünïcode".to_string()),
    ] {
        let refused = ses.create_tenant(json!({"TenantName": name})).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from CreateTenant, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?} {:?}",
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.message().contains(&name),
            "case {case:?}: the refusal must name the tenant it refused, got {:?}",
            refused.message()
        );

        // The read refuses the same malformed name on the same documented-form check,
        // so no tenant under that name is reachable through it either.
        let read = ses.get_tenant(&name).await;
        assert_eq!(
            read.status, 400,
            "case {case:?}: GetTenant must refuse the same malformed name, got {} {:?}",
            read.status, read.body
        );
    }

    // The published bound itself is accepted, and so is the shortest allowed name: the
    // refusal is on the far side of the bound only.
    for (case, name) in [
        ("the shortest allowed name", "t".to_string()),
        (
            "a name exactly at the documented bound",
            "y".repeat(MAX_TENANT_NAME_CHARS),
        ),
    ] {
        let accepted = ses.create_tenant(json!({"TenantName": name})).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
            accepted.status, accepted.body
        );
    }
}

#[tokio::test]
async fn a_tenant_name_the_account_already_holds_is_refused_and_the_stored_tenant_is_kept() {
    let ses = Ses::new();
    let first = ses
        .create_tenant(json!({
            "TenantName": "marketing",
            "Tags": [{"Key": "team", "Value": "growth"}]
        }))
        .await;
    assert_eq!(
        first.status, 200,
        "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
        "marketing", first.status, first.body
    );

    let again = ses
        .create_tenant(json!({
            "TenantName": "marketing",
            "Tags": [{"Key": "team", "Value": "someone-else"}]
        }))
        .await;
    assert_eq!(
        again.status, 400,
        "case {:?}: expected HTTP 400 from the duplicate CreateTenant, got {} {:?}",
        "marketing", again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "AlreadyExistsException",
        "case {:?}: expected AlreadyExistsException, got {:?} {:?}",
        "marketing",
        again.error_code(),
        again.body
    );
    assert!(
        again.message().contains("marketing"),
        "case {:?}: the refusal must name the tenant, got {:?}",
        "marketing",
        again.message()
    );

    // The refused call changed nothing: the tenant still reports the first create's
    // record, id and tags included.
    let read = ses.get_tenant("marketing").await;
    assert_eq!(
        read.body.get("Tenant"),
        Some(&first.body),
        "case {:?}: a refused duplicate must leave the stored tenant untouched, got {:?}",
        "marketing",
        read.body
    );
}

#[tokio::test]
async fn get_tenant_refuses_a_tenant_the_account_does_not_hold() {
    let ses = Ses::new();
    // A name this account never created, and a name it created and that is spelled with
    // different case: the store is keyed by the name as supplied.
    let created = ses.create_tenant(json!({"TenantName": "Mixed_Case"})).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateTenant, got {} {:?}",
        "Mixed_Case", created.status, created.body
    );

    for (case, name) in [
        ("a tenant nothing ever created", "never-created"),
        ("the same name in another case", "mixed_case"),
    ] {
        let read = ses.get_tenant(name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected HTTP 404 from GetTenant, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?} {:?}",
            read.error_code(),
            read.body
        );
        assert_eq!(
            read.message(),
            format!("The requested tenant <{name}> does not exist."),
            "case {case:?}: the refusal must name the tenant it did not find, got {:?}",
            read.message()
        );
    }
}
