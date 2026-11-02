// SPDX-License-Identifier: CC0-1.0

//! The contact-list and contact surface, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised.
//!
//! The rule these cases pin: a paginated listing answers PROMPTLY with a page of at most
//! `PageSize` items and a `NextToken` only while items remain behind that page, so a walk
//! that follows the token terminates and visits every item exactly once; and a call this
//! service cannot serve is refused with its own modelled, terminal error (a 4xx naming the
//! resource), never with a status a client would retry. The tables below span what the
//! rule generalises over: the number of stored items (none, one, several), the page size
//! (absent, one, a size that divides the count, a size larger than it), both listings
//! (`ListContactLists` and `ListContacts`), and the refusal legs.
//!
//! The second rule these cases pin: an update that replaces a resource's metadata stores
//! EXACTLY the members the request supplies — a member it omits is cleared, not kept —
//! while leaving everything the request does not bind alone, so the next read reports the
//! supplied metadata and nothing of the metadata it replaced. The replacement table below
//! spans what that rule generalises over: a member removed from a list of them, every
//! member removed (by omitting the list and by supplying an empty one), the same members
//! in another order, one added, the whole set replaced, and each scalar supplied and
//! omitted. Each row also pins the rule's consequence for a sub-resource that refers to
//! the replaced members: a contact's stated topic preference names a topic OF its list, so
//! it is reported, and resolves to a status, only while the list still publishes that
//! topic.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
pub(crate) struct Reply {
    pub(crate) status: u16,
    error_type: Option<String>,
    pub(crate) body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    pub(crate) fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    pub(crate) fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A string member of the body, or `""` when absent.
    pub(crate) fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router.
pub(crate) struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses` signing
            // name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
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

    pub(crate) async fn create_list(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/contact-lists", Some(body)).await
    }

    async fn get_list(&self, name: &str) -> Reply {
        self.call("GET", &format!("/v2/email/contact-lists/{name}"), None)
            .await
    }

    async fn delete_list(&self, name: &str) -> Reply {
        self.call("DELETE", &format!("/v2/email/contact-lists/{name}"), None)
            .await
    }

    async fn update_list(&self, name: &str, body: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/contact-lists/{name}"),
            Some(body),
        )
        .await
    }

    async fn list_lists(&self, query: &str) -> Reply {
        self.call("GET", &format!("/v2/email/contact-lists{query}"), None)
            .await
    }

    pub(crate) async fn create_contact(&self, list: &str, body: Value) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/contact-lists/{list}/contacts"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn get_contact(&self, list: &str, address: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/contact-lists/{list}/contacts/{address}"),
            None,
        )
        .await
    }

    async fn delete_contact(&self, list: &str, address: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/v2/email/contact-lists/{list}/contacts/{address}"),
            None,
        )
        .await
    }

    pub(crate) async fn list_contacts(&self, list: &str, body: Value) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/contact-lists/{list}/contacts/list"),
            Some(body),
        )
        .await
    }
}

/// The names a `ListContactLists` page reports, in the order it reported them.
fn listed_lists(reply: &Reply) -> Vec<String> {
    reply
        .body
        .get("ContactLists")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.get("ContactListName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The addresses a `ListContacts` page reports, in the order it reported them.
pub(crate) fn listed_contacts(reply: &Reply) -> Vec<String> {
    reply
        .body
        .get("Contacts")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.get("EmailAddress")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `NextToken` a page handed back, if any.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .body
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Percent-encode a caller-supplied value for the query string.
///
/// `ListContactLists` binds `NextToken` to `@httpQuery`, and the token is base64, whose
/// alphabet includes `+`, `/` and `=` — characters a query string gives its own meaning
/// to, so the value is escaped the way an SDK escapes it. Shared with the other suites
/// of this test binary that walk an `@httpQuery` token, put a filter in a query string
/// or name an `@httpQuery` tag key, which the model's `Tag` documentation allows to be
/// written in "Unicode letters" — so everything outside the unreserved set is escaped
/// byte by byte over the value's UTF-8 encoding, and a value in any script survives the
/// round trip.
pub(crate) fn query_escaped(token: &str) -> String {
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

/// One row of the pagination table: how many items are stored and the `PageSize` the
/// walk asks for.
struct PageCase {
    case: &'static str,
    stored: usize,
    page_size: Option<usize>,
}

/// The pagination rule spans the item count and the page size independently, so the rows
/// cross an empty, a single and a multi-item store with an absent page size, a page size
/// of one, one that divides the count and one larger than it.
const PAGE_CASES: &[PageCase] = &[
    PageCase {
        case: "nothing stored, no page size",
        stored: 0,
        page_size: None,
    },
    PageCase {
        case: "nothing stored, page size 1",
        stored: 0,
        page_size: Some(1),
    },
    PageCase {
        case: "one item, page size 1",
        stored: 1,
        page_size: Some(1),
    },
    PageCase {
        case: "one item, no page size",
        stored: 1,
        page_size: None,
    },
    PageCase {
        case: "two items, page size 1",
        stored: 2,
        page_size: Some(1),
    },
    PageCase {
        case: "ten items, page size 1",
        stored: 10,
        page_size: Some(1),
    },
    PageCase {
        case: "ten items, page size 5",
        stored: 10,
        page_size: Some(5),
    },
    PageCase {
        case: "ten items, page size 4",
        stored: 10,
        page_size: Some(4),
    },
    PageCase {
        case: "ten items, page size 100",
        stored: 10,
        page_size: Some(100),
    },
    PageCase {
        case: "ten items, no page size",
        stored: 10,
        page_size: None,
    },
];

/// A walk may not outlive its items: one request per item plus one for the final page is
/// the most any page size can need, and a request past that means the token loop does not
/// terminate.
fn walk_budget(stored: usize) -> usize {
    stored + 2
}

#[tokio::test]
async fn a_contact_list_round_trips_through_its_own_read() {
    let ses = Ses::new();
    let created = ses
        .create_list(json!({
            "ContactListName": "newsletter",
            "Description": "Everything we publish",
            "Topics": [
                {
                    "TopicName": "weekly",
                    "DisplayName": "Weekly digest",
                    "Description": "One mail a week",
                    "DefaultSubscriptionStatus": "OPT_IN"
                },
                {
                    "TopicName": "offers",
                    "DisplayName": "Special offers",
                    "DefaultSubscriptionStatus": "OPT_OUT"
                }
            ],
            "Tags": [{"Key": "team", "Value": "growth"}]
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateContactList, got {} {:?}",
        "newsletter", created.status, created.body
    );

    let read = ses.get_list("newsletter").await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected HTTP 200 from GetContactList, got {} {:?}",
        "newsletter", read.status, read.body
    );
    assert_eq!(
        read.text("ContactListName"),
        "newsletter",
        "case {:?}: the read must report the stored name, got {:?}",
        "newsletter",
        read.body
    );
    assert_eq!(
        read.text("Description"),
        "Everything we publish",
        "case {:?}: the read must report the stored description, got {:?}",
        "newsletter",
        read.body
    );
    let topics = read
        .body
        .get("Topics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        topics.len(),
        2,
        "case {:?}: the read must report both stored topics, got {:?}",
        "newsletter",
        read.body
    );
    assert_eq!(
        topics[0].get("TopicName").and_then(Value::as_str),
        Some("weekly"),
        "case {:?}: the topics must keep the order they were supplied in, got {:?}",
        "newsletter",
        read.body
    );
    assert_eq!(
        read.body
            .get("Tags")
            .and_then(Value::as_array)
            .and_then(|tags| tags.first())
            .and_then(|tag| tag.get("Value"))
            .and_then(Value::as_str),
        Some("growth"),
        "case {:?}: the read must report the stored tag, got {:?}",
        "newsletter",
        read.body
    );

    // One list per name: the second create of the same name is refused, terminally.
    let duplicate = ses
        .create_list(json!({"ContactListName": "newsletter"}))
        .await;
    assert_eq!(
        (duplicate.status, duplicate.error_code().as_str()),
        (400, "AlreadyExistsException"),
        "case {:?}: expected 400 AlreadyExistsException for a name already stored, got {} {:?}",
        "newsletter",
        duplicate.status,
        duplicate.body
    );
    assert_eq!(
        duplicate.message(),
        "List with name: newsletter already exists.",
        "case {:?}: unexpected refusal wording, got {:?}",
        "newsletter",
        duplicate.body
    );

    // And a name nothing was stored under is a terminal 404 naming it.
    for unknown in ["missing", "Newsletter", "news-letter"] {
        let read = ses.get_list(unknown).await;
        assert_eq!(
            (read.status, read.error_code().as_str()),
            (404, "NotFoundException"),
            "case {unknown:?}: expected 404 NotFoundException for a list nothing stored, \
             got {} {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.message(),
            format!("List with name: {unknown} doesn't exist."),
            "case {unknown:?}: unexpected refusal wording, got {:?}",
            read.body
        );
    }

    let deleted = ses.delete_list("newsletter").await;
    assert_eq!(
        deleted.status, 200,
        "case {:?}: expected HTTP 200 from DeleteContactList, got {} {:?}",
        "newsletter", deleted.status, deleted.body
    );
    let gone = ses.get_list("newsletter").await;
    assert_eq!(
        (gone.status, gone.error_code().as_str()),
        (404, "NotFoundException"),
        "case {:?}: a deleted list must read back as 404 NotFoundException, got {} {:?}",
        "newsletter",
        gone.status,
        gone.body
    );
}

#[tokio::test]
async fn the_contact_list_paginator_walks_every_list_exactly_once() {
    for case in PAGE_CASES {
        let ses = Ses::new();
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for index in 0..case.stored {
            let name = format!("list-{index:02}");
            let created = ses
                .create_list(json!({"ContactListName": name.clone()}))
                .await;
            assert_eq!(
                created.status, 200,
                "case {:?}: expected HTTP 200 creating {name:?}, got {} {:?}",
                case.case, created.status, created.body
            );
            expected.insert(name);
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut requests = 0_usize;
        loop {
            requests += 1;
            assert!(
                requests <= walk_budget(case.stored),
                "case {:?}: the walk must terminate within {} requests, it did not",
                case.case,
                walk_budget(case.stored)
            );
            let mut query = Vec::new();
            if let Some(size) = case.page_size {
                query.push(format!("PageSize={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("NextToken={}", query_escaped(token)));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };
            let page = ses.list_lists(&query).await;
            assert_eq!(
                page.status, 200,
                "case {:?}: expected HTTP 200 from ListContactLists, got {} {:?}",
                case.case, page.status, page.body
            );
            let names = listed_lists(&page);
            if let Some(size) = case.page_size {
                assert!(
                    names.len() <= size,
                    "case {:?}: a page may hold at most PageSize {size} lists, got {} ({:?})",
                    case.case,
                    names.len(),
                    names
                );
            }
            seen.extend(names);
            token = next_token(&page);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen.len(),
            expected.len(),
            "case {:?}: the walk must visit every list exactly once, got {seen:?}",
            case.case
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {:?}: the walk must visit exactly the stored lists, got {seen:?}",
            case.case
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {:?}: a walk reports the lists in one stable order, got {seen:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn the_contact_paginator_walks_every_contact_exactly_once() {
    for case in PAGE_CASES {
        let ses = Ses::new();
        let list = "walked";
        let created = ses.create_list(json!({"ContactListName": list})).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 creating the list, got {} {:?}",
            case.case, created.status, created.body
        );
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for index in 0..case.stored {
            let address = format!("contact-{index:02}@example.com");
            let created = ses
                .create_contact(list, json!({"EmailAddress": address.clone()}))
                .await;
            assert_eq!(
                created.status, 200,
                "case {:?}: expected HTTP 200 creating {address:?}, got {} {:?}",
                case.case, created.status, created.body
            );
            expected.insert(address);
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut requests = 0_usize;
        loop {
            requests += 1;
            assert!(
                requests <= walk_budget(case.stored),
                "case {:?}: the walk must terminate within {} requests, it did not",
                case.case,
                walk_budget(case.stored)
            );
            let mut body = serde_json::Map::new();
            if let Some(size) = case.page_size {
                body.insert("PageSize".to_string(), json!(size));
            }
            if let Some(token) = &token {
                body.insert("NextToken".to_string(), json!(token));
            }
            let page = ses.list_contacts(list, Value::Object(body)).await;
            assert_eq!(
                page.status, 200,
                "case {:?}: expected HTTP 200 from ListContacts, got {} {:?}",
                case.case, page.status, page.body
            );
            let addresses = listed_contacts(&page);
            if let Some(size) = case.page_size {
                assert!(
                    addresses.len() <= size,
                    "case {:?}: a page may hold at most PageSize {size} contacts, got {} ({:?})",
                    case.case,
                    addresses.len(),
                    addresses
                );
            }
            seen.extend(addresses);
            token = next_token(&page);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen.len(),
            expected.len(),
            "case {:?}: the walk must visit every contact exactly once, got {seen:?}",
            case.case
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {:?}: the walk must visit exactly the stored contacts, got {seen:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_contact_round_trips_through_its_own_read() {
    let ses = Ses::new();
    let list = "newsletter";
    ses.create_list(json!({
        "ContactListName": list,
        "Topics": [
            {
                "TopicName": "weekly",
                "DisplayName": "Weekly digest",
                "DefaultSubscriptionStatus": "OPT_IN"
            },
            {
                "TopicName": "offers",
                "DisplayName": "Special offers",
                "DefaultSubscriptionStatus": "OPT_OUT"
            }
        ]
    }))
    .await;

    let created = ses
        .create_contact(
            list,
            json!({
                "EmailAddress": "reader@example.com",
                "TopicPreferences": [{"TopicName": "offers", "SubscriptionStatus": "OPT_IN"}],
                "AttributesData": "{\"city\":\"Dublin\"}"
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateContact, got {} {:?}",
        "reader@example.com", created.status, created.body
    );

    let read = ses.get_contact(list, "reader@example.com").await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected HTTP 200 from GetContact, got {} {:?}",
        "reader@example.com", read.status, read.body
    );
    assert_eq!(
        read.text("EmailAddress"),
        "reader@example.com",
        "case {:?}: the read must report the stored address, got {:?}",
        "reader@example.com",
        read.body
    );
    assert_eq!(
        read.text("AttributesData"),
        "{\"city\":\"Dublin\"}",
        "case {:?}: the read must report the stored attribute data, got {:?}",
        "reader@example.com",
        read.body
    );
    assert_eq!(
        read.body
            .get("TopicPreferences")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "case {:?}: the read must report the one stated preference, got {:?}",
        "reader@example.com",
        read.body
    );
    // `TopicDefaultPreferences` is "the default topic preferences APPLIED to the
    // contact": the default of each topic the list publishes that the contact has stated
    // no preference for. The contact stated one for `offers`, so only `weekly` applies.
    let defaults = read
        .body
        .get("TopicDefaultPreferences")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        defaults.len(),
        1,
        "case {:?}: the read must report a default only for the topic the contact stated \
         nothing for, got {:?}",
        "reader@example.com",
        read.body
    );
    assert_eq!(
        defaults[0].get("TopicName").and_then(Value::as_str),
        Some("weekly"),
        "case {:?}: the default must be the one of the unstated topic, got {:?}",
        "reader@example.com",
        read.body
    );
    assert_eq!(
        defaults[0].get("SubscriptionStatus").and_then(Value::as_str),
        Some("OPT_IN"),
        "case {:?}: a topic's default must be the status the list declared, got {:?}",
        "reader@example.com",
        read.body
    );

    // A preference can only name a topic the list publishes.
    let unknown_topic = ses
        .create_contact(
            list,
            json!({
                "EmailAddress": "other@example.com",
                "TopicPreferences": [{"TopicName": "monthly", "SubscriptionStatus": "OPT_IN"}]
            }),
        )
        .await;
    assert_eq!(
        (unknown_topic.status, unknown_topic.error_code().as_str()),
        (400, "BadRequestException"),
        "case {:?}: expected 400 BadRequestException for a topic the list does not publish, \
         got {} {:?}",
        "monthly",
        unknown_topic.status,
        unknown_topic.body
    );

    // One contact per address on a list.
    let duplicate = ses
        .create_contact(list, json!({"EmailAddress": "reader@example.com"}))
        .await;
    assert_eq!(
        (duplicate.status, duplicate.error_code().as_str()),
        (400, "AlreadyExistsException"),
        "case {:?}: expected 400 AlreadyExistsException for an address already on the list, \
         got {} {:?}",
        "reader@example.com",
        duplicate.status,
        duplicate.body
    );

    // An address nothing was stored under, and a list nothing was stored under, are each
    // a terminal 404 naming what is missing.
    let unknown_contact = ses.get_contact(list, "nobody@example.com").await;
    assert_eq!(
        (unknown_contact.status, unknown_contact.error_code().as_str()),
        (404, "NotFoundException"),
        "case {:?}: expected 404 NotFoundException for a contact nothing stored, got {} {:?}",
        "nobody@example.com",
        unknown_contact.status,
        unknown_contact.body
    );
    let unknown_list = ses.list_contacts("missing", json!({})).await;
    assert_eq!(
        (unknown_list.status, unknown_list.error_code().as_str()),
        (404, "NotFoundException"),
        "case {:?}: expected 404 NotFoundException listing a list nothing stored, got {} {:?}",
        "missing",
        unknown_list.status,
        unknown_list.body
    );
    assert_eq!(
        unknown_list.message(),
        "List with name: missing doesn't exist.",
        "case {:?}: unexpected refusal wording, got {:?}",
        "missing",
        unknown_list.body
    );

    let deleted = ses.delete_contact(list, "reader@example.com").await;
    assert_eq!(
        deleted.status, 200,
        "case {:?}: expected HTTP 200 from DeleteContact, got {} {:?}",
        "reader@example.com", deleted.status, deleted.body
    );
    let gone = ses.get_contact(list, "reader@example.com").await;
    assert_eq!(
        (gone.status, gone.error_code().as_str()),
        (404, "NotFoundException"),
        "case {:?}: a deleted contact must read back as 404 NotFoundException, got {} {:?}",
        "reader@example.com",
        gone.status,
        gone.body
    );

    // "Deletes a contact list and all of the contacts on that list": the contacts go with
    // the list, so the listing is refused for the list itself.
    ses.create_contact(list, json!({"EmailAddress": "reader@example.com"}))
        .await;
    ses.delete_list(list).await;
    let after = ses.list_contacts(list, json!({})).await;
    assert_eq!(
        (after.status, after.error_code().as_str()),
        (404, "NotFoundException"),
        "case {:?}: listing a deleted list must be 404 NotFoundException, got {} {:?}",
        list,
        after.status,
        after.body
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_hand_out_is_refused() {
    let ses = Ses::new();
    let list = "tokens";
    ses.create_list(json!({"ContactListName": list})).await;
    ses.create_contact(list, json!({"EmailAddress": "a@example.com"}))
        .await;
    ses.create_contact(list, json!({"EmailAddress": "b@example.com"}))
        .await;

    // The token the contact-list walk hands out, so one listing's token is tried against
    // the other's as well as against made-up values.
    let lists_token = next_token(&ses.list_lists("?PageSize=1").await);
    let contacts_token = next_token(&ses.list_contacts(list, json!({"PageSize": 1})).await);

    // (case, the token to try)
    let forged: Vec<(&str, String)> = vec![
        ("not a token at all", "not-a-real-pagination-token".to_string()),
        ("valid base64 of foreign text", "Zm9yZ2VkLXRva2Vu".to_string()),
        ("an empty token", String::new()),
        (
            "a token from the other listing",
            contacts_token.clone().unwrap_or_default(),
        ),
    ];
    for (case, token) in &forged {
        let reply = ses
            .list_lists(&format!("?NextToken={}", query_escaped(token)))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: ListContactLists must refuse a token it did not hand out with \
             400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    let forged_for_contacts: Vec<(&str, String)> = vec![
        ("not a token at all", "not-a-real-pagination-token".to_string()),
        ("an empty token", String::new()),
        (
            "a token from the other listing",
            lists_token.unwrap_or_default(),
        ),
    ];
    for (case, token) in &forged_for_contacts {
        let reply = ses
            .list_contacts(list, json!({"NextToken": token}))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: ListContacts must refuse a token it did not hand out with 400 \
             BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // The token this service DID hand out still resumes the walk.
    let resumed = ses
        .list_contacts(list, json!({"PageSize": 1, "NextToken": contacts_token}))
        .await;
    assert_eq!(
        resumed.status, 200,
        "case {:?}: a token this service handed out must resume the walk, got {} {:?}",
        "the service's own token", resumed.status, resumed.body
    );
    assert_eq!(
        listed_contacts(&resumed),
        vec!["b@example.com".to_string()],
        "case {:?}: the resumed page must start strictly after the first page, got {:?}",
        "the service's own token",
        resumed.body
    );
}

/// The display name a topic carries when the UPDATE supplies it, which differs from the
/// one the create supplied so a read-back proves each topic's own members were replaced
/// too, not only the set of topic names.
fn updated_display_name(topic: &str) -> String {
    format!("{topic} display, as updated")
}

/// One `Topic` as a request body spells it. `Topic` requires `TopicName`, `DisplayName`
/// and `DefaultSubscriptionStatus`; `SubscriptionStatus` is `OPT_IN` or `OPT_OUT`.
fn updated_topic(name: &str, default_status: &str) -> Value {
    json!({
        "TopicName": name,
        "DisplayName": updated_display_name(name),
        "DefaultSubscriptionStatus": default_status
    })
}

/// The topics a `GetContactList` reply reports, in order, as
/// (`TopicName`, `DisplayName`, `DefaultSubscriptionStatus`).
fn read_topics(reply: &Reply) -> Vec<(String, String, String)> {
    reply
        .body
        .get("Topics")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let member = |name: &str| {
                        row.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (
                        member("TopicName"),
                        member("DisplayName"),
                        member("DefaultSubscriptionStatus"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The topic names a reply's `member` list of topic preferences names, in order.
fn preference_topics(reply: &Reply, member: &str) -> Vec<String> {
    reply
        .body
        .get(member)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.get("TopicName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A timestamp member as epoch seconds, for an ordering assertion.
fn timestamp_seconds(reply: &Reply, member: &str) -> f64 {
    reply
        .body
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_default()
}

/// One row of the metadata-replacement table: the update's body. The expectation IS the
/// body — a complete replacement stores exactly what the request supplied — so each row
/// is read as both.
struct ReplacementCase {
    case: &'static str,
    /// The `Topics` the update supplies as (name, default status), or `None` when the
    /// update omits the member entirely.
    topics: Option<&'static [(&'static str, &'static str)]>,
    /// The `Description` the update supplies, or `None` when it omits the member.
    description: Option<&'static str>,
}

/// Every row runs against the same starting list — topics `weekly`, `offers` and `alerts`
/// and a description — so each row's own difference is what the assertions attribute the
/// result to.
const REPLACEMENT_CASES: &[ReplacementCase] = &[
    ReplacementCase {
        case: "one of three topics removed",
        topics: Some(&[("weekly", "OPT_IN"), ("alerts", "OPT_OUT")]),
        description: Some("still publishing"),
    },
    ReplacementCase {
        case: "every topic removed by omitting Topics",
        topics: None,
        description: Some("no topics any more"),
    },
    ReplacementCase {
        case: "every topic removed by supplying an empty Topics list",
        topics: Some(&[]),
        description: Some("an empty topic list"),
    },
    ReplacementCase {
        case: "the same three topics in another order",
        topics: Some(&[
            ("alerts", "OPT_IN"),
            ("offers", "OPT_IN"),
            ("weekly", "OPT_OUT"),
        ]),
        description: Some("reordered"),
    },
    ReplacementCase {
        case: "a fourth topic added to the three",
        topics: Some(&[
            ("weekly", "OPT_OUT"),
            ("offers", "OPT_IN"),
            ("alerts", "OPT_IN"),
            ("monthly", "OPT_OUT"),
        ]),
        description: Some("one more topic"),
    },
    ReplacementCase {
        case: "the whole set replaced by a topic the list never published",
        topics: Some(&[("monthly", "OPT_IN")]),
        description: Some("replaced wholesale"),
    },
    ReplacementCase {
        case: "the description omitted while the topics are kept",
        topics: Some(&[
            ("weekly", "OPT_IN"),
            ("offers", "OPT_OUT"),
            ("alerts", "OPT_IN"),
        ]),
        description: None,
    },
    ReplacementCase {
        case: "both members omitted",
        topics: None,
        description: None,
    },
];

#[tokio::test]
async fn updating_a_contact_list_replaces_its_metadata_completely() {
    for case in REPLACEMENT_CASES {
        let ses = Ses::new();
        let list = "newsletter";
        let created = ses
            .create_list(json!({
                "ContactListName": list,
                "Description": "Everything we publish",
                "Topics": [
                    {
                        "TopicName": "weekly",
                        "DisplayName": "Weekly digest, as created",
                        "DefaultSubscriptionStatus": "OPT_IN"
                    },
                    {
                        "TopicName": "offers",
                        "DisplayName": "Special offers, as created",
                        "DefaultSubscriptionStatus": "OPT_OUT"
                    },
                    {
                        "TopicName": "alerts",
                        "DisplayName": "Service alerts, as created",
                        "DefaultSubscriptionStatus": "OPT_IN"
                    }
                ],
                "Tags": [{"Key": "team", "Value": "growth"}]
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 from CreateContactList, got {} {:?}",
            case.case, created.status, created.body
        );
        let contact = ses
            .create_contact(
                list,
                json!({
                    "EmailAddress": "reader@example.com",
                    "TopicPreferences": [{"TopicName": "offers", "SubscriptionStatus": "OPT_IN"}]
                }),
            )
            .await;
        assert_eq!(
            contact.status, 200,
            "case {:?}: expected HTTP 200 from CreateContact, got {} {:?}",
            case.case, contact.status, contact.body
        );
        let before = ses.get_list(list).await;
        let created_timestamp = before.body.get("CreatedTimestamp").cloned();

        let mut body = serde_json::Map::new();
        if let Some(topics) = case.topics {
            body.insert(
                "Topics".to_string(),
                Value::Array(
                    topics
                        .iter()
                        .map(|(name, status)| updated_topic(name, status))
                        .collect(),
                ),
            );
        }
        if let Some(description) = case.description {
            body.insert("Description".to_string(), json!(description));
        }
        let updated = ses.update_list(list, Value::Object(body)).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected HTTP 200 from UpdateContactList, got {} {:?}",
            case.case, updated.status, updated.body
        );

        let read = ses.get_list(list).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 reading the updated list, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            read.text("ContactListName"),
            list,
            "case {:?}: the update must not rename the list, got {:?}",
            case.case,
            read.body
        );
        let expected_topics: Vec<(String, String, String)> = case
            .topics
            .unwrap_or(&[])
            .iter()
            .map(|(name, status)| {
                (
                    (*name).to_string(),
                    updated_display_name(name),
                    (*status).to_string(),
                )
            })
            .collect();
        assert_eq!(
            read_topics(&read),
            expected_topics,
            "case {:?}: the read must report exactly the supplied topics, in order, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.body.get("Description").and_then(Value::as_str),
            case.description,
            "case {:?}: the read must report exactly the supplied description, got {:?}",
            case.case,
            read.body
        );
        // What the update's input does not bind is untouched: the tags and the creation
        // time, and the contacts on the list are still on it.
        assert_eq!(
            read.body
                .get("Tags")
                .and_then(Value::as_array)
                .and_then(|tags| tags.first())
                .and_then(|tag| tag.get("Value"))
                .and_then(Value::as_str),
            Some("growth"),
            "case {:?}: the update must keep the list's tags, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.body.get("CreatedTimestamp").cloned(),
            created_timestamp,
            "case {:?}: the update must not move the creation time, got {:?}",
            case.case,
            read.body
        );
        assert!(
            timestamp_seconds(&read, "LastUpdatedTimestamp")
                >= timestamp_seconds(&read, "CreatedTimestamp"),
            "case {:?}: the last-updated time may not precede the creation time, got {:?}",
            case.case,
            read.body
        );
        let read_contact = ses.get_contact(list, "reader@example.com").await;
        assert_eq!(
            read_contact.status, 200,
            "case {:?}: the contact must survive the metadata replacement, got {} {:?}",
            case.case, read_contact.status, read_contact.body
        );
        // `TopicDefaultPreferences` is computed from the list's topics the contact has
        // stated no preference for, so the replacement shows through on the contact as
        // well — without the topic whose preference the contact stated (`offers`, while
        // the list still publishes it).
        assert_eq!(
            preference_topics(&read_contact, "TopicDefaultPreferences"),
            expected_topics
                .iter()
                .map(|(name, _, _)| name.clone())
                .filter(|name| name != "offers")
                .collect::<Vec<String>>(),
            "case {:?}: a contact's defaults must follow the list's topics it stated \
             nothing for, got {:?}",
            case.case,
            read_contact.body
        );
        // The contact stated one preference, for `offers`. A stated preference names a
        // topic OF the list, so it is reported only while the list still publishes that
        // topic: once the replacement drops the topic, the preference names nothing.
        let offers_published = case
            .topics
            .unwrap_or(&[])
            .iter()
            .any(|(name, _)| *name == "offers");
        let expected_preferences: Vec<String> = if offers_published {
            vec!["offers".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            preference_topics(&read_contact, "TopicPreferences"),
            expected_preferences,
            "case {:?}: GetContact must report a stated preference only while the list \
             publishes its topic, got {:?}",
            case.case,
            read_contact.body
        );
        let listed = ses.list_contacts(list, json!({})).await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected HTTP 200 from ListContacts, got {} {:?}",
            case.case, listed.status, listed.body
        );
        let listed_row = listed
            .body
            .get("Contacts")
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            listed_row
                .get("TopicPreferences")
                .and_then(Value::as_array)
                .map(|rows| rows
                    .iter()
                    .filter_map(|row| row.get("TopicName").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<String>>())
                .unwrap_or_default(),
            expected_preferences,
            "case {:?}: ListContacts must report the same stated preferences as the \
             contact's own read, got {listed_row:?}",
            case.case
        );
        // And a filter on that topic selects the contact only while the topic is there:
        // nothing is subscribed to a topic the list does not publish.
        let filtered = ses
            .list_contacts(
                list,
                json!({
                    "Filter": {
                        "FilteredStatus": "OPT_IN",
                        "TopicFilter": {"TopicName": "offers"}
                    }
                }),
            )
            .await;
        assert_eq!(
            filtered.status, 200,
            "case {:?}: expected HTTP 200 from a filtered ListContacts, got {} {:?}",
            case.case, filtered.status, filtered.body
        );
        assert_eq!(
            listed_contacts(&filtered),
            if offers_published {
                vec!["reader@example.com".to_string()]
            } else {
                Vec::new()
            },
            "case {:?}: a topic filter may select a contact only while the list publishes \
             the topic, got {:?}",
            case.case,
            filtered.body
        );
    }
}

#[tokio::test]
async fn a_preference_dropped_with_its_topic_does_not_come_back_with_the_topic() {
    let ses = Ses::new();
    let list = "newsletter";
    // (case, the topic set the update supplies)
    let weekly = updated_topic("weekly", "OPT_IN");
    let offers = updated_topic("offers", "OPT_OUT");
    ses.create_list(json!({
        "ContactListName": list,
        "Topics": [weekly.clone(), offers.clone()]
    }))
    .await;
    let created = ses
        .create_contact(
            list,
            json!({
                "EmailAddress": "reader@example.com",
                "TopicPreferences": [
                    {"TopicName": "weekly", "SubscriptionStatus": "OPT_OUT"},
                    {"TopicName": "offers", "SubscriptionStatus": "OPT_IN"}
                ]
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from CreateContact, got {} {:?}",
        "both topics stated", created.status, created.body
    );

    // Remove `weekly`: the preference stated for it goes with the topic.
    let removed = ses
        .update_list(list, json!({"Topics": [offers.clone()]}))
        .await;
    assert_eq!(
        removed.status, 200,
        "case {:?}: expected HTTP 200 from UpdateContactList, got {} {:?}",
        "weekly removed", removed.status, removed.body
    );
    let after_removal = ses.get_contact(list, "reader@example.com").await;
    assert_eq!(
        preference_topics(&after_removal, "TopicPreferences"),
        vec!["offers".to_string()],
        "case {:?}: the preference for the removed topic must be gone, got {:?}",
        "weekly removed",
        after_removal.body
    );

    // Declare `weekly` again: nothing kept the dropped preference, so it does not come
    // back — the contact now has only the list's default for that topic.
    let readded = ses
        .update_list(list, json!({"Topics": [offers, weekly]}))
        .await;
    assert_eq!(
        readded.status, 200,
        "case {:?}: expected HTTP 200 from UpdateContactList, got {} {:?}",
        "weekly re-added", readded.status, readded.body
    );
    let after_readd = ses.get_contact(list, "reader@example.com").await;
    assert_eq!(
        preference_topics(&after_readd, "TopicPreferences"),
        vec!["offers".to_string()],
        "case {:?}: a dropped preference may not return with its topic, got {:?}",
        "weekly re-added",
        after_readd.body
    );
    assert_eq!(
        preference_topics(&after_readd, "TopicDefaultPreferences"),
        vec!["weekly".to_string()],
        "case {:?}: the re-added topic must contribute its default, and the topic the \
         contact stated a preference for must not, got {:?}",
        "weekly re-added",
        after_readd.body
    );
}

#[tokio::test]
async fn a_topic_filter_naming_a_topic_the_list_does_not_publish_selects_nothing() {
    let ses = Ses::new();
    let list = "newsletter";
    ses.create_list(json!({
        "ContactListName": list,
        "Topics": [updated_topic("offers", "OPT_OUT")]
    }))
    .await;
    // A contact unsubscribed from everything resolves OPT_OUT for every topic the list
    // publishes — and for no topic it does not.
    ses.create_contact(
        list,
        json!({"EmailAddress": "reader@example.com", "UnsubscribeAll": true}),
    )
    .await;

    // (case, the topic the filter names, whether the list publishes it)
    for (case, topic_name, published) in [
        ("a topic the list publishes", "offers", true),
        ("a topic the list never published", "monthly", false),
        ("a topic the list's own name is not", "newsletter", false),
    ] {
        let filtered = ses
            .list_contacts(
                list,
                json!({
                    "Filter": {
                        "FilteredStatus": "OPT_OUT",
                        "TopicFilter": {"TopicName": topic_name}
                    }
                }),
            )
            .await;
        assert_eq!(
            filtered.status, 200,
            "case {case:?}: expected HTTP 200 from a filtered ListContacts, got {} {:?}",
            filtered.status, filtered.body
        );
        assert_eq!(
            listed_contacts(&filtered),
            if published {
                vec!["reader@example.com".to_string()]
            } else {
                Vec::new()
            },
            "case {case:?}: a filter may select a contact only for a topic the list \
             publishes, got {:?}",
            filtered.body
        );
    }
}

#[tokio::test]
async fn updating_a_contact_list_nothing_was_stored_under_is_refused() {
    let ses = Ses::new();
    let list = "newsletter";
    ses.create_list(json!({
        "ContactListName": list,
        "Description": "Everything we publish"
    }))
    .await;

    // A name nothing was stored under — including the stored name spelled differently,
    // since the name is compared exactly — is a terminal 404 naming it.
    for unknown in ["missing", "Newsletter", "news-letter"] {
        let reply = ses
            .update_list(unknown, json!({"Description": "written by a refused call"}))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {unknown:?}: expected 404 NotFoundException updating a list nothing \
             stored, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!("List with name: {unknown} doesn't exist."),
            "case {unknown:?}: unexpected refusal wording, got {:?}",
            reply.body
        );
    }

    // A refused update stores nothing: no list is created for the name it named, and the
    // stored list still reads back as it was created.
    assert_eq!(
        listed_lists(&ses.list_lists("").await),
        vec![list.to_string()],
        "case {:?}: a refused update may not create a list",
        "after the refusals"
    );
    let read = ses.get_list(list).await;
    assert_eq!(
        read.text("Description"),
        "Everything we publish",
        "case {:?}: a refused update may not touch another list, got {:?}",
        "after the refusals",
        read.body
    );
}
