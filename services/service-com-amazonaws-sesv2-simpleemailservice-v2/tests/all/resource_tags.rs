// SPDX-License-Identifier: CC0-1.0

//! The tagging read surface — `ListTagsForResource` — end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: `ListTagsForResource` reports exactly the tags stored on
//! the resource its `ResourceArn` names — resolved by the resource TYPE and the id the
//! ARN's resource field carries, against the records of the account the call is
//! authenticated for in the Region its credential scope names — in the order the tags
//! were supplied, and it changes nothing a later caller sees; a value that is no SES
//! resource ARN, or that names no resource within one, is refused as invalid input,
//! and an SES resource ARN resolving to no record is refused with the modelled
//! terminal 404.
//!
//! The accepted table below spans what the rule generalises over: the KIND of resource
//! the ARN names (contact list, email identity by address and by domain, configuration
//! set, tenant, each created through its own operation), the number of tags stored
//! (none, one, several, the documented maximum), the order they were supplied in
//! (which is not their sorted order), the characters a key and a value carry, an empty
//! tag value, and an ARN spelling a domain identity in another case. The refusal table
//! spans the form legs (no ARN at all, another vendor, a bare resource type, an empty
//! resource id) and the resolution legs (a name never created, a resource kind nothing
//! creates here, another account, another Region, another resource type under a name
//! that exists, and a tenant id that is not the stored tenant's).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// `ListTagsForResource` binds `ResourceArn` to `@httpQuery`, and an ARN carries `:` and
// `/` — characters a query string gives its own meaning to — so the value is escaped
// the way an SDK escapes it, by the one escape this test binary shares.
use crate::contact_lists::query_escaped;

/// The canonical account id every emulated ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// An account id that is not this emulator's.
const OTHER_ACCOUNT_ID: &str = "999999999999";

/// The Region every call in this suite is signed for, so the Region an ARN it resolves
/// has to carry.
const HOME: &str = "us-east-1";

/// A Region these calls are not signed for.
const AWAY: &str = "eu-west-1";

/// Tags one resource may carry, taken from the crate's own constant so the bound is
/// stated once: the model's `Tag` documentation states "A resource can have as many as
/// 50 tags" (`smithy_query(query="shape",shape="Tag")`).
const MAX_TAGS_PER_RESOURCE: usize =
    service_com_amazonaws_sesv2_simpleemailservice_v2::state::MAX_TAGS_PER_RESOURCE;

/// The sentence SES answers a `ResourceArn` in no SES resource ARN form, spelled out
/// here rather than read from the crate, so a change to the handler's wording fails
/// these rows.
const NOT_AN_SES_RESOURCE_ARN: &str = "Provided ARN is not in SES resource ARN format";

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

    /// The `Tags` the response reported, as key/value pairs in the order reported.
    fn tags(&self, member: &str) -> Vec<(String, String)> {
        self.body
            .get(member)
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .map(|tag| {
                        (
                            tag.get("Key")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            tag.get("Value")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the response carries a `Tags` member at all.
    fn has_tags(&self) -> bool {
        self.body.get("Tags").is_some()
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

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let host = format!("email.{HOME}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name, and what names the Region the call executes in; the signature itself
            // is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, SignedHeaders=host, Signature=00"),
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

    /// `ListTagsForResource` for `resource_arn`, which the operation binds to
    /// `@httpQuery`.
    async fn list_tags(&self, resource_arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/tags?ResourceArn={}", query_escaped(resource_arn)),
            None,
        )
        .await
    }

    async fn get_contact_list(&self, name: &str) -> Reply {
        self.call("GET", &format!("/v2/email/contact-lists/{name}"), None)
            .await
    }

    /// Create the row's resource with the tags it carries, and answer the ARN that
    /// addresses it.
    async fn create(&self, case: &TagCase) -> String {
        let tags: Vec<Value> = case
            .tags
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect();
        let (path, body, member) = match case.kind {
            Kind::ContactList => (
                "/v2/email/contact-lists",
                json!({"ContactListName": case.name, "Tags": tags}),
                "ContactListName",
            ),
            Kind::EmailIdentity => (
                "/v2/email/identities",
                json!({"EmailIdentity": case.name, "Tags": tags}),
                "EmailIdentity",
            ),
            Kind::ConfigurationSet => (
                "/v2/email/configuration-sets",
                json!({"ConfigurationSetName": case.name, "Tags": tags}),
                "ConfigurationSetName",
            ),
            Kind::Tenant => (
                "/v2/email/tenants",
                json!({"TenantName": case.name, "Tags": tags}),
                "TenantName",
            ),
        };
        let reply = self.call("POST", path, Some(body)).await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: creating the {member} {:?} answered {} {:?}",
            case.case,
            case.name,
            reply.status,
            reply.message()
        );
        match case.kind {
            // A tenant's ARN carries the id SES minted for it, so the ARN that addresses
            // one is the ARN the create answered with.
            Kind::Tenant => reply.text("TenantArn"),
            _ => arn(HOME, ACCOUNT_ID, case.kind.resource_type(), case.addressed_as()),
        }
    }
}

/// `arn:aws:ses:<region>:<account>:<type>/<id>`, the form SES writes a resource of its
/// own in.
fn arn(region: &str, account: &str, resource_type: &str, resource_id: &str) -> String {
    format!("arn:aws:ses:{region}:{account}:{resource_type}/{resource_id}")
}

/// The kind of resource a row tags, which decides the operation that creates it and the
/// resource type its ARN carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ContactList,
    EmailIdentity,
    ConfigurationSet,
    Tenant,
}

impl Kind {
    /// The resource-type token SES writes this kind of resource under in the resource
    /// field of its ARN, as the service's ARN forms for its own resources state them:
    /// `identity/<identity>`, `contact-list/<name>`, `configuration-set/<name>` and
    /// `tenant/<name>/<id>`.
    fn resource_type(self) -> &'static str {
        match self {
            Self::ContactList => "contact-list",
            Self::EmailIdentity => "identity",
            Self::ConfigurationSet => "configuration-set",
            Self::Tenant => "tenant",
        }
    }
}

/// One accepted row: the resource to create with its tags, and the id the ARN addresses
/// it by.
struct TagCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    kind: Kind,
    /// The name or identity the resource is created with.
    name: &'static str,
    /// The id the ARN spells, when the row varies it from `name`.
    addressed_by: Option<&'static str>,
    /// The tags the create supplies, in the order it supplies them.
    tags: Vec<(String, String)>,
}

