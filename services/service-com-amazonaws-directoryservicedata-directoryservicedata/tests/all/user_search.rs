// SPDX-License-Identifier: CC0-1.0

//! `SearchUsers` over HTTP: which users a search reports, and which it must not.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rule under test is the one the operation states: a directory's
//! user is reported when the value it holds for at least one of the attributes the
//! request NAMED is the search string, and nothing else about the user decides it. The
//! search string is matched whole, because the operation documents that wildcard
//! searches aren't supported; an attribute name and a string value are both matched
//! case insensitively, which is how a directory resolves them.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A `DirectoryId` satisfying `^d-[0-9a-f]{10}$`.
const DIRECTORY: &str = "d-1234567890";

/// A second one, for the cases that are about one directory not answering for another.
const OTHER_DIRECTORY: &str = "d-00abcdef99";

/// A third, for the paging case, so its users cannot perturb the table above.
const PAGED_DIRECTORY: &str = "d-00000000aa";

/// A `DirectoryId` nothing is ever written to.
const UNUSED_DIRECTORY: &str = "d-aaaaaaaaaa";

/// The largest page `MaxResults` admits (`@range(min: 1, max: 250)`), spelled out here
/// rather than imported so a wrong bound in the implementation fails.
const MAX_RESULTS_CEILING: i64 = 250;

/// A SigV4 credential scope naming this service: `ds-data` is its `sigv4Name`, and the
/// route key is what claims the request for this service.
const SIGNED_FOR_DS_DATA: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ds-data/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// A string member of the answer, or the empty string when it carries none.
    fn text(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// Every operation of this service is `POST <uri>` with a JSON body; `DirectoryId`
/// rides in the query string (`@httpQuery`).
async fn call(router: &MultiServiceRouter, uri: &str, directory: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("{uri}?DirectoryId={directory}"))
        .header("host", "ds-data.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_DS_DATA)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create_user(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Users/CreateUser", directory, body).await
}

async fn describe_user(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Users/DescribeUser", directory, body).await
}

async fn list_users(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Users/ListUsers", directory, body).await
}

async fn search_users(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, "/Users/SearchUsers", directory, body).await
}

/// One search request.
fn search(string: &str, attributes: &[&str]) -> Value {
    json!({"SearchString": string, "SearchAttributes": attributes})
}

