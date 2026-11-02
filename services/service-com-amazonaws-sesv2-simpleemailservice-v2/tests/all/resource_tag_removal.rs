// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization of the `DELETE /v2/email/tags`
//! query binding, the handler and serialization are all exercised.
//!
//! The rule these cases pin: `UntagResource` removes from the resource its `ResourceArn`
//! names — resolved by the resource TYPE and the id the ARN's resource field carries,
//! against the records of the account the call is authenticated for in the Region its
//! credential scope names — exactly the tags whose key the request names, since a tag is
//! identified by its key and "when you specify a tag key, the action removes both that
//! key and its associated tag value"; every tag the request does not name stays on the
//! resource in the order it was stored, and a key the resource does not carry names no
//! stored tag, so it removes nothing and the call still succeeds. A value that is no SES
//! resource ARN, or that names no resource within one, is refused as invalid input, and
//! an SES resource ARN resolving to no record of this account in this Region is refused
//! with the modelled terminal 404 — neither having written anything.
//!
//! The accepted table spans what the rule generalises over: the KIND of resource the ARN
//! names (contact list, email identity by address and by domain spelled in another case,
//! configuration set, tenant, dedicated IP pool, each created through its own
//! operation), how many of the stored keys the request names (one, several, all, none of
//! them, more than the resource carries), where in the stored order they sit (first,
//! last, interleaved), the count of tags stored (none, one, several, the documented
//! maximum), the order the keys are named in (which is neither the stored nor the sorted
//! one), repetition of one key within a request, the characters a key carries (the
//! documented symbol set, white space, a key outside ASCII), a tag whose value is empty,
//! and a key differing from a stored one only in case. The refusal table spans the form
//! legs (no ARN at all, another vendor, a bare resource type, an empty resource id) and
//! the resolution legs (a name never created, a resource kind nothing creates here,
//! another resource type under a name that exists, another account, another Region, and
//! a tenant id that is not the stored tenant's).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// `UntagResource` binds both its members to `@httpQuery`, and the values are an ARN and
// tag keys — which carry `:`, `/`, `=`, `+` and white space, characters a query string
// gives its own meaning to, and which the model's `Tag` documentation allows to be
// "Unicode letters" — so they are escaped the way an SDK escapes them, by the one escape
// this test binary shares.
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

/// The sentence the tagging surface answers a `ResourceArn` naming no resource the
/// account holds: the resource KIND and the NAME the ARN addresses it by, never the whole
/// ARN. Spelled out here rather than read from the crate, so a change to the handler's
/// wording fails these rows.
fn not_present(kind: &str, name: &str) -> String {
    format!("No {kind} present with name: {name}")
}

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
    fn tags(&self) -> Vec<(String, String)> {
        self.body
            .get("Tags")
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

    /// `UntagResource` for `resource_arn` naming `keys`, both of which the operation
    /// binds to `@httpQuery`: the keys are repeated across the query string, as the
    /// model's own example spells them
    /// (`/v2/email/tags?ResourceArn=ResourceArn&TagKeys=Key1&TagKeys=Key2`).
    async fn untag_resource(&self, resource_arn: &str, keys: &[String]) -> Reply {
        let mut query = format!("ResourceArn={}", query_escaped(resource_arn));
        for key in keys {
            query.push_str(&format!("&TagKeys={}", query_escaped(key)));
        }
        self.call("DELETE", &format!("/v2/email/tags?{query}"), None)
            .await
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

    /// Create the row's resource with the tags it starts out carrying, and answer the ARN
    /// that addresses it.
    async fn create(&self, kind: Kind, name: &str, tags: &[(String, String)]) -> String {
        let tags: Vec<Value> = tags
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect();
        let (path, body) = match kind {
            Kind::ContactList => (
                "/v2/email/contact-lists",
                json!({"ContactListName": name, "Tags": tags}),
            ),
            Kind::EmailIdentity => (
                "/v2/email/identities",
                json!({"EmailIdentity": name, "Tags": tags}),
            ),
            Kind::ConfigurationSet => (
                "/v2/email/configuration-sets",
                json!({"ConfigurationSetName": name, "Tags": tags}),
            ),
            Kind::Tenant => ("/v2/email/tenants", json!({"TenantName": name, "Tags": tags})),
            Kind::DedicatedIpPool => (
                "/v2/email/dedicated-ip-pools",
                json!({"PoolName": name, "Tags": tags}),
            ),
        };
        let reply = self.call("POST", path, Some(body)).await;
        assert_eq!(
            reply.status,
            200,
            "creating the {kind:?} {name:?} answered {} {:?}",
            reply.status,
            reply.message()
        );
        match kind {
            // A tenant's ARN carries the id SES minted for it, so the ARN that addresses
            // one is the ARN the create answered with.
            Kind::Tenant => reply.text("TenantArn"),
            _ => arn(HOME, ACCOUNT_ID, kind.resource_type(), name),
        }
    }
}

/// `arn:aws:ses:<region>:<account>:<type>/<id>`, the form SES writes a resource of its
/// own in.
fn arn(region: &str, account: &str, resource_type: &str, resource_id: &str) -> String {
    format!("arn:aws:ses:{region}:{account}:{resource_type}/{resource_id}")
}

/// The kind of resource a row removes tags from, which decides the operation that creates
/// it and the resource type its ARN carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ContactList,
    EmailIdentity,
    ConfigurationSet,
    Tenant,
    DedicatedIpPool,
}