impl TagCase {
    fn addressed_as(&self) -> &str {
        self.addressed_by.unwrap_or(self.name)
    }
}

/// A tag pair, as a row states one.
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// The accepted table: the resource kinds the rule spans, each with a tag set that
/// varies the count, the order and the characters.
fn accepted_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            case: "a contact list with two tags",
            kind: Kind::ContactList,
            name: "weekly-digest",
            addressed_by: None,
            tags: vec![tag("owner", "marketing"), tag("env", "prod")],
        },
        TagCase {
            case: "a contact list with no tags at all",
            kind: Kind::ContactList,
            name: "untagged",
            addressed_by: None,
            tags: Vec::new(),
        },
        TagCase {
            case: "a contact list with one tag whose value is empty",
            kind: Kind::ContactList,
            // `Tag$Value` documents "The minimum length is 0 characters", so a tag may
            // carry a key and no value.
            name: "empty-value",
            addressed_by: None,
            tags: vec![tag("retired", "")],
        },
        TagCase {
            case: "a contact list whose tags were not supplied in sorted order",
            kind: Kind::ContactList,
            name: "ordered",
            addressed_by: None,
            tags: vec![tag("zone", "c"), tag("alpha", "a"), tag("mid", "b")],
        },
        TagCase {
            case: "a contact list whose tags use the documented symbol set",
            kind: Kind::ContactList,
            // `Tag`'s documentation: "The characters can be Unicode letters, digits,
            // white space, or one of the following symbols: _ . : / = + -".
            name: "symbols",
            addressed_by: None,
            tags: vec![
                tag("cost_center.1:a/b", "x=y+z-1"),
                tag("team name", "platform ops"),
            ],
        },
        TagCase {
            case: "a contact list carrying the documented maximum number of tags",
            kind: Kind::ContactList,
            name: "fully-tagged",
            addressed_by: None,
            // Supplied in descending key order, so the reported order is the supplied
            // one rather than the sorted one.
            tags: (0..MAX_TAGS_PER_RESOURCE)
                .rev()
                .map(|index| tag(&format!("key-{index:02}"), &format!("value-{index:02}")))
                .collect(),
        },
        TagCase {
            case: "an email identity that is an address",
            kind: Kind::EmailIdentity,
            name: "sender@example.com",
            addressed_by: None,
            tags: vec![tag("purpose", "transactional")],
        },
        TagCase {
            case: "an email identity that is a domain, addressed in another case",
            kind: Kind::EmailIdentity,
            name: "example.org",
            // A domain is compared the way DNS compares labels, so an ARN spelling it in
            // another case names the same identity.
            addressed_by: Some("EXAMPLE.ORG"),
            tags: vec![tag("owner", "web"), tag("env", "staging")],
        },
        TagCase {
            case: "a configuration set",
            kind: Kind::ConfigurationSet,
            name: "nightly",
            addressed_by: None,
            tags: vec![tag("schedule", "nightly")],
        },
        TagCase {
            case: "a tenant, addressed by the ARN its create minted",
            kind: Kind::Tenant,
            name: "tenant_one",
            addressed_by: None,
            tags: vec![tag("tier", "gold")],
        },
    ]
}

