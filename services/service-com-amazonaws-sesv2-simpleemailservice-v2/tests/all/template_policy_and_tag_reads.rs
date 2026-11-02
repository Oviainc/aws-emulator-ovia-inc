// SPDX-License-Identifier: CC0-1.0

//! What a read of this service reports, and under which media type, end to end over
//! HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! Four rules are pinned here, each over a table whose rows differ along what the rule
//! generalises over:
//!
//! 1. the media type a response announces is the one the REQUEST declared for its own
//!    body, and this front door's own `application/x-amz-json-1.1` for a request that
//!    declared none (a `GET` or a `DELETE` carrying no body) — on a success and on a
//!    refusal alike;
//! 2. a read of a resource this service holds reports the resource's tag set as the LIST
//!    it is, empty when the resource carries no tag;
//! 3. a delete removes the resource from the store, so every later read of that name is
//!    refused as absent and a create under the name is accepted again;
//! 4. the sentence a refusal carries: a `ResourceArn` naming no resource the account
//!    holds is reported by the resource KIND and the NAME the ARN addresses it by, a
//!    duplicate sending-authorization policy by the policy name alone, and a sending
//!    authorization policy is reported back with the insignificant whitespace between
//!    its JSON tokens dropped.

use std::fmt::Write as _;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every row calls in, which is the Region the ARNs below name.
const HOME: &str = "us-east-1";

/// The media type the `restJson1` protocol trait this service's model declares implies,
/// which a request that declares one of its own declares.
const REST_JSON: &str = "application/json";

/// The media type this front door announces on a response to a request that declared
/// none of its own.
const ANNOUNCED: &str = "application/x-amz-json-1.1";