/// The `SAMAccountName`s a search reported, in the order it reported them.
fn matched_names(answer: &Answer) -> Vec<String> {
    answer.body["Users"]
        .as_array()
        .map(|users| {
            users
                .iter()
                .map(|user| {
                    user["SAMAccountName"]
                        .as_str()
                        .unwrap_or_else(|| panic!("a reported user carries a name: {user:?}"))
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The users every case in the table searches: the same string sits on different
/// attributes of different users, so only the attribute a request names can decide
/// which of them it reports.
async fn populate(router: &MultiServiceRouter) {
    // Attribute names satisfy `LdapDisplayName`'s `^[A-Za-z*][A-Za-z-*]*$`, and the
    // value forms cover every arm of the modelled `AttributeValue` union
    // (`smithy_query(query="shape", shape="AttributeValue")`: S, N, BOOL, SS).
    let creates = [
        json!({
            "SAMAccountName": "alice",
            "GivenName": "Ada",
            "Surname": "Lovelace",
            "EmailAddress": "ada@corp.example.com",
            "OtherAttributes": {
                "Description": {"S": "Engineering"},
                "Floor": {"N": 7},
                "Contractor": {"BOOL": true},
                "Telephone": {"SS": ["555-0100", "555-0101"]},
            },
        }),
        json!({
            "SAMAccountName": "bob",
            "GivenName": "Ada",
            "Surname": "Turing",
            // The same value as alice's, in another case.
            "OtherAttributes": {"Description": {"S": "engineering"}},
        }),
        json!({
            "SAMAccountName": "carol",
            "GivenName": "Grace",
            "Surname": "Hopper",
            // "Ada" again, but on an attribute no other user holds.
            "OtherAttributes": {"Title": {"S": "Ada"}},
        }),
        // An account name that merely CONTAINS another case's search string.
        json!({"SAMAccountName": "ada.king", "GivenName": "Augusta", "Surname": "King"}),
    ];
    for body in creates {
        let created = create_user(router, DIRECTORY, &body).await;
        assert_eq!(
            created.status, 200,
            "case {body:?}: CreateUser must be accepted, got {} with {:?}",
            created.status, created.body
        );
    }

    // A user of another directory, which no search of this one may ever report.
    let outsider = create_user(
        router,
        OTHER_DIRECTORY,
        &json!({"SAMAccountName": "outsider", "GivenName": "Ada"}),
    )
    .await;
    assert_eq!(
        outsider.status, 200,
        "CreateUser in the other directory must be accepted, got {:?}",
        outsider.body
    );
}

/// RULE: a user matches when the value it holds for one of the NAMED attributes is the
/// search string — no other attribute of that user is consulted, and the string is
/// matched whole.
///
/// The rows differ along which attribute holds the value (a structural member under the
/// directory's attribute name and under the model's member name, each arm of the
/// `AttributeValue` union, an attribute only one user holds), the case of the name and
/// of the value, how many attributes are named, and whether the string sits on an
/// attribute the request named at all.
#[tokio::test]
async fn a_search_matches_only_the_attributes_the_request_named() {
    let router = emulator();
    populate(&router).await;

    let cases: [(&str, &str, &[&str], &[&str]); 18] = [
        (
            "a structural member, under the model's member name",
            "Ada",
            &["GivenName"],
            &["alice", "bob"],
        ),
        (
            "the same member, under the attribute a directory holds it in",
            "Lovelace",
            &["sn"],
            &["alice"],
        ),
        (
            "that member under the model's name for it",
            "Lovelace",
            &["Surname"],
            &["alice"],
        ),
        (
            "an attribute name in another case",
            "Lovelace",
            &["SURNAME"],
            &["alice"],
        ),
        (
            "a value in another case",
            "lovelace",
            &["Surname"],
            &["alice"],
        ),
        (
            "an email address, under the attribute a directory holds it in",
            "ada@corp.example.com",
            &["mail"],
            &["alice"],
        ),
        (
            "an email address, under the model's member name",
            "ada@corp.example.com",
            &["EmailAddress"],
            &["alice"],
        ),
        ("an account name", "alice", &["SAMAccountName"], &["alice"]),
        (
            "a stored string attribute, matched case insensitively",
            "ENGINEERING",
            &["Description"],
            &["alice", "bob"],
        ),
        ("a stored number attribute", "7", &["Floor"], &["alice"]),
        (
            "a stored boolean attribute",
            "true",
            &["Contractor"],
            &["alice"],
        ),
        (
            "one value of a multi-valued attribute",
            "555-0101",
            &["Telephone"],
            &["alice"],
        ),
        (
            "the same string, on the one attribute another user holds it in",
            "Ada",
            &["Title"],
            &["carol"],
        ),
        (
            "every named attribute contributes its matches",
            "Ada",
            &["GivenName", "Title"],
            &["alice", "bob", "carol"],
        ),
        (
            "a value held only under an attribute the request did not name",
            "Lovelace",
            &["GivenName"],
            &[],
        ),
        (
            "an attribute no user of this directory holds",
            "Ada",
            &["Department"],
            &[],
        ),
        (
            "a value no named attribute holds",
            "Babbage",
            &["Surname"],
            &[],
        ),
        (
            "a prefix of a value, which is not a match",
            "Lov",
            &["Surname"],
            &[],
        ),
    ];

    for (case, string, attributes, expected) in cases {
        let answer = search_users(&router, DIRECTORY, &search(string, attributes)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: SearchUsers must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        let matched = matched_names(&answer);
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            matched, expected,
            "case {case:?}: searching {string:?} over {attributes:?} must report \
             {expected:?} in account-name order, got {matched:?}"
        );
        assert_eq!(
            answer.text("DirectoryId"),
            DIRECTORY,
            "case {case:?}: the answer must name the directory it searched, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["NextToken"],
            Value::Null,
            "case {case:?}: a page with nothing left after it carries no NextToken, got {:?}",
            answer.body
        );
    }

    // A prefix of an account name is not a match either, even with a user whose name
    // begins with the string searched for.
    let prefix = search_users(&router, DIRECTORY, &search("ada", &["SAMAccountName"])).await;
    assert!(
        matched_names(&prefix).is_empty(),
        "an account name is matched whole, not as a prefix of 'ada.king', got {:?}",
        prefix.body
    );
}

/// RULE: the projection is the directory's, not the client's — a matched user comes
/// back as the record a listing reports for it, widened by the attributes the search
/// NAMED, and by nothing else.
///
/// The rows differ along which attributes the search names, hence which members widen
/// the listing's record: none, the email attribute under either of its names, the
/// object's own naming attributes, a stored attribute.
#[tokio::test]
async fn a_matched_user_carries_the_listing_record_widened_by_what_it_named() {
    let router = emulator();
    populate(&router).await;

    // The record alice's create stored, as `DescribeUser` reports it: every projected
    // value below must equal the one this read answers with.
    let stored = describe_user(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "alice", "OtherAttributes": ["Description"]}),
    )
    .await;

    // What `ListUsers` reports for the same object: the members every search result
    // carries too, whatever it named.
    let listed = list_users(&router, DIRECTORY, &json!({})).await;
    let summary = listed.body["Users"]
        .as_array()
        .expect("a Users list")
        .iter()
        .find(|user| user["SAMAccountName"] == json!("alice"))
        .expect("alice is listed")
        .clone();
    let summary_members = members(&summary);

    let cases: [(&str, &str, &[&str], &[&str]); 6] = [
        (
            "an attribute the listing already reports",
            "Ada",
            &["GivenName"],
            &[],
        ),
        (
            "the account name alone",
            "alice",
            &["SAMAccountName"],
            &[],
        ),
        (
            "the email attribute, under the attribute a directory holds it in",
            "ada@corp.example.com",
            &["mail"],
            &["EmailAddress"],
        ),
        (
            "the email attribute, under the model's member name",
            "ada@corp.example.com",
            &["EmailAddress"],
            &["EmailAddress"],
        ),
        (
            "the object's distinguished name and UPN",
            "Ada",
            &["GivenName", "distinguishedName", "userPrincipalName"],
            &["DistinguishedName", "UserPrincipalName"],
        ),
        (
            "a stored attribute",
            "Engineering",
            &["Description"],
            &["OtherAttributes"],
        ),
    ];

    for (case, string, attributes, widened) in cases {
        let answer = search_users(&router, DIRECTORY, &search(string, attributes)).await;
        let alice = answer.body["Users"]
            .as_array()
            .expect("a Users list")
            .iter()
            .find(|user| user["SAMAccountName"] == json!("alice"))
            .unwrap_or_else(|| panic!("case {case:?}: alice must match, got {:?}", answer.body))
            .clone();

        let mut expected = summary_members.clone();
        expected.extend(widened.iter().map(|member| (*member).to_string()));
        expected.sort();
        assert_eq!(
            members(&alice),
            expected,
            "case {case:?}: searching {attributes:?} must answer the listing's record \
             widened by {widened:?}, got {alice:?}"
        );

        // Every member it carries holds the value the record holds.
        for member in members(&alice) {
            assert_eq!(
                alice[&member], stored.body[&member],
                "case {case:?}: {member} must be the value the record holds, got \
                 {alice:?} against {:?}",
                stored.body
            );
        }
    }
}

/// The members an answered object carries, in a stable order.
fn members(user: &Value) -> Vec<String> {
    let mut named: Vec<String> = user
        .as_object()
        .expect("an object")
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(member, _)| member.clone())
        .collect();
    named.sort();
    named
}

/// The matching users are paged through in account-name order, whatever the page size,
/// and a token resumes the search it was issued for and no other.
#[tokio::test]
async fn a_search_pages_through_the_matching_users_in_order() {
    let router = emulator();

    // Seven users share the searched attribute value; three hold another value for the
    // same attribute, so a page that leaked a non-matching user would show up.
    let matching = ["zoe", "Alice", "bob", "carol.d", "dave-9", "Eve", "frank_7"];
    for name in matching {
        create_user(
            &router,
            PAGED_DIRECTORY,
            &json!({"SAMAccountName": name, "OtherAttributes": {"Description": {"S": "shared"}}}),
        )
        .await;
    }
    for name in ["ned", "olive", "pat"] {
        create_user(
            &router,
            PAGED_DIRECTORY,
            &json!({"SAMAccountName": name, "OtherAttributes": {"Description": {"S": "other"}}}),
        )
        .await;
    }

    let mut expected: Vec<String> = matching.iter().map(|name| (*name).to_string()).collect();
    expected.sort_by_key(|name| name.to_lowercase());

    for page_size in [1_i64, 2, 3, 6, 7, 8, MAX_RESULTS_CEILING] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = search("shared", &["Description"]);
            request["MaxResults"] = json!(page_size);
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let page = search_users(&router, PAGED_DIRECTORY, &request).await;
            assert_eq!(
                page.status, 200,
                "case maxResults={page_size}: SearchUsers must be accepted, got {} with {:?}",
                page.status, page.body
            );
            let names = matched_names(&page);
            assert!(
                i64::try_from(names.len()).expect("a page fits in i64") <= page_size,
                "case maxResults={page_size}: a page must hold at most {page_size} users, \
                 got {names:?}"
            );
            walked.extend(names);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case maxResults={page_size}: paging must terminate, walked {walked:?}"
            );
            token = page.body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case maxResults={page_size}: walking every page must report every matching \
             user once, in account-name order"
        );
    }

    // A token names a position in ONE search: another search string, another attribute
    // set, and another operation's listing cannot be resumed with it.
    let issued = {
        let mut request = search("shared", &["Description"]);
        request["MaxResults"] = json!(1);
        search_users(&router, PAGED_DIRECTORY, &request).await.body["NextToken"]
            .as_str()
            .expect("a first page of seven matches carries a token")
            .to_string()
    };
    let listing_token = list_users(&router, PAGED_DIRECTORY, &json!({"MaxResults": 1}))
        .await
        .body["NextToken"]
        .as_str()
        .expect("a first page of ten users carries a token")
        .to_string();

    let cases: [(&str, Value); 5] = [
        ("another search string", {
            let mut request = search("other", &["Description"]);
            request["NextToken"] = json!(issued.clone());
            request
        }),
        ("another attribute named", {
            let mut request = search("shared", &["Title"]);
            request["NextToken"] = json!(issued.clone());
            request
        }),
        ("one more attribute named", {
            let mut request = search("shared", &["Description", "Title"]);
            request["NextToken"] = json!(issued.clone());
            request
        }),
        ("a token from another operation's listing", {
            let mut request = search("shared", &["Description"]);
            request["NextToken"] = json!(listing_token);
            request
        }),
        ("a token with a digit changed", {
            let mut bytes = issued.clone().into_bytes();
            let last = bytes.len() - 1;
            bytes[last] = if bytes[last] == b'a' { b'b' } else { b'a' };
            let mut request = search("shared", &["Description"]);
            request["NextToken"] = json!(String::from_utf8(bytes).expect("hex stays utf-8"));
            request
        }),
    ];

    for (case, request) in cases {
        let refused = search_users(&router, PAGED_DIRECTORY, &request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token this search did not issue must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["Reason"],
            json!("INVALID_NEXT_TOKEN"),
            "case {case:?}: the refusal must say which rule it broke, got {:?}",
            refused.body
        );
    }

    // The token this search DID issue resumes it.
    let resumed = {
        let mut request = search("shared", &["Description"]);
        request["NextToken"] = json!(issued);
        search_users(&router, PAGED_DIRECTORY, &request).await
    };
    assert_eq!(
        matched_names(&resumed),
        expected[1..].to_vec(),
        "the issued token must resume the search just after its first page, got {:?}",
        resumed.body
    );
}