#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_stored_on_the_resource_the_arn_names() {
    for case in accepted_cases() {
        let ses = Ses::new();
        let resource_arn = ses.create(&case).await;

        let reply = ses.list_tags(&resource_arn).await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: ListTagsForResource for {resource_arn:?} answered {} {:?} ({:?})",
            case.case,
            reply.status,
            reply.error_code(),
            reply.message()
        );
        assert!(
            reply.has_tags(),
            "case {:?}: Tags is a required output member, but the response carried none: {:?}",
            case.case,
            reply.body
        );
        let reported = reply.tags("Tags");
        assert_eq!(
            reported, case.tags,
            "case {:?}: expected the stored tags {:?} in the order supplied, got {reported:?}",
            case.case, case.tags
        );
    }
}

#[tokio::test]
async fn list_tags_for_resource_agrees_with_the_resource_read_that_reports_the_same_tags() {
    // The one stored tag set has two readers, so a row of the accepted table pins that
    // both report it: `GetContactList` carries no `Tags` member at all for a list that
    // carries none, while `ListTagsForResource` always reports a list.
    for case in accepted_cases()
        .into_iter()
        .filter(|case| case.kind == Kind::ContactList)
    {
        let ses = Ses::new();
        let resource_arn = ses.create(&case).await;

        let read = ses.get_contact_list(case.name).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: GetContactList answered {} {:?}",
            case.case,
            read.status,
            read.message()
        );
        let listed = ses.list_tags(&resource_arn).await.tags("Tags");
        let on_the_resource = read.tags("Tags");
        assert_eq!(
            listed, on_the_resource,
            "case {:?}: ListTagsForResource reported {listed:?} but GetContactList reported {on_the_resource:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn reading_a_resources_tags_leaves_every_later_caller_seeing_what_it_saw() {
    let ses = Ses::new();
    let case = TagCase {
        case: "a contact list read twice",
        kind: Kind::ContactList,
        name: "stable",
        addressed_by: None,
        tags: vec![tag("owner", "marketing"), tag("env", "prod")],
    };
    let resource_arn = ses.create(&case).await;

    let first = ses.list_tags(&resource_arn).await.tags("Tags");
    let second = ses.list_tags(&resource_arn).await.tags("Tags");
    assert_eq!(
        first, case.tags,
        "case {:?}: the first read reported {first:?}, expected {:?}",
        case.case, case.tags
    );
    assert_eq!(
        second, first,
        "case {:?}: a second read reported {second:?}, expected the {first:?} the first read did",
        case.case
    );
    let after = ses.get_contact_list(case.name).await.tags("Tags");
    assert_eq!(
        after, case.tags,
        "case {:?}: after two reads GetContactList reported {after:?}, expected {:?}",
        case.case, case.tags
    );
}

/// One refused row: the `ResourceArn` to ask for and the refusal it earns.
struct RefusalCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    resource_arn: String,
    status: u16,
    error_code: &'static str,
    message: String,
}