/// One answered request: the status, the media type it announced, the error shape when
/// there was one, and the parsed body.
struct Reply {
    status: u16,
    content_type: Option<String>,
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

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member of the body, or `Value::Null` when absent.
    fn member(&self, member: &str) -> Value {
        self.body.get(member).cloned().unwrap_or(Value::Null)
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
        // These cases exercise handler and serializer behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// The call every helper below goes through: `declared` is the media type the
    /// request announces for its own body, or `None` for a request that announces none,
    /// which is what a bodyless `GET` or `DELETE` sends.
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        declared: Option<&str>,
    ) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let host = format!("email.{HOME}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name, and what names the Region the call executes in; the signature itself
            // is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, SignedHeaders=host, Signature=00"),
            );
        if let Some(declared) = declared {
            builder = builder.header("content-type", declared);
        }
        let request = builder.body(payload).expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header("content-type");
        let error_type = header("x-amzn-errortype");
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            content_type,
            error_type,
            body,
        }
    }

    /// A request carrying a JSON body, which declares that body's media type the way
    /// every SDK does.
    async fn with_body(&self, method: &str, path: &str, body: Value) -> Reply {
        self.send(method, path, Some(body), Some(REST_JSON)).await
    }

    /// A request carrying no body at all, which declares no media type.
    async fn without_body(&self, method: &str, path: &str) -> Reply {
        self.send(method, path, None, None).await
    }

    async fn create_template(&self, name: &str, tags: &[(&str, &str)]) -> Reply {
        let mut body = json!({
            "TemplateName": name,
            "TemplateContent": {"Subject": "Your receipt", "Text": "Thanks."},
        });
        body["Tags"] = tag_list(tags);
        self.with_body("POST", "/v2/email/templates", body).await
    }

    async fn get_template(&self, name: &str) -> Reply {
        self.without_body("GET", &format!("/v2/email/templates/{name}"))
            .await
    }

    async fn delete_template(&self, name: &str) -> Reply {
        self.without_body("DELETE", &format!("/v2/email/templates/{name}"))
            .await
    }

    async fn create_contact_list(&self, name: &str, tags: &[(&str, &str)]) -> Reply {
        let mut body = json!({"ContactListName": name});
        body["Tags"] = tag_list(tags);
        self.with_body("POST", "/v2/email/contact-lists", body).await
    }

    async fn get_contact_list(&self, name: &str) -> Reply {
        self.without_body("GET", &format!("/v2/email/contact-lists/{name}"))
            .await
    }

    async fn create_configuration_set(&self, name: &str, tags: &[(&str, &str)]) -> Reply {
        let mut body = json!({"ConfigurationSetName": name});
        body["Tags"] = tag_list(tags);
        self.with_body("POST", "/v2/email/configuration-sets", body)
            .await
    }

    async fn get_configuration_set(&self, name: &str) -> Reply {
        self.without_body("GET", &format!("/v2/email/configuration-sets/{name}"))
            .await
    }

    async fn create_identity(&self, identity: &str, tags: &[(&str, &str)]) -> Reply {
        let mut body = json!({"EmailIdentity": identity});
        body["Tags"] = tag_list(tags);
        self.with_body("POST", "/v2/email/identities", body).await
    }

    async fn get_identity(&self, identity: &str) -> Reply {
        self.without_body("GET", &format!("/v2/email/identities/{identity}"))
            .await
    }

    async fn create_dedicated_ip_pool(&self, name: &str) -> Reply {
        self.with_body(
            "POST",
            "/v2/email/dedicated-ip-pools",
            json!({"PoolName": name}),
        )
        .await
    }

    async fn create_policy(&self, identity: &str, name: &str, document: &str) -> Reply {
        self.with_body(
            "POST",
            &format!("/v2/email/identities/{identity}/policies/{name}"),
            json!({"Policy": document}),
        )
        .await
    }

    async fn get_policies(&self, identity: &str) -> Reply {
        self.without_body("GET", &format!("/v2/email/identities/{identity}/policies"))
            .await
    }

    async fn list_tags(&self, resource_arn: &str) -> Reply {
        self.without_body(
            "GET",
            &format!("/v2/email/tags?ResourceArn={}", query_escaped(resource_arn)),
        )
        .await
    }

    async fn tag_resource(&self, resource_arn: &str, tags: &[(&str, &str)]) -> Reply {
        self.with_body(
            "POST",
            "/v2/email/tags",
            json!({"ResourceArn": resource_arn, "Tags": tag_list(tags)}),
        )
        .await
    }

    async fn untag_resource(&self, resource_arn: &str, keys: &[&str]) -> Reply {
        let mut query = format!("ResourceArn={}", query_escaped(resource_arn));
        for key in keys {
            write!(query, "&TagKeys={}", query_escaped(key)).expect("writing to a String");
        }
        self.without_body("DELETE", &format!("/v2/email/tags?{query}"))
            .await
    }
}

/// Everything outside the unreserved set escaped byte by byte, the way an SDK escapes a
/// value it puts in a query string.
fn query_escaped(token: &str) -> String {
    let mut escaped = String::with_capacity(token.len());
    for byte in token.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                escaped.push(char::from(*byte));
            }
            other => write!(escaped, "%{other:02X}").expect("writing to a String cannot fail"),
        }
    }
    escaped
}

/// The `Tags` list a request sends for `pairs`, which is also the list a read reports.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// `arn:aws:ses:<region>:<account>:<type>/<id>`, the form SES writes a resource of its
/// own in.
fn arn(resource_type: &str, resource_id: &str) -> String {
    format!(
        "arn:aws:ses:{HOME}:{}:{resource_type}/{resource_id}",
        emulator_server::emulator_account_id()
    )
}

/// A sending-authorization policy whose statement authorizes `principal`, written in the
/// layout `layout` renders: the document's own tokens are the same in every row, so what
/// differs between rows is only the whitespace between them.
fn policy(principal: &str, layout: Layout) -> String {
    let compact = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Sid\":\"AllowSend\",\
         \"Effect\":\"Allow\",\"Action\":\"ses:SendEmail\",\
         \"Principal\":{{\"AWS\":\"{principal}\"}}}}]}}"
    );
    layout.render(&compact)
}