impl Kind {
    /// The resource-type token SES writes this kind of resource under in the resource
    /// field of its ARN, as the service's ARN forms for its own resources state them:
    /// `identity/<identity>`, `contact-list/<name>`, `configuration-set/<name>`,
    /// `tenant/<name>/<id>` and `dedicated-ip-pool/<name>`.
    fn resource_type(self) -> &'static str {
        match self {
            Self::ContactList => "contact-list",
            Self::EmailIdentity => "identity",
            Self::ConfigurationSet => "configuration-set",
            Self::Tenant => "tenant",
            Self::DedicatedIpPool => "dedicated-ip-pool",
        }
    }
}

/// A tag pair, as a row states one.
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// `count` distinct tags, keyed in ascending order, for a row that varies the COUNT.
fn generated_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| tag(&format!("key-{index:02}"), &format!("value-{index:02}")))
        .collect()
}

/// One accepted row: the resource to create with the tags it starts out carrying, the tag
/// keys one `UntagResource` call names, and the tag set the resource is left carrying.
struct RemovalCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    kind: Kind,
    /// The name or identity the resource is created with.
    name: &'static str,
    /// The id the ARN spells, when the row varies it from `name`.
    addressed_by: Option<&'static str>,
    /// The tags the create supplies, in the order it supplies them.
    initial: Vec<(String, String)>,
    /// The keys the `UntagResource` call names, in the order it names them.
    removed: Vec<String>,
    /// The tags the resource carries afterwards, in the order it reports them.
    expected: Vec<(String, String)>,
}

