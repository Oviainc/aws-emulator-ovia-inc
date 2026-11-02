// SPDX-License-Identifier: CC0-1.0

//! What a client READS off this service's contact plane, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler, the error envelope and
//! the response headers are all exercised.
//!
//! Three rules are pinned here, each over a table spanning what it generalises over.
//!
//! 1. `Contact$TopicDefaultPreferences` reports the defaults APPLIED to the contact: one
//!    entry per topic of its list the contact has stated no preference for, carrying that
//!    topic's `DefaultSubscriptionStatus` ("the default subscription status to be applied
//!    to a contact if the contact has not noted their preference for subscribing to a
//!    topic"), in the order the list publishes its topics; a contact to which no default
//!    applies carries no member at all. The rows span the number of topics published
//!    (none, one, several), how many of them the contact stated a preference for (none,
//!    some, all), the two `SubscriptionStatus` values on both the list's side and the
//!    contact's, and `UnsubscribeAll` either way — and each row is read through
//!    `GetContact` AND `ListContacts`, which must agree.
//!
//! 2. A read of a resource that carries tags reports its tag set as a LIST — the EMPTY
//!    list when it carries none — rather than as no member at all. The rows span the
//!    tagged resources this service reads back (a contact list, an email template, a
//!    configuration set) against the number of tags stored (none, one, several).
//!
//! 3. The media type a response is announced under follows the framing of the REQUEST it
//!    answers: a request that framed a JSON body is answered under that body's media
//!    type, and a request that framed no body — a read, a delete — is answered under the
//!    `application/x-amz-json-1.1` this service announces, whatever the status and
//!    whichever layer rendered the response. The rows span both sides of that split
//!    against a success carrying a body, a success whose output has no members, a
//!    modelled 404, a modelled 400 a handler raises and a 400 the constraint walk raises
//!    at the boundary.
//!
//! The duplicate-contact refusal is pinned beside them: SES words a contact refusal with
//! the address the request supplied and the KIND of resource it looked in, never the
//! list's own name.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The media type this service announces on a response to a request that framed NO body.
///
/// Recorded behaviour: the model declares `restJson1`, whose generic default is
/// `application/json`, and SES API v2 answers a bodiless request — a read, a delete — in
/// the AWS JSON media type instead.
const ANNOUNCED_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The media type an SDK frames a JSON request body in, and the one a response to such a
/// request is announced under.
const REQUEST_MEDIA_TYPE: &str = "application/json";

/// One answered request: the status, the error shape when there was one, the media type
/// it was announced under, and the parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    content_type: Option<String>,
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

    /// A member of the body, or `Value::Null` when the response carries none.
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
        // These cases exercise handler and envelope behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            );
        if body.is_some() {
            // A request FRAMES the body it carries and only that: a read or a delete
            // sends no body, so it declares no media type for one — exactly as an SDK
            // sends each of them.
            request = request.header("content-type", REQUEST_MEDIA_TYPE);
        }
        let request = request.body(payload).expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let error_type = header("x-amzn-errortype");
        let content_type = header("content-type");
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            content_type,
            body,
        }
    }

    async fn create_list(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/contact-lists", Some(body)).await
    }

    async fn create_contact(&self, list: &str, body: Value) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/contact-lists/{list}/contacts"),
            Some(body),
        )
        .await
    }

    async fn get_contact(&self, list: &str, address: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/contact-lists/{list}/contacts/{address}"),
            None,
        )
        .await
    }

    async fn list_contacts(&self, list: &str) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/contact-lists/{list}/contacts/list"),
            Some(json!({})),
        )
        .await
    }
}

/// A `Topics` body member from (topic, default status) pairs. `Topic` requires
/// `TopicName`, `DisplayName` and `DefaultSubscriptionStatus`.
fn topics(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(name, default_status)| {
                json!({
                    "TopicName": name,
                    "DisplayName": format!("{name} display"),
                    "DefaultSubscriptionStatus": default_status,
                })
            })
            .collect(),
    )
}

/// A `TopicPreferences` member, or `Value::Null` for a contact that states none.
fn preferences(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(name, status)| json!({"TopicName": name, "SubscriptionStatus": status}))
            .collect(),
    )
}