/// How a row writes the whitespace between a document's tokens.
#[derive(Debug, Clone, Copy)]
enum Layout {
    /// No whitespace at all, which is the form a read reports.
    Compact,
    /// A space after every `:` and `,` outside a string, which is what a client that
    /// serializes a dictionary with its language's defaults sends.
    Spaced,
    /// Indented over several lines, which is what a client that pretty-prints sends.
    Indented,
    /// Wrapped in the whitespace a JSON parser ignores.
    Wrapped,
}

impl Layout {
    /// `compact` rewritten in this layout. The scan tracks string literals, so the
    /// whitespace only ever lands BETWEEN tokens and a value keeps its own.
    fn render(self, compact: &str) -> String {
        match self {
            Self::Compact => compact.to_string(),
            Self::Wrapped => format!(" \n\t{compact}\n "),
            Self::Spaced | Self::Indented => {
                let separator = match self {
                    Self::Indented => "\n    ",
                    _ => " ",
                };
                let mut rendered = String::with_capacity(compact.len() * 2);
                let mut in_string = false;
                let mut escaped = false;
                for character in compact.chars() {
                    rendered.push(character);
                    if in_string {
                        if escaped {
                            escaped = false;
                        } else if character == '\\' {
                            escaped = true;
                        } else if character == '"' {
                            in_string = false;
                        }
                        continue;
                    }
                    match character {
                        '"' => in_string = true,
                        ':' | ',' => rendered.push_str(separator),
                        _ => {}
                    }
                }
                rendered
            }
        }
    }
}

/// `document` with the whitespace between its JSON tokens dropped — the form every read
/// of a stored policy reports, spelled out here rather than read from the crate so a
/// change to the stored form fails these rows.
fn minified(document: &str) -> String {
    let mut minified = String::with_capacity(document.len());
    let mut in_string = false;
    let mut escaped = false;
    for character in document.chars() {
        if in_string {
            minified.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                minified.push(character);
            }
            ' ' | '\t' | '\r' | '\n' => {}
            _ => minified.push(character),
        }
    }
    minified
}

/// The sentence the tagging surface answers a `ResourceArn` naming no resource the
/// account holds: the resource KIND and the NAME the ARN addresses it by. Spelled out
/// here rather than read from the crate, so a change to the handler's wording fails these
/// rows.
fn not_present(kind: &str, name: &str) -> String {
    format!("No {kind} present with name: {name}")
}

/// The sentence every operation over an `EmailTemplateName` answers for a template the
/// account holds none of.
fn template_not_found(name: &str) -> String {
    format!("Template {name} does not exist.")
}

/// One row of the media-type table: the request to send, and the status and media type
/// the answer has to carry.
struct MediaTypeCase {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The media type the REQUEST declares for its own body, or `None` for a request
    /// that carries none and declares none.
    declared: Option<&'static str>,
    status: u16,
    announced: &'static str,
}