/// The accepted table: the rule generalises over the KIND of resource the ARN names, over
/// which and how many of the stored keys a request names, over where those keys sit in the
/// stored order, and over the characters a key carries.
fn accepted_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "one of two keys removed, the other keeping its place",
            kind: Kind::ContactList,
            name: "weekly-digest",
            addressed_by: None,
            initial: vec![tag("owner", "marketing"), tag("env", "prod")],
            removed: vec!["owner".to_string()],
            expected: vec![tag("env", "prod")],
        },
        RemovalCase {
            case: "the last of two keys removed",
            kind: Kind::ContactList,
            name: "last-key",
            addressed_by: None,
            initial: vec![tag("owner", "marketing"), tag("env", "prod")],
            removed: vec!["env".to_string()],
            expected: vec![tag("owner", "marketing")],
        },
        RemovalCase {
            case: "two interleaved keys of four removed in one call",
            kind: Kind::ContactList,
            name: "interleaved",
            addressed_by: None,
            initial: vec![
                tag("alpha", "1"),
                tag("beta", "2"),
                tag("gamma", "3"),
                tag("delta", "4"),
            ],
            removed: vec!["beta".to_string(), "delta".to_string()],
            expected: vec![tag("alpha", "1"), tag("gamma", "3")],
        },
        RemovalCase {
            case: "the keys named in neither the stored nor the sorted order",
            kind: Kind::ContactList,
            name: "named-out-of-order",
            addressed_by: None,
            initial: vec![tag("alpha", "1"), tag("beta", "2"), tag("gamma", "3")],
            removed: vec!["gamma".to_string(), "alpha".to_string()],
            expected: vec![tag("beta", "2")],
        },
        RemovalCase {
            case: "every key the resource carries",
            kind: Kind::ContactList,
            name: "stripped",
            addressed_by: None,
            initial: vec![tag("owner", "marketing"), tag("env", "prod")],
            removed: vec!["owner".to_string(), "env".to_string()],
            expected: Vec::new(),
        },
        RemovalCase {
            case: "a key the resource does not carry, beside one it does",
            kind: Kind::ContactList,
            name: "partly-absent",
            addressed_by: None,
            initial: vec![tag("owner", "marketing"), tag("env", "prod")],
            removed: vec!["never-set".to_string(), "owner".to_string()],
            expected: vec![tag("env", "prod")],
        },
        RemovalCase {
            case: "only keys the resource does not carry",
            kind: Kind::ContactList,
            name: "all-absent",
            addressed_by: None,
            initial: vec![tag("owner", "marketing")],
            removed: vec!["never-set".to_string(), "also-never-set".to_string()],
            expected: vec![tag("owner", "marketing")],
        },
        RemovalCase {
            case: "a key named on a resource carrying no tags at all",
            kind: Kind::ContactList,
            name: "untagged",
            addressed_by: None,
            initial: Vec::new(),
            removed: vec!["owner".to_string()],
            expected: Vec::new(),
        },
        RemovalCase {
            case: "one key named twice in the same request",
            kind: Kind::ContactList,
            name: "named-twice",
            addressed_by: None,
            initial: vec![tag("owner", "marketing"), tag("env", "prod")],
            removed: vec!["owner".to_string(), "owner".to_string()],
            expected: vec![tag("env", "prod")],
        },
        RemovalCase {
            case: "a key whose stored value is empty",
            kind: Kind::ContactList,
            // `Tag$Value` documents "The minimum length is 0 characters", so a tag may
            // carry a key and no value — and the key is what identifies it either way.
            name: "empty-value",
            addressed_by: None,
            initial: vec![tag("retired", ""), tag("env", "prod")],
            removed: vec!["retired".to_string()],
            expected: vec![tag("env", "prod")],
        },
        RemovalCase {
            case: "keys written with the documented symbol set and with white space",
            kind: Kind::ContactList,
            // `Tag`'s documentation: "The characters can be Unicode letters, digits,
            // white space, or one of the following symbols: _ . : / = + -".
            name: "symbols",
            addressed_by: None,
            initial: vec![
                tag("cost_center.1:a/b", "x=y+z-1"),
                tag("team name", "platform ops"),
                tag("env", "prod"),
            ],
            removed: vec!["cost_center.1:a/b".to_string(), "team name".to_string()],
            expected: vec![tag("env", "prod")],
        },
        RemovalCase {
            case: "a key written outside ASCII",
            kind: Kind::ContactList,
            // `Tag`'s documentation allows "Unicode letters", so a key in any script is
            // a key this surface stores and removes.
            name: "unicode-key",
            addressed_by: None,
            initial: vec![tag("プロジェクト", "本番"), tag("env", "prod")],
            removed: vec!["プロジェクト".to_string()],
            expected: vec![tag("env", "prod")],
        },
        RemovalCase {
            case: "a key differing from the stored one only in case",
            kind: Kind::ContactList,
            name: "case-sensitive",
            addressed_by: None,
            initial: vec![tag("Env", "prod")],
            removed: vec!["env".to_string()],
            expected: vec![tag("Env", "prod")],
        },
        RemovalCase {
            case: "every other key of a resource carrying the documented maximum",
            kind: Kind::ContactList,
            name: "fully-tagged",
            addressed_by: None,
            initial: generated_tags(MAX_TAGS_PER_RESOURCE),
            removed: generated_tags(MAX_TAGS_PER_RESOURCE)
                .into_iter()
                .enumerate()
                .filter(|(index, _)| index % 2 == 0)
                .map(|(_, (key, _))| key)
                .collect(),
            expected: generated_tags(MAX_TAGS_PER_RESOURCE)
                .into_iter()
                .enumerate()
                .filter(|(index, _)| index % 2 == 1)
                .map(|(_, pair)| pair)
                .collect(),
        },
        RemovalCase {
            case: "more keys named than the resource carries",
            kind: Kind::ContactList,
            name: "over-named",
            addressed_by: None,
            initial: vec![tag("owner", "marketing")],
            removed: generated_tags(MAX_TAGS_PER_RESOURCE + 1)
                .into_iter()
                .map(|(key, _)| key)
                .chain(std::iter::once("owner".to_string()))
                .collect(),
            expected: Vec::new(),
        },
        RemovalCase {
            case: "an email identity that is an address",
            kind: Kind::EmailIdentity,
            name: "sender@example.com",
            addressed_by: None,
            initial: vec![tag("purpose", "transactional"), tag("env", "prod")],
            removed: vec!["purpose".to_string()],
            expected: vec![tag("env", "prod")],
        },
        RemovalCase {
            case: "an email identity that is a domain, addressed in another case",
            kind: Kind::EmailIdentity,
            name: "example.org",
            // A domain is compared the way DNS compares labels, so an ARN spelling it in
            // another case names the same identity.
            addressed_by: Some("EXAMPLE.ORG"),
            initial: vec![tag("owner", "web"), tag("env", "staging")],
            removed: vec!["env".to_string()],
            expected: vec![tag("owner", "web")],
        },
        RemovalCase {
            case: "a configuration set",
            kind: Kind::ConfigurationSet,
            name: "nightly",
            addressed_by: None,
            initial: vec![tag("schedule", "nightly"), tag("owner", "ops")],
            removed: vec!["schedule".to_string()],
            expected: vec![tag("owner", "ops")],
        },
        RemovalCase {
            case: "a tenant, addressed by the ARN its create minted",
            kind: Kind::Tenant,
            name: "tenant_one",
            addressed_by: None,
            initial: vec![tag("tier", "gold"), tag("env", "prod")],
            removed: vec!["tier".to_string()],
            expected: vec![tag("env", "prod")],
        },
        RemovalCase {
            case: "a dedicated IP pool",
            kind: Kind::DedicatedIpPool,
            name: "campaign-pool",
            addressed_by: None,
            initial: vec![tag("owner", "marketing"), tag("env", "prod")],
            removed: vec!["env".to_string()],
            expected: vec![tag("owner", "marketing")],
        },
    ]
}