/// A search reads ONE directory and ONE realm, and reading never materialises a
/// directory.
#[tokio::test]
async fn a_search_reads_one_directory_and_one_realm() {
    let router = emulator();
    populate(&router).await;

    let own_realm = describe_user(&router, DIRECTORY, &json!({"SAMAccountName": "alice"}))
        .await
        .text("Realm");
    assert!(
        !own_realm.is_empty(),
        "the directory must report a domain name"
    );

    // The directory's own realm, in any case, searches the directory's objects.
    for spelling in [own_realm.clone(), own_realm.to_uppercase()] {
        let mut request = search("Ada", &["GivenName"]);
        request["Realm"] = json!(spelling);
        let answer = search_users(&router, DIRECTORY, &request).await;
        assert_eq!(
            matched_names(&answer),
            vec!["alice".to_string(), "bob".to_string()],
            "case {spelling:?}: the directory's own realm is case insensitive, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.text("Realm"),
            spelling,
            "case {spelling:?}: the answer must report the realm it read, got {:?}",
            answer.body
        );
    }

    // Another realm is a domain this directory holds no users in.
    let other_realm = "trusted.example.net";
    let mut request = search("Ada", &["GivenName"]);
    request["Realm"] = json!(other_realm);
    let foreign = search_users(&router, DIRECTORY, &request).await;
    assert_eq!(
        foreign.status, 200,
        "searching another realm must be accepted, got {} with {:?}",
        foreign.status, foreign.body
    );
    assert!(
        matched_names(&foreign).is_empty(),
        "another realm holds no users here, got {:?}",
        foreign.body
    );
    assert_eq!(
        foreign.text("Realm"),
        other_realm,
        "the answer must report the realm it read, got {:?}",
        foreign.body
    );

    // Each directory answers only for its own users.
    let other = search_users(&router, OTHER_DIRECTORY, &search("Ada", &["GivenName"])).await;
    assert_eq!(
        matched_names(&other),
        vec!["outsider".to_string()],
        "a directory must report only its own matching users, got {:?}",
        other.body
    );

    // A directory nothing was written to holds nothing, and searching it does not
    // create it.
    let unused = search_users(&router, UNUSED_DIRECTORY, &search("Ada", &["GivenName"])).await;
    assert_eq!(
        unused.status, 200,
        "searching an unused directory must be accepted, got {} with {:?}",
        unused.status, unused.body
    );
    assert!(
        matched_names(&unused).is_empty(),
        "an unused directory holds no users, got {:?}",
        unused.body
    );
    let listed = list_users(&router, UNUSED_DIRECTORY, &json!({})).await;
    assert!(
        listed.body["Users"]
            .as_array()
            .is_none_or(|users| users.is_empty()),
        "a search must not materialise the directory it read, got {:?}",
        listed.body
    );
}