#[tokio::test]
async fn the_media_type_a_response_announces_is_the_one_the_request_declared() {
    let ses = Ses::new();
    assert_eq!(
        ses.create_template("announced", &[("purpose", "receipts")])
            .await
            .status,
        200,
        "the template every row below reads has to exist first"
    );
    let identity = "announce.example";
    assert_eq!(
        ses.create_identity(identity, &[]).await.status,
        200,
        "the identity every row below reads has to exist first"
    );
    let identity_arn = arn("identity", identity);

    // The rows span: a request with a body and one without, a success and a refusal of
    // each, the methods this surface serves (GET, POST, PUT, DELETE), and a bodyless
    // request that declares a media type anyway — which is answered under the one it
    // declared, because the rule is about what the REQUEST said and not about its method.
    let rows: Vec<MediaTypeCase> = vec![
        MediaTypeCase {
            case: "a bodyless GET that succeeds",
            method: "GET",
            path: "/v2/email/templates/announced".to_string(),
            body: None,
            declared: None,
            status: 200,
            announced: ANNOUNCED,
        },
        MediaTypeCase {
            case: "a bodyless GET that is refused",
            method: "GET",
            path: "/v2/email/templates/never-created".to_string(),
            body: None,
            declared: None,
            status: 404,
            announced: ANNOUNCED,
        },
        MediaTypeCase {
            case: "a bodyless GET over a query-bound read",
            method: "GET",
            path: format!("/v2/email/tags?ResourceArn={}", query_escaped(&identity_arn)),
            body: None,
            declared: None,
            status: 200,
            announced: ANNOUNCED,
        },
        MediaTypeCase {
            case: "a bodyless DELETE that succeeds",
            method: "DELETE",
            path: format!(
                "/v2/email/tags?ResourceArn={}&TagKeys=absent",
                query_escaped(&identity_arn)
            ),
            body: None,
            declared: None,
            status: 200,
            announced: ANNOUNCED,
        },
        MediaTypeCase {
            case: "a bodyless GET that declares a media type anyway",
            method: "GET",
            path: "/v2/email/templates/announced".to_string(),
            body: None,
            declared: Some(REST_JSON),
            status: 200,
            announced: REST_JSON,
        },
        MediaTypeCase {
            case: "a POST carrying a body that succeeds",
            method: "POST",
            path: "/v2/email/tags".to_string(),
            body: Some(
                json!({"ResourceArn": identity_arn, "Tags": tag_list(&[("env", "test")])}),
            ),
            declared: Some(REST_JSON),
            status: 200,
            announced: REST_JSON,
        },
        MediaTypeCase {
            case: "a POST carrying a body that is refused",
            method: "POST",
            path: "/v2/email/tags".to_string(),
            body: Some(json!({
                "ResourceArn": arn("identity", "never-created.example"),
                "Tags": tag_list(&[("env", "test")]),
            })),
            declared: Some(REST_JSON),
            status: 404,
            announced: REST_JSON,
        },
        MediaTypeCase {
            case: "a PUT carrying a body that succeeds",
            method: "PUT",
            path: "/v2/email/templates/announced".to_string(),
            body: Some(json!({"TemplateContent": {"Subject": "Your receipt"}})),
            declared: Some(REST_JSON),
            status: 200,
            announced: REST_JSON,
        },
        MediaTypeCase {
            case: "a bodyless DELETE that is refused",
            method: "DELETE",
            path: "/v2/email/templates/never-created".to_string(),
            body: None,
            declared: None,
            status: 404,
            announced: ANNOUNCED,
        },
    ];

    for row in rows {
        let case = row.case;
        let reply = ses
            .send(row.method, &row.path, row.body, row.declared)
            .await;
        assert_eq!(
            reply.status, row.status,
            "case {case:?}: expected HTTP {}, got {} {:?}",
            row.status,
            reply.status,
            reply.message()
        );
        assert_eq!(
            reply.content_type.as_deref(),
            Some(row.announced),
            "case {case:?}: expected content-type {:?}, got {:?}",
            row.announced,
            reply.content_type
        );
    }
}