/// The ARN the row addresses its resource by: the one its create minted, with the id
/// spelled as the row asks.
async fn created_arn(ses: &Ses, case: &RemovalCase) -> String {
    let minted = ses.create(case.kind, case.name, &case.initial).await;
    match case.addressed_by {
        Some(addressed_by) => arn(HOME, ACCOUNT_ID, case.kind.resource_type(), addressed_by),
        None => minted,
    }
}

#[tokio::test]
async fn untag_resource_removes_the_named_keys_and_leaves_every_other_tag() {
    for case in accepted_cases() {
        let ses = Ses::new();
        let resource_arn = created_arn(&ses, &case).await;

        let removed = ses.untag_resource(&resource_arn, &case.removed).await;
        assert_eq!(
            removed.status,
            200,
            "case {:?}: UntagResource answered {} {:?}, expected 200",
            case.case,
            removed.status,
            removed.message()
        );
        assert_eq!(
            removed.error_code(),
            "",
            "case {:?}: UntagResource named the error {:?}",
            case.case,
            removed.error_code()
        );

        let left = ses.list_tags(&resource_arn).await;
        assert_eq!(
            left.status,
            200,
            "case {:?}: ListTagsForResource answered {} {:?}, expected 200",
            case.case,
            left.status,
            left.message()
        );
        let left = left.tags();
        assert_eq!(
            left, case.expected,
            "case {:?}: after removing {:?} the resource reported {left:?}, expected {:?}",
            case.case, case.removed, case.expected
        );
    }
}