/// A `TopicPreferenceList` as a read reports it, for the pairs a row expects — or
/// `Value::Null` where the row expects no member at all.
fn expected_list(pairs: &[(&str, &str)]) -> Value {
    if pairs.is_empty() {
        Value::Null
    } else {
        preferences(pairs)
    }
}

/// One row of the default-preference table.
struct DefaultsCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The topics the list publishes, each with the status it gives a contact that has
    /// stated none.
    published: &'static [(&'static str, &'static str)],
    /// The preferences the contact states.
    stated: &'static [(&'static str, &'static str)],
    /// `UnsubscribeAll` as the create supplies it.
    unsubscribe_all: bool,
    /// `TopicDefaultPreferences` as both reads must report it, in order; empty means the
    /// member must be absent.
    defaults: &'static [(&'static str, &'static str)],
}

/// The rows span the number of topics published, how many of them the contact stated a
/// preference for, both `SubscriptionStatus` values on each side, and `UnsubscribeAll`.
const DEFAULTS_CASES: &[DefaultsCase] = &[
    DefaultsCase {
        case: "no topics published, so no default exists",
        published: &[],
        stated: &[],
        unsubscribe_all: false,
        defaults: &[],
    },
    DefaultsCase {
        case: "one topic published, no preference stated",
        published: &[("news", "OPT_IN")],
        stated: &[],
        unsubscribe_all: false,
        defaults: &[("news", "OPT_IN")],
    },
    DefaultsCase {
        case: "one topic published, its default is OPT_OUT",
        published: &[("news", "OPT_OUT")],
        stated: &[],
        unsubscribe_all: false,
        defaults: &[("news", "OPT_OUT")],
    },
    DefaultsCase {
        case: "the one published topic has a stated preference agreeing with the default",
        published: &[("news", "OPT_IN")],
        stated: &[("news", "OPT_IN")],
        unsubscribe_all: false,
        defaults: &[],
    },
    DefaultsCase {
        case: "the one published topic has a stated preference against the default",
        published: &[("news", "OPT_IN")],
        stated: &[("news", "OPT_OUT")],
        unsubscribe_all: false,
        defaults: &[],
    },
    DefaultsCase {
        case: "two topics published, one of them stated",
        published: &[("news", "OPT_IN"), ("offers", "OPT_OUT")],
        stated: &[("news", "OPT_OUT")],
        unsubscribe_all: false,
        defaults: &[("offers", "OPT_OUT")],
    },
    DefaultsCase {
        case: "two topics published, the second of them stated",
        published: &[("news", "OPT_IN"), ("offers", "OPT_OUT")],
        stated: &[("offers", "OPT_IN")],
        unsubscribe_all: false,
        defaults: &[("news", "OPT_IN")],
    },
    DefaultsCase {
        case: "two topics published, both stated",
        published: &[("news", "OPT_IN"), ("offers", "OPT_OUT")],
        stated: &[("news", "OPT_OUT"), ("offers", "OPT_IN")],
        unsubscribe_all: false,
        defaults: &[],
    },
    DefaultsCase {
        case: "three topics published, none stated, reported in the published order",
        published: &[("weekly", "OPT_IN"), ("news", "OPT_OUT"), ("offers", "OPT_IN")],
        stated: &[],
        unsubscribe_all: false,
        defaults: &[("weekly", "OPT_IN"), ("news", "OPT_OUT"), ("offers", "OPT_IN")],
    },
    DefaultsCase {
        case: "three topics published, the middle one stated",
        published: &[("weekly", "OPT_IN"), ("news", "OPT_OUT"), ("offers", "OPT_IN")],
        stated: &[("news", "OPT_IN")],
        unsubscribe_all: false,
        defaults: &[("weekly", "OPT_IN"), ("offers", "OPT_IN")],
    },
    DefaultsCase {
        case: "unsubscribed from everything, every topic stated",
        published: &[("news", "OPT_IN"), ("offers", "OPT_OUT")],
        stated: &[("news", "OPT_IN"), ("offers", "OPT_IN")],
        unsubscribe_all: true,
        defaults: &[],
    },
    DefaultsCase {
        case: "unsubscribed from everything, nothing stated",
        published: &[("news", "OPT_IN"), ("offers", "OPT_OUT")],
        stated: &[],
        unsubscribe_all: true,
        defaults: &[("news", "OPT_IN"), ("offers", "OPT_OUT")],
    },
];