#[tokio::test]
async fn a_read_reports_the_resources_tag_set_as_the_list_it_is() {
    // (what the row varies, the tags the create carries). The rows span the counts a tag
    // set can have — none, one, several — and a tag whose value is the empty string the
    // model admits.
    let tag_sets: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("no tags at all", Vec::new()),
        ("one tag", vec![("purpose", "receipts")]),
        (
            "several tags, in the order supplied",
            vec![("zone", "eu"), ("purpose", "receipts")],
        ),
        ("a tag whose value is empty", vec![("owner", "")]),
    ];

    for (case, tags) in tag_sets {
        let ses = Ses::new();
        let name = "read-back";
        let identity = "tagged.example";

        assert_eq!(
            ses.create_template(name, &tags).await.status,
            200,
            "case {case:?}: the template create has to be accepted"
        );
        assert_eq!(
            ses.create_contact_list(name, &tags).await.status,
            200,
            "case {case:?}: the contact-list create has to be accepted"
        );
        assert_eq!(
            ses.create_configuration_set(name, &tags).await.status,
            200,
            "case {case:?}: the configuration-set create has to be accepted"
        );
        assert_eq!(
            ses.create_identity(identity, &tags).await.status,
            200,
            "case {case:?}: the identity create has to be accepted"
        );

        // Every read of a resource this service holds, and the tagging read that reports
        // the same stored set, report the one tag set as a list.
        let reads = vec![
            ("GetEmailTemplate", ses.get_template(name).await),
            ("GetContactList", ses.get_contact_list(name).await),
            (
                "GetConfigurationSet",
                ses.get_configuration_set(name).await,
            ),
            ("GetEmailIdentity", ses.get_identity(identity).await),
            (
                "ListTagsForResource",
                ses.list_tags(&arn("identity", identity)).await,
            ),
        ];
        for (operation, reply) in reads {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: {operation} answered {} {:?}",
                reply.status,
                reply.message()
            );
            assert_eq!(
                reply.member("Tags"),
                tag_list(&tags),
                "case {case:?}: expected {operation} to report Tags {:?}, got {:?}",
                tag_list(&tags),
                reply.member("Tags")
            );
        }
    }
}

/// One row of the delete table: the name the template is created under and the tags it
/// carries, neither of which changes that the delete removes it.
struct DeleteCase {
    case: &'static str,
    name: &'static str,
    tags: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn a_deleted_email_template_is_absent_from_every_later_read() {
    // The rows span the forms an `EmailTemplateName` takes — one character, punctuation,
    // mixed case, a long name — and whether the template was created with tags, none of
    // which changes that the delete removes it.
    let rows: Vec<DeleteCase> = vec![
        DeleteCase {
            case: "a one-character name",
            name: "T",
            tags: Vec::new(),
        },
        DeleteCase {
            case: "a name of punctuation and mixed case",
            name: "Order.Receipt_v2-2026",
            tags: vec![("purpose", "receipts")],
        },
        DeleteCase {
            case: "a long name",
            name: "order-receipt-template-that-is-sixty-four-characters-long-enough",
            tags: vec![("zone", "eu"), ("purpose", "receipts")],
        },
    ];

    for DeleteCase { case, name, tags } in rows {
        let ses = Ses::new();
        assert_eq!(
            ses.create_template(name, &tags).await.status,
            200,
            "case {case:?}: the create has to be accepted"
        );
        let read = ses.get_template(name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the stored template has to be readable before the delete, got {} {:?}",
            read.status,
            read.message()
        );

        let deleted = ses.delete_template(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected the delete to be accepted, got {} {:?}",
            deleted.status,
            deleted.message()
        );

        // Every operation over the name reports the template as absent afterwards, with
        // the one sentence this service words that condition in.
        let after: Vec<(&str, Reply)> = vec![
            ("GetEmailTemplate", ses.get_template(name).await),
            ("DeleteEmailTemplate", ses.delete_template(name).await),
            (
                "UpdateEmailTemplate",
                ses.with_body(
                    "PUT",
                    &format!("/v2/email/templates/{name}"),
                    json!({"TemplateContent": {"Subject": "Your receipt"}}),
                )
                .await,
            ),
        ];
        for (operation, reply) in after {
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected {operation} to answer 404 for the deleted \
                 template, got {} {:?}",
                reply.status,
                reply.message()
            );
            assert_eq!(
                reply.error_code(),
                "NotFoundException",
                "case {case:?}: expected {operation} to answer NotFoundException, got {:?}",
                reply.error_code()
            );
            assert_eq!(
                reply.message(),
                template_not_found(name),
                "case {case:?}: expected {operation} to answer {:?}, got {:?}",
                template_not_found(name),
                reply.message()
            );
        }

        // The name is free again: a create under it is accepted and reports the content
        // and tags IT carried, not the deleted template's.
        assert_eq!(
            ses.create_template(name, &[]).await.status,
            200,
            "case {case:?}: expected a create under the deleted name to be accepted"
        );
        let recreated = ses.get_template(name).await;
        assert_eq!(
            recreated.member("Tags"),
            tag_list(&[]),
            "case {case:?}: expected the recreated template to carry no tags, got {:?}",
            recreated.member("Tags")
        );
    }
}