#[tokio::test]
async fn tags_untag_resource_removed_are_gone_from_the_resources_own_read() {
    // What a removal leaves on a resource has two readers, so the rows that remove from a
    // contact list pin that both report the same set.
    for case in accepted_cases()
        .into_iter()
        .filter(|case| case.kind == Kind::ContactList)
    {
        let ses = Ses::new();
        let resource_arn = created_arn(&ses, &case).await;
        let removed = ses.untag_resource(&resource_arn, &case.removed).await;
        assert_eq!(
            removed.status,
            200,
            "case {:?}: UntagResource answered {} {:?}",
            case.case,
            removed.status,
            removed.message()
        );

        let on_the_resource = ses.get_contact_list(case.name).await.tags();
        assert_eq!(
            on_the_resource, case.expected,
            "case {:?}: GetContactList reported {on_the_resource:?}, expected {:?}",
            case.case, case.expected
        );
    }
}

#[tokio::test]
async fn removing_the_same_keys_twice_leaves_the_resource_where_the_first_call_left_it() {
    // The end state the caller asked for is the keys being absent, so the second call
    // over the same keys changes nothing and still succeeds.
    for case in accepted_cases() {
        let ses = Ses::new();
        let resource_arn = created_arn(&ses, &case).await;

        for attempt in 1..=2 {
            let removed = ses.untag_resource(&resource_arn, &case.removed).await;
            assert_eq!(
                removed.status,
                200,
                "case {:?}: removal attempt {attempt} answered {} {:?}, expected 200",
                case.case,
                removed.status,
                removed.message()
            );
            let left = ses.list_tags(&resource_arn).await.tags();
            assert_eq!(
                left, case.expected,
                "case {:?}: after removal attempt {attempt} the resource reported \
                 {left:?}, expected {:?}",
                case.case, case.expected
            );
        }
    }
}

#[tokio::test]
async fn untag_resource_reaches_only_the_resource_its_arn_names() {
    let ses = Ses::new();
    let shared = vec![tag("owner", "marketing"), tag("env", "prod")];
    // Three resources carrying the same keys: one the removal names, one of the same kind
    // under another name, and one of another kind under the SAME name — so a removal that
    // resolved the ARN by anything less than its type and its id would show here.
    let named = ses
        .create(Kind::ContactList, "shared-name", &shared)
        .await;
    let sibling = ses.create(Kind::ContactList, "other-name", &shared).await;
    let other_kind = ses
        .create(Kind::ConfigurationSet, "shared-name", &shared)
        .await;

    let removed = ses
        .untag_resource(&named, &["owner".to_string()])
        .await;
    assert_eq!(
        removed.status,
        200,
        "UntagResource answered {} {:?}, expected 200",
        removed.status,
        removed.message()
    );

    let on_named = ses.list_tags(&named).await.tags();
    assert_eq!(
        on_named,
        vec![tag("env", "prod")],
        "case {:?}: the named contact list reported {on_named:?}",
        "the resource the ARN names"
    );
    for (case, arn) in [
        ("the contact list of another name", &sibling),
        ("the configuration set of the same name", &other_kind),
    ] {
        let untouched = ses.list_tags(arn).await.tags();
        assert_eq!(
            untouched, shared,
            "case {case:?}: expected {shared:?}, got {untouched:?}"
        );
    }
}

/// One refused row: the `ResourceArn` to send, and the refusal it earns.
struct RefusalCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    resource_arn: String,
    status: u16,
    error_code: &'static str,
    message: String,
}