/// The list and contact every default-preference row uses.
const LIST: &str = "newsletter";
const CONTACT: &str = "reader@example.com";

#[tokio::test]
async fn topic_default_preferences_report_only_the_defaults_that_apply_to_the_contact() {
    for case in DEFAULTS_CASES {
        let ses = Ses::new();
        let created_list = ses
            .create_list(json!({"ContactListName": LIST, "Topics": topics(case.published)}))
            .await;
        assert_eq!(
            created_list.status, 200,
            "case {:?}: seeding the list must succeed, got {} {:?}",
            case.case, created_list.status, created_list.body
        );
        let created = ses
            .create_contact(
                LIST,
                json!({
                    "EmailAddress": CONTACT,
                    "TopicPreferences": preferences(case.stated),
                    "UnsubscribeAll": case.unsubscribe_all,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: seeding the contact must succeed, got {} {:?}",
            case.case, created.status, created.body
        );

        let expected_defaults = expected_list(case.defaults);
        let read = ses.get_contact(LIST, CONTACT).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetContact, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            read.member("TopicDefaultPreferences"),
            expected_defaults,
            "case {:?}: expected GetContact to report TopicDefaultPreferences \
             {expected_defaults:?}, got {:?}",
            case.case,
            read.member("TopicDefaultPreferences")
        );
        // The contact's own answers are untouched by the rule: what it stated is what it
        // stated, whatever the list's defaults are.
        assert_eq!(
            read.member("TopicPreferences"),
            expected_list(case.stated),
            "case {:?}: expected GetContact to report the stated preferences {:?}, got {:?}",
            case.case,
            case.stated,
            read.member("TopicPreferences")
        );

        // The listing projects the same contact, so it reports the same two members: one
        // contact cannot read differently in a listing than on its own.
        let listed = ses.list_contacts(LIST).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected HTTP 200 from ListContacts, got {} {:?}",
            case.case, listed.status, listed.body
        );
        let contacts = listed
            .member("Contacts")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            contacts.len(),
            1,
            "case {:?}: the listing must report the one seeded contact, got {:?}",
            case.case,
            listed.body
        );
        let member_in_listing = |member: &str| {
            contacts[0]
                .get(member)
                .cloned()
                .unwrap_or(Value::Null)
        };
        let in_listing = member_in_listing("TopicDefaultPreferences");
        assert_eq!(
            in_listing, expected_defaults,
            "case {:?}: expected ListContacts to report TopicDefaultPreferences \
             {expected_defaults:?}, got {in_listing:?}",
            case.case
        );
        let stated_in_listing = member_in_listing("TopicPreferences");
        assert_eq!(
            stated_in_listing,
            expected_list(case.stated),
            "case {:?}: expected ListContacts to report the stated preferences {:?}, got \
             {stated_in_listing:?}",
            case.case,
            case.stated
        );
        assert_eq!(
            (in_listing, stated_in_listing),
            (
                read.member("TopicDefaultPreferences"),
                read.member("TopicPreferences")
            ),
            "case {:?}: the listing and the contact's own read must report the same \
             preferences, got {:?} against {:?}",
            case.case,
            listed.body,
            read.body
        );
    }
}

/// One row of the tag-set table: the resource kind, and how many tags it is created with.
struct TagsCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// Which read reports the tag set.
    kind: Kind,
    /// The tags the create supplies, in the order supplied.
    tags: &'static [(&'static str, &'static str)],
}

/// The tagged resources of this service whose own read reports their tag set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    ContactList,
    EmailTemplate,
    ConfigurationSet,
}

impl Kind {
    /// The collection the create posts to, and the resource's own read path.
    fn collection(self) -> &'static str {
        match self {
            Kind::ContactList => "/v2/email/contact-lists",
            Kind::EmailTemplate => "/v2/email/templates",
            Kind::ConfigurationSet => "/v2/email/configuration-sets",
        }
    }

    /// The create body for `name`, beside the `Tags` member the row adds.
    fn create_body(self, name: &str) -> Value {
        match self {
            Kind::ContactList => json!({"ContactListName": name}),
            Kind::EmailTemplate => json!({
                "TemplateName": name,
                "TemplateContent": {"Subject": "Your receipt"},
            }),
            Kind::ConfigurationSet => json!({"ConfigurationSetName": name}),
        }
    }
}