#[tokio::test]
async fn a_stored_identity_policy_is_reported_with_the_whitespace_between_its_tokens_dropped() {
    // (what the row varies, the principal the statement names, the layout the request
    // writes the document in). The rows span the layouts a client sends — already
    // compact, a space after every separator, indented over several lines, wrapped in
    // whitespace a parser ignores — and the principal forms the document carries.
    let rows: Vec<(&str, &str, Layout)> = vec![
        (
            "a document already carrying no whitespace",
            "arn:aws:iam::000000000000:root",
            Layout::Compact,
        ),
        (
            "a space after every separator",
            "arn:aws:iam::000000000000:user/sender",
            Layout::Spaced,
        ),
        ("an indented document", "*", Layout::Indented),
        (
            "a document wrapped in whitespace",
            "arn:aws:iam::000000000000:root",
            Layout::Wrapped,
        ),
    ];

    for (case, principal, layout) in rows {
        let ses = Ses::new();
        let identity = "policy.example";
        assert_eq!(
            ses.create_identity(identity, &[]).await.status,
            200,
            "case {case:?}: the identity has to exist first"
        );
        let document = policy(principal, layout);
        let created = ses.create_policy(identity, "AllowSend", &document).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the policy create to be accepted, got {} {:?}",
            created.status,
            created.message()
        );

        let expected = minified(&document);
        // The two readers of the one stored map answer from the same record, so both
        // report the same bytes.
        for (operation, reply) in [
            ("GetEmailIdentityPolicies", ses.get_policies(identity).await),
            ("GetEmailIdentity", ses.get_identity(identity).await),
        ] {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: {operation} answered {} {:?}",
                reply.status,
                reply.message()
            );
            assert_eq!(
                reply.member("Policies"),
                json!({"AllowSend": expected}),
                "case {case:?}: expected {operation} to report {expected:?}, got {:?}",
                reply.member("Policies")
            );
        }

        // Reported form in, reported form out: putting back what a read reported stores
        // the same bytes, so a client that round-trips a document sees no drift.
        let ses = Ses::new();
        assert_eq!(ses.create_identity(identity, &[]).await.status, 200);
        assert_eq!(
            ses.create_policy(identity, "AllowSend", &expected)
                .await
                .status,
            200,
            "case {case:?}: the reported form has to be accepted as a document too"
        );
        let again = ses.get_policies(identity).await;
        assert_eq!(
            again.member("Policies"),
            json!({"AllowSend": expected}),
            "case {case:?}: expected the reported form to be a fixed point, got {:?}",
            again.member("Policies")
        );
    }
}

#[tokio::test]
async fn a_policy_name_the_identity_already_carries_is_refused_by_name() {
    // `PolicyName` is modelled `length 1..64`, so the rows span the forms a name takes:
    // one character, dashes and underscores, mixed case, and a name of exactly 64
    // characters.
    let longest = format!("{}_{}", "a".repeat(60), "Z-9");
    let names: Vec<(&str, &str)> = vec![
        ("a one-character name", "P"),
        ("a name carrying a dash and an underscore", "allow-send_1"),
        ("a mixed-case name", "AllowPartnerSend"),
        ("the longest name the model admits", longest.as_str()),
    ];

    for (case, name) in names {
        let ses = Ses::new();
        let identity = "duplicate.example";
        assert_eq!(
            ses.create_identity(identity, &[]).await.status,
            200,
            "case {case:?}: the identity has to exist first"
        );
        let document = policy("arn:aws:iam::000000000000:root", Layout::Compact);
        assert_eq!(
            ses.create_policy(identity, name, &document).await.status,
            200,
            "case {case:?}: the first create has to be accepted"
        );

        let again = ses.create_policy(identity, name, &document).await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: expected the duplicate create to be refused 400, got {} {:?}",
            again.status,
            again.message()
        );
        assert_eq!(
            again.error_code(),
            "AlreadyExistsException",
            "case {case:?}: expected AlreadyExistsException, got {:?}",
            again.error_code()
        );
        // The sentence names the policy whose name the caller has to change, and nothing
        // else: no identity, no trailing period.
        assert_eq!(
            again.message(),
            format!("Policy <{name}> already exists"),
            "case {case:?}: expected {:?}, got {:?}",
            format!("Policy <{name}> already exists"),
            again.message()
        );

        // The refused create left the stored document exactly as it was.
        let read = ses.get_policies(identity).await;
        assert_eq!(
            read.member("Policies"),
            json!({name: document}),
            "case {case:?}: expected the stored policy to stand, got {:?}",
            read.member("Policies")
        );
    }
}