#[tokio::test]
async fn untag_resource_refuses_an_arn_it_cannot_resolve_and_writes_nothing() {
    let ses = Ses::new();
    let held_name = "news";
    let held_tags = vec![tag("owner", "marketing"), tag("env", "prod")];
    let held_arn = ses
        .create(Kind::ContactList, held_name, &held_tags)
        .await;
    let tenant_tags = vec![tag("tier", "gold")];
    let tenant_arn = ses
        .create(Kind::Tenant, "tenant_one", &tenant_tags)
        .await;
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
            resource_arn: held_name.to_string(),
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
            message: not_present("ContactList", "never-created"),
        },
        RefusalCase {
            case: "an email identity the account never created",
            resource_arn: arn(HOME, ACCOUNT_ID, "identity", "sender@example.net"),
            status: 404,
            error_code: "NotFoundException",
            message: not_present("EmailIdentity", "sender@example.net"),
        },
        RefusalCase {
            case: "another resource type under a name the account does hold",
            resource_arn: arn(HOME, ACCOUNT_ID, "configuration-set", held_name),
            status: 404,
            error_code: "NotFoundException",
            message: not_present("ConfigurationSet", held_name),
        },
        RefusalCase {
            case: "a resource kind nothing creates in this emulator yet",
            resource_arn: arn(HOME, ACCOUNT_ID, "template", "welcome"),
            status: 404,
            error_code: "NotFoundException",
            message: not_present("Template", "welcome"),
        },
        RefusalCase {
            case: "the held resource's name under another account",
            resource_arn: arn(HOME, OTHER_ACCOUNT_ID, "contact-list", held_name),
            status: 404,
            error_code: "NotFoundException",
            message: not_present("ContactList", held_name),
        },
        RefusalCase {
            case: "the held resource's name in another Region",
            resource_arn: arn(AWAY, ACCOUNT_ID, "contact-list", held_name),
            status: 404,
            error_code: "NotFoundException",
            message: not_present("ContactList", held_name),
        },
        RefusalCase {
            case: "a tenant the account never created, addressed by name and id",
            resource_arn: arn(HOME, ACCOUNT_ID, "tenant", "tenant_never/tn-0123456789abcdef"),
            status: 404,
            error_code: "NotFoundException",
            message: "No Tenant present with name: tenant_neverwith tenantId: tn-0123456789abcdef"
                .to_string(),
        },
        RefusalCase {
            case: "a tenant ARN carrying a name but no id",
            resource_arn: arn(HOME, ACCOUNT_ID, "tenant", "tenant_never"),
            status: 404,
            error_code: "NotFoundException",
            // The ARN names no tenant id to report, so the tenant is named the way every
            // other kind's resource is: by its kind and the name the ARN carries.
            message: not_present("Tenant", "tenant_never"),
        },
        RefusalCase {
            case: "the held tenant's name with an id that is not the stored tenant's",
            resource_arn: other_tenant_id_arn.clone(),
            status: 404,
            error_code: "NotFoundException",
            // A tenant ARN names the tenant and its id, and SES reports a tenant ARN
            // resolving to no tenant by naming both of them rather than the whole ARN:
            // the fixed text `No Tenant present with name: ` and `with tenantId: ` run
            // together without a separator.
            message: "No Tenant present with name: tenant_onewith tenantId: not-the-stored-id"
                .to_string(),
        },
    ];

    for case in &cases {
        let reply = ses
            .untag_resource(&case.resource_arn, &["owner".to_string()])
            .await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: UntagResource for {:?} answered {} {:?}, expected {}",
            case.case,
            case.resource_arn,
            reply.status,
            reply.message(),
            case.status
        );
        assert_eq!(
            reply.error_code(),
            case.error_code,
            "case {:?}: expected the modelled {}, got {:?}",
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
    }

    // Every refusal left the held resources exactly as they were, so a refused removal
    // persists nothing.
    let still_held = ses.list_tags(&held_arn).await.tags();
    assert_eq!(
        still_held, held_tags,
        "after the refusals the held contact list reported {still_held:?}, expected {held_tags:?}"
    );
    let still_tenant = ses.list_tags(&tenant_arn).await.tags();
    assert_eq!(
        still_tenant, tenant_tags,
        "after the refusals the held tenant reported {still_tenant:?}, expected {tenant_tags:?}"
    );
}
