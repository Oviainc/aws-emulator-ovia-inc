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

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

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

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
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

    async fn create_list(&self, body: Value) -> Reply {
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

    async fn list_lists(&self, query: &str) -> Reply {
        self.call("GET", &format!("/v2/email/contact-lists{query}"), None)
            .await
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

    async fn delete_contact(&self, list: &str, address: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/v2/email/contact-lists/{list}/contacts/{address}"),
            None,
        )
        .await
    }

    async fn list_contacts(&self, list: &str, body: Value) -> Reply {
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
fn listed_contacts(reply: &Reply) -> Vec<String> {
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

/// Percent-encode a `NextToken` for the query string.
///
/// `ListContactLists` binds `NextToken` to `@httpQuery`, and the token is base64, whose
/// alphabet includes `+`, `/` and `=` — characters a query string gives its own meaning
/// to, so the value is escaped the way an SDK escapes it.
fn query_escaped(token: &str) -> String {
    token
        .chars()
        .map(|character| match character {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => character.to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
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
    // `TopicDefaultPreferences` is "the default topic preferences applied to the
    // contact": one per topic the list publishes, carrying that topic's default.
    let defaults = read
        .body
        .get("TopicDefaultPreferences")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        defaults.len(),
        2,
        "case {:?}: the read must report one default per topic the list publishes, got {:?}",
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