#[tokio::test]
async fn the_tagging_surface_names_the_kind_and_name_of_the_resource_it_cannot_find() {
    let ses = Ses::new();
    // One resource of each kind the account does hold, so a row that names another id of
    // the same kind is refused for the id and not for the kind.
    assert_eq!(ses.create_contact_list("news", &[]).await.status, 200);
    assert_eq!(
        ses.create_configuration_set("held-set", &[]).await.status,
        200
    );
    assert_eq!(ses.create_identity("held.example", &[]).await.status, 200);
    assert_eq!(ses.create_dedicated_ip_pool("held-pool").await.status, 200);

    // (what the row varies, the ARN resource type, the id the ARN carries, the kind the
    // sentence names). The resource types are the ones SES writes its own resources under
    // in the resource field of an ARN — `identity/<identity>`, `contact-list/<name>`,
    // `configuration-set/<name>`, `dedicated-ip-pool/<name>` — plus one this emulator
    // keeps no table for at all, which is indistinguishable from an id never created.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a domain identity the account never created",
            "identity",
            "never-created.example",
            "EmailIdentity",
        ),
        (
            "an address identity the account never created",
            "identity",
            "sender@example.net",
            "EmailIdentity",
        ),
        (
            "a contact list the account never created",
            "contact-list",
            "never-created",
            "ContactList",
        ),
        (
            "a configuration set the account never created",
            "configuration-set",
            "never-created",
            "ConfigurationSet",
        ),
        (
            "a dedicated IP pool the account never created",
            "dedicated-ip-pool",
            "never-created",
            "DedicatedIpPool",
        ),
        (
            "a resource kind nothing creates in this emulator yet",
            "template",
            "welcome",
            "Template",
        ),
        (
            "a held name under another resource kind",
            "configuration-set",
            "news",
            "ConfigurationSet",
        ),
    ];

    for (case, resource_type, resource_id, kind) in rows {
        let resource_arn = arn(resource_type, resource_id);
        let expected = not_present(kind, resource_id);
        // Every operation that resolves a `ResourceArn` to a tag set words the one
        // condition the same way.
        let answers: Vec<(&str, Reply)> = vec![
            ("ListTagsForResource", ses.list_tags(&resource_arn).await),
            (
                "TagResource",
                ses.tag_resource(&resource_arn, &[("env", "test")]).await,
            ),
            (
                "UntagResource",
                ses.untag_resource(&resource_arn, &["env"]).await,
            ),
        ];
        for (operation, reply) in answers {
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected {operation} to answer 404, got {} {:?}",
                reply.status,
                reply.message()
            );
            assert_eq!(
                reply.error_code(),
                "NotFoundException",
                "case {case:?}: expected {operation} to answer NotFoundException, got {:?}",
                reply.error_code()
            );
            assert_eq!(
                reply.message(),
                expected,
                "case {case:?}: expected {operation} to answer {expected:?}, got {:?}",
                reply.message()
            );
        }
    }
}