/// The rows span the three tagged reads against the number of tags stored.
const TAGS_CASES: &[TagsCase] = &[
    TagsCase {
        case: "a contact list created with no tags",
        kind: Kind::ContactList,
        tags: &[],
    },
    TagsCase {
        case: "a contact list created with one tag",
        kind: Kind::ContactList,
        tags: &[("team", "growth")],
    },
    TagsCase {
        case: "a contact list created with several tags",
        kind: Kind::ContactList,
        tags: &[("zone", "eu"), ("team", "growth")],
    },
    TagsCase {
        case: "an email template created with no tags",
        kind: Kind::EmailTemplate,
        tags: &[],
    },
    TagsCase {
        case: "an email template created with one tag",
        kind: Kind::EmailTemplate,
        tags: &[("purpose", "receipts")],
    },
    TagsCase {
        case: "a configuration set created with no tags",
        kind: Kind::ConfigurationSet,
        tags: &[],
    },
    TagsCase {
        case: "a configuration set created with several tags",
        kind: Kind::ConfigurationSet,
        tags: &[("schedule", "nightly"), ("owner", "web")],
    },
];

/// The `Tags` member for the pairs a row supplies.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

#[tokio::test]
async fn a_resources_own_read_reports_its_tag_set_as_a_list_even_when_it_holds_no_tags() {
    for case in TAGS_CASES {
        let ses = Ses::new();
        let name = "tagged";
        let mut body = case.kind.create_body(name);
        if !case.tags.is_empty() {
            body.as_object_mut()
                .expect("the create body is an object")
                .insert("Tags".to_string(), tag_list(case.tags));
        }
        let created = ses
            .call("POST", case.kind.collection(), Some(body))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: seeding the resource must succeed, got {} {:?}",
            case.case, created.status, created.body
        );

        let read = ses
            .call("GET", &format!("{}/{name}", case.kind.collection()), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from the resource's own read, got {} {:?}",
            case.case, read.status, read.body
        );
        let expected = tag_list(case.tags);
        assert_eq!(
            read.member("Tags"),
            expected,
            "case {:?}: expected the read to report Tags {expected:?}, got {:?}",
            case.case,
            read.member("Tags")
        );
    }
}

/// One row of the duplicate-refusal table: the list the contact is created on, and the
/// address created twice.
struct DuplicateCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    list: &'static str,
    address: &'static str,
}

/// The rows span the list's name (so the refusal cannot be reading it) and the form of
/// the address (a plain one, one with a plus tag, one whose local part is mixed case).
const DUPLICATE_CASES: &[DuplicateCase] = &[
    DuplicateCase {
        case: "a plain address on a plain list",
        list: "newsletter",
        address: "reader@example.com",
    },
    DuplicateCase {
        case: "a tagged address on a list named with symbols",
        list: "news_letter-2",
        address: "reader+weekly@example.com",
    },
    DuplicateCase {
        case: "a mixed-case local part",
        list: "digest",
        address: "Reader.One@example.com",
    },
];

#[tokio::test]
async fn a_duplicate_contact_is_refused_with_the_address_and_no_list_name() {
    for case in DUPLICATE_CASES {
        let ses = Ses::new();
        let created_list = ses
            .create_list(json!({"ContactListName": case.list}))
            .await;
        assert_eq!(
            created_list.status, 200,
            "case {:?}: seeding the list must succeed, got {} {:?}",
            case.case, created_list.status, created_list.body
        );
        let created = ses
            .create_contact(case.list, json!({"EmailAddress": case.address}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: seeding the contact must succeed, got {} {:?}",
            case.case, created.status, created.body
        );

        let duplicate = ses
            .create_contact(case.list, json!({"EmailAddress": case.address}))
            .await;
        assert_eq!(
            (duplicate.status, duplicate.error_code().as_str()),
            (400, "AlreadyExistsException"),
            "case {:?}: expected 400 AlreadyExistsException, got {} {:?}",
            case.case,
            duplicate.status,
            duplicate.body
        );
        let expected = format!("{} already exists in List.", case.address);
        assert_eq!(
            duplicate.message(),
            expected,
            "case {:?}: expected the refusal {expected:?}, got {:?}",
            case.case,
            duplicate.message()
        );
        assert!(
            !duplicate.message().contains(case.list),
            "case {:?}: the refusal must not name the list, got {:?}",
            case.case,
            duplicate.message()
        );
    }
}

/// One row of the media-type table: a request, the status it is answered with, and the
/// media type that answer is announced under.
struct MediaTypeCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    status: u16,
    media_type: &'static str,
}