#[tokio::test]
async fn list_tags_for_resource_refuses_an_arn_it_cannot_resolve() {
    let ses = Ses::new();
    let held = TagCase {
        case: "the one resource the account holds",
        kind: Kind::ContactList,
        name: "news",
        addressed_by: None,
        tags: vec![tag("owner", "marketing")],
    };
    let held_arn = ses.create(&held).await;
    let tenant = TagCase {
        case: "the one tenant the account holds",
        kind: Kind::Tenant,
        name: "tenant_one",
        addressed_by: None,
        tags: vec![tag("tier", "gold")],
    };
    let tenant_arn = ses.create(&tenant).await;
    // The stored tenant's ARN with its id replaced: the name is the account's, the id is
    // not, so the value names no tenant this account holds.
    let other_tenant_id_arn = format!(
        "{}/not-the-stored-id",
        tenant_arn
            .rsplit_once('/')
            .expect("a tenant ARN carries its name and its id")
            .0
    );

    let cases = vec![
        RefusalCase {
            case: "a value that is no ARN at all",
            resource_arn: held.name.to_string(),
            status: 400,
            error_code: "BadRequestException",
            message: NOT_AN_SES_RESOURCE_ARN.to_string(),
        },
        RefusalCase {
            case: "an ARN of another vendor",
            resource_arn: format!("arn:aws:sqs:{HOME}:{ACCOUNT_ID}:queue/news"),
            status: 400,
            error_code: "BadRequestException",
            message: NOT_AN_SES_RESOURCE_ARN.to_string(),
        },
        RefusalCase {
            case: "an SES ARN carrying a bare resource type",
            resource_arn: format!("arn:aws:ses:{HOME}:{ACCOUNT_ID}:contact-list"),
            status: 400,
            error_code: "BadRequestException",
            message: NOT_AN_SES_RESOURCE_ARN.to_string(),
        },
        RefusalCase {
            case: "an SES ARN carrying an empty resource id",
            resource_arn: arn(HOME, ACCOUNT_ID, "contact-list", ""),
            status: 400,
            error_code: "BadRequestException",
            message: NOT_AN_SES_RESOURCE_ARN.to_string(),
        },
        RefusalCase {
            case: "a contact list the account never created",
            resource_arn: arn(HOME, ACCOUNT_ID, "contact-list", "never-created"),
            status: 404,
            error_code: "NotFoundException",
            message: format!(
                "Resource {} does not exist.",
                arn(HOME, ACCOUNT_ID, "contact-list", "never-created")
            ),
        },
        RefusalCase {
            case: "an email identity the account never created",
            resource_arn: arn(HOME, ACCOUNT_ID, "identity", "sender@example.net"),
            status: 404,
            error_code: "NotFoundException",
            message: format!(
                "Resource {} does not exist.",
                arn(HOME, ACCOUNT_ID, "identity", "sender@example.net")
            ),
        },
        RefusalCase {
            case: "another resource type under a name the account does hold",
            resource_arn: arn(HOME, ACCOUNT_ID, "configuration-set", held.name),
            status: 404,
            error_code: "NotFoundException",
            message: format!(
                "Resource {} does not exist.",
                arn(HOME, ACCOUNT_ID, "configuration-set", held.name)
            ),
        },
        RefusalCase {
            case: "a resource kind nothing creates in this emulator yet",
            resource_arn: arn(HOME, ACCOUNT_ID, "template", "welcome"),
            status: 404,
            error_code: "NotFoundException",
            message: format!(
                "Resource {} does not exist.",
                arn(HOME, ACCOUNT_ID, "template", "welcome")
            ),
        },
        RefusalCase {
            case: "the held resource's name under another account",
            resource_arn: arn(HOME, OTHER_ACCOUNT_ID, "contact-list", held.name),
            status: 404,
            error_code: "NotFoundException",
            message: format!(
                "Resource {} does not exist.",
                arn(HOME, OTHER_ACCOUNT_ID, "contact-list", held.name)
            ),
        },
        RefusalCase {
            case: "the held resource's name in another Region",
            resource_arn: arn(AWAY, ACCOUNT_ID, "contact-list", held.name),
            status: 404,
            error_code: "NotFoundException",
            message: format!(
                "Resource {} does not exist.",
                arn(AWAY, ACCOUNT_ID, "contact-list", held.name)
            ),
        },
        RefusalCase {
            case: "the held tenant's name with an id that is not the stored tenant's",
            resource_arn: other_tenant_id_arn.clone(),
            status: 404,
            error_code: "NotFoundException",
            message: format!("Resource {other_tenant_id_arn} does not exist."),
        },
    ];

    for case in &cases {
        let reply = ses.list_tags(&case.resource_arn).await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: ListTagsForResource for {:?} answered {} {:?}, expected {}",
            case.case,
            case.resource_arn,
            reply.status,
            reply.message(),
            case.status
        );
        assert_eq!(
            reply.error_code(),
            case.error_code,
            "case {:?}: expected the modelled {} , got {:?}",
            case.case,
            case.error_code,
            reply.error_code()
        );
        assert_eq!(
            reply.message(),
            case.message,
            "case {:?}: expected the message {:?}, got {:?}",
            case.case,
            case.message,
            reply.message()
        );
        assert!(
            !reply.has_tags(),
            "case {:?}: a refusal reported tags: {:?}",
            case.case,
            reply.body
        );
    }

    // Every refusal left the held resources exactly as they were, so a refused read
    // persists nothing.
    let still_held = ses.list_tags(&held_arn).await.tags("Tags");
    assert_eq!(
        still_held, held.tags,
        "after the refusals the held contact list reported {still_held:?}, expected {:?}",
        held.tags
    );
    let still_tenant = ses.list_tags(&tenant_arn).await.tags("Tags");
    assert_eq!(
        still_tenant, tenant.tags,
        "after the refusals the held tenant reported {still_tenant:?}, expected {:?}",
        tenant.tags
    );
}