#[tokio::test]
async fn a_response_is_announced_in_the_framing_of_the_request_it_answers() {
    let ses = Ses::new();
    let created_list = ses
        .create_list(json!({"ContactListName": LIST, "Topics": topics(&[("news", "OPT_IN")])}))
        .await;
    assert_eq!(
        created_list.status, 200,
        "seeding the list must succeed, got {} {:?}",
        created_list.status, created_list.body
    );
    let created = ses
        .create_contact(LIST, json!({"EmailAddress": CONTACT}))
        .await;
    assert_eq!(
        created.status, 200,
        "seeding the contact must succeed, got {} {:?}",
        created.status, created.body
    );

    // The rows span both sides of the rule — a request that frames no body and one that
    // frames a JSON body — against what can produce a response: a success carrying a
    // body, a success whose output has no members, a modelled 404 and a modelled 400 a
    // handler raises, and a 400 the constraint walk raises at the boundary for a request
    // missing a required member.
    let cases: Vec<MediaTypeCase> = vec![
        MediaTypeCase {
            case: "a read answering a body",
            method: "GET",
            path: format!("/v2/email/contact-lists/{LIST}"),
            body: None,
            status: 200,
            media_type: ANNOUNCED_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "a listing answering a body",
            method: "GET",
            path: "/v2/email/contact-lists".to_string(),
            body: None,
            status: 200,
            media_type: ANNOUNCED_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "a read of a contact the list holds",
            method: "GET",
            path: format!("/v2/email/contact-lists/{LIST}/contacts/{CONTACT}"),
            body: None,
            status: 200,
            media_type: ANNOUNCED_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "a modelled 404 for a contact nothing was stored under",
            method: "GET",
            path: format!("/v2/email/contact-lists/{LIST}/contacts/nobody@example.com"),
            body: None,
            status: 404,
            media_type: ANNOUNCED_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "a listing that posts its paging members",
            method: "POST",
            path: format!("/v2/email/contact-lists/{LIST}/contacts/list"),
            body: Some(json!({})),
            status: 200,
            media_type: REQUEST_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "an update whose output has no members",
            method: "PUT",
            path: format!("/v2/email/contact-lists/{LIST}/contacts/{CONTACT}"),
            body: Some(json!({"UnsubscribeAll": true})),
            status: 200,
            media_type: REQUEST_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "a modelled 400 a handler raises",
            method: "POST",
            path: format!("/v2/email/contact-lists/{LIST}/contacts"),
            body: Some(json!({"EmailAddress": CONTACT})),
            status: 400,
            media_type: REQUEST_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "a 400 the constraint walk raises at the boundary",
            method: "POST",
            path: "/v2/email/contact-lists".to_string(),
            body: Some(json!({})),
            status: 400,
            media_type: REQUEST_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "a delete whose output has no members",
            method: "DELETE",
            path: format!("/v2/email/contact-lists/{LIST}/contacts/{CONTACT}"),
            body: None,
            status: 200,
            media_type: ANNOUNCED_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "a delete of the list itself",
            method: "DELETE",
            path: format!("/v2/email/contact-lists/{LIST}"),
            body: None,
            status: 200,
            media_type: ANNOUNCED_MEDIA_TYPE,
        },
        MediaTypeCase {
            case: "a modelled 404 for the list once it is gone",
            method: "DELETE",
            path: format!("/v2/email/contact-lists/{LIST}"),
            body: None,
            status: 404,
            media_type: ANNOUNCED_MEDIA_TYPE,
        },
    ];

    for case in &cases {
        let reply = ses.call(case.method, &case.path, case.body.clone()).await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: expected HTTP {}, got {} {:?}",
            case.case, case.status, reply.status, reply.body
        );
        assert_eq!(
            reply.content_type.as_deref(),
            Some(case.media_type),
            "case {:?}: expected content-type {:?}, got {:?}",
            case.case,
            case.media_type,
            reply.content_type
        );
    }
}
