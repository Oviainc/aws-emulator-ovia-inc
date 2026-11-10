// SPDX-License-Identifier: CC0-1.0

//! The Directory Service Data user, group and membership lifecycle, end to end over
//! HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rule under test is the one this data plane states for a
//! directory's objects: an object is whatever the create stored, every operation
//! computes its answer from that record, and a delete removes the record and
//! everything that pointed at it.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A `DirectoryId` satisfying `^d-[0-9a-f]{10}$`.
const DIRECTORY: &str = "d-1234567890";

/// A second one, for the cases that are about one directory not answering for another.
const OTHER_DIRECTORY: &str = "d-00abcdef99";

/// The largest page `MaxResults` admits (`@range(min: 1, max: 250)`), spelled out here
/// rather than imported so a wrong bound in the implementation fails.
const MAX_RESULTS_CEILING: i64 = 250;

/// The lowest RID Active Directory hands to an object it creates: everything below
/// 1000 is reserved for the built-in principals.
const FIRST_RID: u64 = 1000;

/// `GroupType`'s modelled values, from
/// `smithy_query(query="shape", shape="GroupType")` — "Distribution", "Security".
const GROUP_TYPES: [&str; 2] = ["Distribution", "Security"];

/// `GroupScope`'s modelled values, from
/// `smithy_query(query="shape", shape="GroupScope")` — "DomainLocal", "Global",
/// "Universal", "BuiltinLocal".
const GROUP_SCOPES: [&str; 4] = ["DomainLocal", "Global", "Universal", "BuiltinLocal"];

/// A SigV4 credential scope naming this service: `ds-data` is its `sigv4Name`, and the
/// route key is what claims the request for this service.
const SIGNED_FOR_DS_DATA: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ds-data/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// A `DirectoryId` the account holds that nothing is ever written to, so what it reports
/// is "this directory holds no objects" and not the authorization gate's refusal.
const UNUSED_DIRECTORY: &str = "d-aaaaaaaaaa";

/// One emulator: a router carrying only this service, over one shared engine, with the
/// account holding every directory these cases address — every operation of this data
/// plane is gated on that.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    super::validation_envelope::hold_directories(
        &engine,
        &[DIRECTORY, OTHER_DIRECTORY, UNUSED_DIRECTORY],
    );
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
async fn call(router: &MultiServiceRouter, uri: &str, body: &Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "ds-data.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_DS_DATA)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
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
    call(
        router,
        &format!("/Users/CreateUser?DirectoryId={directory}"),
        body,
    )
    .await
}

async fn describe_user(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(
        router,
        &format!("/Users/DescribeUser?DirectoryId={directory}"),
        body,
    )
    .await
}

async fn list_users(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(
        router,
        &format!("/Users/ListUsers?DirectoryId={directory}"),
        body,
    )
    .await
}

async fn delete_user(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(
        router,
        &format!("/Users/DeleteUser?DirectoryId={directory}"),
        body,
    )
    .await
}

async fn create_group(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(
        router,
        &format!("/Groups/CreateGroup?DirectoryId={directory}"),
        body,
    )
    .await
}

async fn add_group_member(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(
        router,
        &format!("/GroupMemberships/AddGroupMember?DirectoryId={directory}"),
        body,
    )
    .await
}

async fn list_group_members(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(
        router,
        &format!("/GroupMemberships/ListGroupMembers?DirectoryId={directory}"),
        body,
    )
    .await
}

/// The distinguished name a created object of this suite's directories carries:
/// `CN=<name>,OU=Users,OU=<NetBIOS name>,<DC= components of the realm>`.
///
/// Built from the realm the SAME answer reports, so what is pinned is the derivation
/// rule — a managed directory names an object the account created inside the
/// organizational unit it delegates to the account — and not whichever domain name the
/// emulator's directories carry. These directories were registered without a NetBIOS
/// short name, so the unit goes by the default a create that names none gets: the first
/// label of the domain name, in upper case.
fn expected_distinguished_name(realm: &str, common_name: &str) -> String {
    let labels: Vec<&str> = realm.trim_end_matches('.').split('.').collect();
    let components = labels
        .iter()
        .map(|label| format!("DC={label}"))
        .collect::<Vec<_>>()
        .join(",");
    let delegated_ou = labels.first().unwrap_or(&"").to_uppercase();
    format!("CN={common_name},OU=Users,OU={delegated_ou},{components}")
}

/// A realm is a domain name: `Realm`'s
/// `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$` in the form this test needs it — at
/// least two labels of letters, digits and separators.
fn looks_like_a_realm(value: &str) -> bool {
    let labels: Vec<&str> = value.trim_end_matches('.').split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// The domain SID and the RID of a Windows SID `S-1-5-21-a-b-c-rid`, or `None` when
/// `value` is not of that form.
///
/// `S-1-5` is the NT authority and `21` the sub-authority every domain SID carries;
/// the three values after it identify the domain and the last is the object's RID.
fn split_sid(value: &str) -> Option<(String, u64)> {
    let rest = value.strip_prefix("S-1-5-21-")?;
    let parts: Vec<&str> = rest.split('-').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut numbers = Vec::new();
    for part in &parts {
        numbers.push(part.parse::<u64>().ok()?);
    }
    if numbers[..3].iter().any(|value| *value > u64::from(u32::MAX)) {
        return None;
    }
    let domain = format!("S-1-5-21-{}-{}-{}", numbers[0], numbers[1], numbers[2]);
    Some((domain, numbers[3]))
}

/// The `SAMAccountName`s of a `ListUsers` page, in the order it reported them.
fn listed_user_names(answer: &Answer) -> Vec<String> {
    answer.body["Users"]
        .as_array()
        .unwrap_or_else(|| panic!("ListUsers must report a Users list, got {:?}", answer.body))
        .iter()
        .map(|user| {
            user["SAMAccountName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `SAMAccountName`s of a `ListGroupMembers` page, in the order it reported them.
fn listed_member_names(answer: &Answer) -> Vec<String> {
    answer.body["Members"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListGroupMembers must report a Members list, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|member| {
            member["SAMAccountName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One user case: what the create carried, keyed by what makes it different from the
/// others. The rule is the same for every row — the describe answers with what the
/// create stored — so the rows vary the members, their forms and their types.
fn user_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required members",
            json!({"SAMAccountName": "jdoe"}),
        ),
        (
            "every optional name member",
            json!({
                "SAMAccountName": "asmith",
                "EmailAddress": "asmith@corp.example.com",
                "GivenName": "Ada",
                "Surname": "Smith",
            }),
        ),
        (
            "one-character name, the shortest UserName admits",
            json!({"SAMAccountName": "a", "GivenName": "A"}),
        ),
        (
            "twenty-character name, the longest UserName admits",
            json!({"SAMAccountName": "aaaaaaaaaabbbbbbbbbb"}),
        ),
        (
            "a name carrying every character class the pattern admits",
            json!({"SAMAccountName": "a_b-c.d9"}),
        ),
        (
            "a mixed-case name",
            json!({"SAMAccountName": "MixedCase", "Surname": "Case"}),
        ),
        (
            // The `S` member is the one this API takes a value as, so a row covering
            // several attributes varies their NAMES and values, not the union member;
            // the other three members are refused, which `directory_authorization`
            // covers. The names are written as the directory's schema holds them, which
            // is the spelling a read reports (`object_naming` is the suite that pins how
            // another spelling of one is resolved).
            "several attributes, each sent as the AttributeValue S member",
            json!({
                "SAMAccountName": "attrs",
                "OtherAttributes": {
                    "Description": {"S": "the whole attribute catalogue"},
                    "CodePage": {"S": "437"},
                    "ServicePrincipalName": {"S": "host/a.corp.example.com"},
                },
            }),
        ),
        (
            "a single attribute, the fewest Attributes admits",
            json!({
                "SAMAccountName": "oneattr",
                "OtherAttributes": {"TelephoneNumber": {"S": "+1-555-0100"}},
            }),
        ),
    ]
}

/// `CreateUser` persists a user and `DescribeUser` answers from that record: every
/// member of the describe is the value the create stored, the SID is minted once and
/// kept, and only the attributes the describe asks for come back.
#[tokio::test]
async fn a_created_user_is_described_from_its_stored_record() {
    let router = emulator();
    let mut sids: BTreeMap<String, String> = BTreeMap::new();
    let mut domains: BTreeMap<String, String> = BTreeMap::new();

    for (case, request) in user_cases() {
        let name = request["SAMAccountName"].as_str().expect("a name").to_string();

        let created = create_user(&router, DIRECTORY, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateUser must be accepted, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("DirectoryId"),
            DIRECTORY,
            "case {case:?}: CreateUser must report the directory it wrote to, got {:?}",
            created.body
        );
        assert_eq!(
            created.text("SAMAccountName"),
            name,
            "case {case:?}: CreateUser must report the name it stored, got {:?}",
            created.body
        );

        let sid = created.text("SID");
        let (domain, rid) = split_sid(&sid).unwrap_or_else(|| {
            panic!("case {case:?}: SID {sid:?} is not of the form S-1-5-21-a-b-c-rid")
        });
        assert!(
            rid >= FIRST_RID,
            "case {case:?}: SID {sid:?} must carry a RID at or above {FIRST_RID}, got {rid}"
        );
        // Every object of one directory extends that directory's domain SID.
        domains.insert(name.clone(), domain);
        assert!(
            sids.values().all(|held| *held != sid),
            "case {case:?}: SID {sid:?} was already handed to another object: {sids:?}"
        );
        sids.insert(name.clone(), sid.clone());

        // Ask for every attribute the create carried, so the projection has something
        // to answer with.
        let wanted: Vec<String> = request["OtherAttributes"]
            .as_object()
            .map(|attributes| attributes.keys().cloned().collect())
            .unwrap_or_default();
        let mut describe_request = json!({"SAMAccountName": name});
        if !wanted.is_empty() {
            describe_request["OtherAttributes"] = json!(wanted);
        }
        let described = describe_user(&router, DIRECTORY, &describe_request).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeUser must find the user the create stored, got {} with {:?}",
            described.status, described.body
        );

        let realm = described.text("Realm");
        assert!(
            looks_like_a_realm(&realm),
            "case {case:?}: DescribeUser must report the directory's domain, got {realm:?}"
        );
        assert_eq!(
            described.text("SID"),
            sid,
            "case {case:?}: DescribeUser must report the SID the create minted, got {:?}",
            described.body
        );
        assert_eq!(
            described.text("SAMAccountName"),
            name,
            "case {case:?}: DescribeUser must report the stored name, got {:?}",
            described.body
        );
        assert_eq!(
            described.text("DistinguishedName"),
            expected_distinguished_name(&realm, &name),
            "case {case:?}: the distinguished name must name the object inside the realm \
             the same answer reports, got {:?}",
            described.body
        );
        assert_eq!(
            described.text("UserPrincipalName"),
            format!("{name}@{}", realm.to_uppercase()),
            "case {case:?}: the user principal name must be the account name in that realm, \
             got {:?}",
            described.body
        );
        // `CreateUser` models no password member, so a created account has no credential
        // and Active Directory leaves it INACTIVE: the control plane's
        // `ResetUserPassword` is what gives it one and so what makes it active.
        assert_eq!(
            described.body["Enabled"],
            json!(false),
            "case {case:?}: a created account has no password, so it must be inactive, \
             got {:?}",
            described.body
        );
        for member in ["EmailAddress", "GivenName", "Surname"] {
            assert_eq!(
                described.body[member],
                request
                    .get(member)
                    .cloned()
                    .unwrap_or(Value::Null),
                "case {case:?}: {member} must be what the create stored, got {:?}",
                described.body
            );
        }
        assert_eq!(
            described.body["OtherAttributes"],
            request
                .get("OtherAttributes")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: the requested attributes must be the stored ones, got {:?}",
            described.body
        );
    }

    // Every object of this directory carries the same domain SID: the RID is what
    // distinguishes them.
    let distinct: std::collections::BTreeSet<&String> = domains.values().collect();
    assert_eq!(
        distinct.len(),
        1,
        "one directory is one domain, got {domains:?}"
    );

    // The store answers for the names it holds, not for the request: a name nothing
    // created is a 404 even though its spelling is legal.
    let missing = describe_user(&router, DIRECTORY, &json!({"SAMAccountName": "nobody"})).await;
    assert_eq!(
        missing.status, 404,
        "a name nothing created must not be described, got {} with {:?}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {:?}",
        missing.body
    );

    // And the records belong to the directory they were written to.
    let elsewhere = describe_user(&router, OTHER_DIRECTORY, &json!({"SAMAccountName": "jdoe"})).await;
    assert_eq!(
        elsewhere.status, 404,
        "a user of one directory must not be described in another, got {} with {:?}",
        elsewhere.status, elsewhere.body
    );
}

/// An account name is case insensitive, so a describe finds the user whatever case it
/// spells the name in — and answers with the spelling the create stored, never with
/// the one the request carried.
#[tokio::test]
async fn an_account_name_resolves_case_insensitively() {
    let router = emulator();
    let stored = "MixedCase.User";
    create_user(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": stored, "GivenName": "Stored"}),
    )
    .await;

    for spelling in [
        "MixedCase.User",
        "mixedcase.user",
        "MIXEDCASE.USER",
        "mIxEdCaSe.UsEr",
    ] {
        let described = describe_user(&router, DIRECTORY, &json!({"SAMAccountName": spelling})).await;
        assert_eq!(
            described.status, 200,
            "case {spelling:?}: an account name resolves case insensitively, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.text("SAMAccountName"),
            stored,
            "case {spelling:?}: the answer must carry the stored spelling, not the requested \
             one, got {:?}",
            described.body
        );
        assert_eq!(
            described.body["GivenName"],
            json!("Stored"),
            "case {spelling:?}: the answer must be read from the stored record, got {:?}",
            described.body
        );
    }

    // One name, one object: a second create of any spelling of it conflicts.
    for spelling in ["MixedCase.User", "mixedcase.user"] {
        let again = create_user(&router, DIRECTORY, &json!({"SAMAccountName": spelling})).await;
        assert_eq!(
            again.status, 409,
            "case {spelling:?}: a second create of the same name must conflict, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {spelling:?}: expected ConflictException, got {:?}",
            again.body
        );
    }
    assert_eq!(
        listed_user_names(&list_users(&router, DIRECTORY, &json!({})).await).len(),
        1,
        "a refused create must leave exactly the one stored user"
    );
}

/// `DescribeUser` projects only the attributes the request asks for, matching the
/// names case insensitively, and reports no `OtherAttributes` at all when it asks for
/// none or for nothing the user holds.
#[tokio::test]
async fn describe_user_projects_only_the_requested_attributes() {
    let router = emulator();
    create_user(
        &router,
        DIRECTORY,
        &json!({
            "SAMAccountName": "projected",
            "OtherAttributes": {
                "Description": {"S": "a described object"},
                "CodePage": {"S": "437"},
                "Co": {"S": "Antarctica"},
            },
        }),
    )
    .await;

    let cases = [
        ("no attributes asked for", json!(null), json!(null)),
        (
            "one of the three",
            json!(["Description"]),
            json!({"Description": {"S": "a described object"}}),
        ),
        (
            "another one, asked for in a different case",
            json!(["CODEPAGE"]),
            json!({"CodePage": {"S": "437"}}),
        ),
        (
            "two of the three",
            json!(["co", "description"]),
            json!({"Co": {"S": "Antarctica"}, "Description": {"S": "a described object"}}),
        ),
        (
            "one held and one not",
            json!(["co", "wWWHomePage"]),
            json!({"Co": {"S": "Antarctica"}}),
        ),
        (
            "only attributes the user does not hold",
            json!(["wWWHomePage"]),
            json!(null),
        ),
    ];

    for (case, asked, expected) in cases {
        let mut request = json!({"SAMAccountName": "projected"});
        if !asked.is_null() {
            request["OtherAttributes"] = asked;
        }
        let described = describe_user(&router, DIRECTORY, &request).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeUser must be accepted, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.body["OtherAttributes"], expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            described.body
        );
    }
}

/// `ListUsers` reports the stored users of one directory in account-name order, serves
/// no more than `MaxResults` of them, and emits a `NextToken` only while more remain;
/// walking the pages enumerates every user exactly once whatever the page size.
#[tokio::test]
async fn list_users_pages_through_the_stored_users_in_order() {
    let router = emulator();
    // Names deliberately out of creation order, with mixed case, so the reported order
    // can only come from the stored keys.
    let created = [
        "zoe", "Alice", "bob", "carol.d", "dave-9", "Eve", "frank_7",
    ];
    for name in created {
        let answer = create_user(&router, DIRECTORY, &json!({"SAMAccountName": name})).await;
        assert_eq!(
            answer.status, 200,
            "CreateUser {name:?} must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }
    // A user of another directory must never appear in this one's listing.
    create_user(&router, OTHER_DIRECTORY, &json!({"SAMAccountName": "outsider"})).await;

    let mut expected: Vec<String> = created.iter().map(|name| (*name).to_string()).collect();
    expected.sort_by_key(|name| name.to_lowercase());

    for page_size in [1_i64, 2, 3, 6, 7, 8, MAX_RESULTS_CEILING] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let page = list_users(&router, DIRECTORY, &request).await;
            assert_eq!(
                page.status, 200,
                "case maxResults={page_size}: ListUsers must be accepted, got {} with {:?}",
                page.status, page.body
            );
            let names = listed_user_names(&page);
            assert!(
                i64::try_from(names.len()).expect("a page fits in i64") <= page_size,
                "case maxResults={page_size}: a page must hold at most {page_size} users, got {names:?}"
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
            "case maxResults={page_size}: walking every page must enumerate every stored user \
             once, in account-name order"
        );
    }

    // A single page holding everything carries no token.
    let whole = list_users(&router, DIRECTORY, &json!({})).await;
    assert_eq!(
        listed_user_names(&whole),
        expected,
        "ListUsers with no MaxResults must report every stored user"
    );
    assert_eq!(
        whole.body["NextToken"],
        Value::Null,
        "a page with nothing left after it must carry no NextToken, got {:?}",
        whole.body
    );

    // A directory the account holds that nothing was written to holds nothing, and is
    // not created by reading.
    let empty = list_users(&router, UNUSED_DIRECTORY, &json!({})).await;
    assert_eq!(
        empty.status, 200,
        "ListUsers on an unused directory must be accepted, got {} with {:?}",
        empty.status, empty.body
    );
    assert!(
        listed_user_names(&empty).is_empty(),
        "an unused directory holds no users, got {:?}",
        empty.body
    );

    // Every user reported carries the SID and the state its own record holds.
    for user in whole.body["Users"].as_array().expect("a Users list") {
        let name = user["SAMAccountName"].as_str().expect("a name");
        let described = describe_user(&router, DIRECTORY, &json!({"SAMAccountName": name})).await;
        assert_eq!(
            user["SID"], described.body["SID"],
            "user {name:?}: the summary must carry the SID the record holds, got {user:?}"
        );
        assert_eq!(
            user["Enabled"],
            described.body["Enabled"],
            "user {name:?}: the summary must report the state the describe reports, got {user:?}"
        );
        // `CreateUser` carries no password, so the account has no credential and is
        // INACTIVE until a password reset on the control plane gives it one.
        assert_eq!(
            user["Enabled"],
            json!(false),
            "user {name:?}: a created account has no password, so it must be reported \
             inactive, got {user:?}"
        );
    }
}

/// A `NextToken` is a cursor this service issued for THIS listing: anything else is a
/// `ValidationException`, not an empty page and not a page of another listing.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let router = emulator();
    for name in ["one", "two", "three"] {
        create_user(&router, DIRECTORY, &json!({"SAMAccountName": name})).await;
        create_user(&router, OTHER_DIRECTORY, &json!({"SAMAccountName": name})).await;
    }

    let issued = list_users(&router, DIRECTORY, &json!({"MaxResults": 1}))
        .await
        .body["NextToken"]
        .as_str()
        .expect("a first page of three users carries a token")
        .to_string();

    let cases = [
        ("not a token at all", "not-a-token".to_string()),
        ("empty-looking hex", "00".to_string()),
        ("a truncated token", issued[..issued.len() - 2].to_string()),
        ("a token with a digit changed", {
            let mut bytes = issued.clone().into_bytes();
            let last = bytes.len() - 1;
            bytes[last] = if bytes[last] == b'a' { b'b' } else { b'a' };
            String::from_utf8(bytes).expect("hex stays utf-8")
        }),
    ];

    for (case, token) in cases {
        let refused = list_users(&router, DIRECTORY, &json!({"NextToken": token})).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token this listing did not issue must be refused, got {} with {:?}",
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

    // The token is bound to the listing it came from: another directory's walk cannot
    // be resumed with it.
    let crossed = list_users(&router, OTHER_DIRECTORY, &json!({"NextToken": issued.clone()})).await;
    assert_eq!(
        crossed.status, 400,
        "a token issued for one directory must not resume another's listing, got {} with {:?}",
        crossed.status, crossed.body
    );

    // And a group listing cannot be resumed with a user listing's token.
    create_group(&router, DIRECTORY, &json!({"SAMAccountName": "a-group"})).await;
    let wrong_kind = list_group_members(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "a-group", "NextToken": issued}),
    )
    .await;
    assert_eq!(
        wrong_kind.status, 400,
        "a ListUsers token must not resume a ListGroupMembers walk, got {} with {:?}",
        wrong_kind.status, wrong_kind.body
    );
}

/// `CreateGroup` stores a group with a minted SID, `AddGroupMember` records a
/// membership on it, and `ListGroupMembers` computes its answer from that set: each
/// member's SID is the one its own record holds, and its `MemberType` is what the
/// member IS.
#[tokio::test]
async fn group_membership_is_computed_from_the_stored_set() {
    let router = emulator();

    // Every (GroupType, GroupScope) pair the model admits is a group that can be
    // created and added to.
    let mut groups = Vec::new();
    for (index, group_type) in GROUP_TYPES.iter().enumerate() {
        for (scope_index, group_scope) in GROUP_SCOPES.iter().enumerate() {
            let name = format!("group-{index}-{scope_index}");
            let created = create_group(
                &router,
                DIRECTORY,
                &json!({
                    "SAMAccountName": name,
                    "GroupType": group_type,
                    "GroupScope": group_scope,
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {group_type}/{group_scope}: CreateGroup must be accepted, got {} with {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.text("SAMAccountName"),
                name,
                "case {group_type}/{group_scope}: CreateGroup must report the stored name, \
                 got {:?}",
                created.body
            );
            let sid = created.text("SID");
            let (_, rid) = split_sid(&sid).unwrap_or_else(|| {
                panic!("case {group_type}/{group_scope}: SID {sid:?} is not a domain SID")
            });
            assert!(
                rid >= FIRST_RID,
                "case {group_type}/{group_scope}: SID {sid:?} must carry a RID at or above \
                 {FIRST_RID}"
            );
            groups.push((name, sid));
        }
    }

    // Users and a nested group, so a membership of each member kind is exercised.
    let members: [(&str, &str); 4] = [
        ("jdoe", "USER"),
        ("asmith", "USER"),
        ("Zoe", "USER"),
        ("nested-group", "GROUP"),
    ];
    let mut member_sids: BTreeMap<&str, String> = BTreeMap::new();
    for (name, kind) in members {
        let created = if kind == "USER" {
            create_user(&router, DIRECTORY, &json!({"SAMAccountName": name})).await
        } else {
            create_group(&router, DIRECTORY, &json!({"SAMAccountName": name})).await
        };
        member_sids.insert(name, created.text("SID"));
    }

    let (group, group_sid) = groups.first().cloned().expect("a group was created");
    for (name, _) in members {
        let added = add_group_member(
            &router,
            DIRECTORY,
            &json!({"GroupName": group, "MemberName": name}),
        )
        .await;
        assert_eq!(
            added.status, 200,
            "member {name:?}: AddGroupMember must be accepted, got {} with {:?}",
            added.status, added.body
        );
    }

    let listed = list_group_members(&router, DIRECTORY, &json!({"SAMAccountName": group})).await;
    assert_eq!(
        listed.status, 200,
        "ListGroupMembers must be accepted, got {} with {:?}",
        listed.status, listed.body
    );
    let mut expected: Vec<String> = members
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    expected.sort_by_key(|name| name.to_lowercase());
    assert_eq!(
        listed_member_names(&listed),
        expected,
        "every member added must be reported, in member-name order, got {:?}",
        listed.body
    );
    for member in listed.body["Members"].as_array().expect("a Members list") {
        let name = member["SAMAccountName"].as_str().expect("a name");
        let (_, kind) = members
            .iter()
            .find(|(member_name, _)| *member_name == name)
            .expect("a member this test added");
        assert_eq!(
            member["SID"],
            json!(member_sids[name]),
            "member {name:?}: the SID must be the one the member's own record holds, \
             got {member:?}"
        );
        assert_eq!(
            member["MemberType"],
            json!(kind),
            "member {name:?}: MemberType must say what the member is, got {member:?}"
        );
    }
    assert_ne!(
        group_sid,
        member_sids["nested-group"],
        "two groups must not share a SID"
    );

    // A membership is a set: adding a member the group already holds conflicts,
    // whatever case the name is spelled in, and the group is unchanged.
    for spelling in ["jdoe", "JDOE"] {
        let again = add_group_member(
            &router,
            DIRECTORY,
            &json!({"GroupName": group, "MemberName": spelling}),
        )
        .await;
        assert_eq!(
            again.status, 409,
            "case {spelling:?}: adding a member the group already holds must conflict, \
             got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {spelling:?}: expected ConflictException, got {:?}",
            again.body
        );
    }
    assert_eq!(
        listed_member_names(&list_group_members(&router, DIRECTORY, &json!({"SAMAccountName": group})).await),
        expected,
        "a refused add must leave the membership set as it was"
    );

    // Both objects must exist: the group that is added to, and the member added.
    let unknown_cases = [
        (
            "no such group",
            json!({"GroupName": "nope", "MemberName": "jdoe"}),
        ),
        (
            "no such member",
            json!({"GroupName": group.clone(), "MemberName": "nobody"}),
        ),
    ];
    for (case, request) in unknown_cases {
        let refused = add_group_member(&router, DIRECTORY, &request).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.body
        );
    }

    // A group of one directory is not a group of another.
    let elsewhere = list_group_members(&router, OTHER_DIRECTORY, &json!({"SAMAccountName": group})).await;
    assert_eq!(
        elsewhere.status, 404,
        "a group of one directory must not be listed in another, got {} with {:?}",
        elsewhere.status, elsewhere.body
    );

    // A group nobody was added to reports an empty set, not a 404.
    let (lonely, _) = groups.last().cloned().expect("a second group");
    let none = list_group_members(&router, DIRECTORY, &json!({"SAMAccountName": lonely})).await;
    assert_eq!(
        none.status, 200,
        "an empty group must still be listed, got {} with {:?}",
        none.status, none.body
    );
    assert!(
        listed_member_names(&none).is_empty(),
        "a group nobody was added to holds no members, got {:?}",
        none.body
    );
}

/// `ListGroupMembers` honours `MaxResults` as a hard page size and resumes exactly
/// after the member the previous page ended on.
#[tokio::test]
async fn list_group_members_pages_through_the_stored_set() {
    let router = emulator();
    create_group(&router, DIRECTORY, &json!({"SAMAccountName": "engineers"})).await;
    let members = ["zoe", "Alice", "bob", "carol", "dave"];
    for name in members {
        create_user(&router, DIRECTORY, &json!({"SAMAccountName": name})).await;
        add_group_member(
            &router,
            DIRECTORY,
            &json!({"GroupName": "engineers", "MemberName": name}),
        )
        .await;
    }
    let mut expected: Vec<String> = members.iter().map(|name| (*name).to_string()).collect();
    expected.sort_by_key(|name| name.to_lowercase());

    for page_size in [1_i64, 2, 4, 5, MAX_RESULTS_CEILING] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"SAMAccountName": "engineers", "MaxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let page = list_group_members(&router, DIRECTORY, &request).await;
            assert_eq!(
                page.status, 200,
                "case maxResults={page_size}: ListGroupMembers must be accepted, got {} with {:?}",
                page.status, page.body
            );
            let names = listed_member_names(&page);
            assert!(
                i64::try_from(names.len()).expect("a page fits in i64") <= page_size,
                "case maxResults={page_size}: a page must hold at most {page_size} members, \
                 got {names:?}"
            );
            walked.extend(names);
            assert!(
                walked.len() <= expected.len(),
                "case maxResults={page_size}: paging must terminate, walked {walked:?}"
            );
            token = page.body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case maxResults={page_size}: walking every page must enumerate every member once, \
             in member-name order"
        );
    }
}

/// `DeleteUser` removes the record and every membership that pointed at it: the
/// describe then answers 404, the listing no longer carries it, no group lists it, and
/// the objects beside it are untouched.
#[tokio::test]
async fn deleting_a_user_removes_it_and_its_memberships() {
    let router = emulator();
    create_group(&router, DIRECTORY, &json!({"SAMAccountName": "engineers"})).await;
    create_group(&router, DIRECTORY, &json!({"SAMAccountName": "oncall"})).await;
    for name in ["jdoe", "asmith", "bwong"] {
        create_user(&router, DIRECTORY, &json!({"SAMAccountName": name})).await;
        for group in ["engineers", "oncall"] {
            add_group_member(
                &router,
                DIRECTORY,
                &json!({"GroupName": group, "MemberName": name}),
            )
            .await;
        }
    }

    let removed = delete_user(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"})).await;
    assert_eq!(
        removed.status, 200,
        "DeleteUser must be accepted, got {} with {:?}",
        removed.status, removed.body
    );

    let described = describe_user(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"})).await;
    assert_eq!(
        described.status, 404,
        "a deleted user must not be described, got {} with {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.error_code(),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {:?}",
        described.body
    );
    assert_eq!(
        listed_user_names(&list_users(&router, DIRECTORY, &json!({})).await),
        vec!["asmith".to_string(), "bwong".to_string()],
        "a deleted user must be gone from the listing, and the others left alone"
    );
    for group in ["engineers", "oncall"] {
        let listed = list_group_members(&router, DIRECTORY, &json!({"SAMAccountName": group})).await;
        assert_eq!(
            listed_member_names(&listed),
            vec!["asmith".to_string(), "bwong".to_string()],
            "group {group:?}: a deleted user must be gone from its memberships, got {:?}",
            listed.body
        );
    }

    // A second delete has nothing to remove, and neither has a name nothing created.
    for case in ["jdoe", "nobody"] {
        let again = delete_user(&router, DIRECTORY, &json!({"SAMAccountName": case})).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: deleting what the directory does not hold must be a 404, \
             got {} with {:?}",
            again.status, again.body
        );
    }

    // The name is free again, and the user that takes it is a new object with its own
    // SID and no memberships.
    let recreated = create_user(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"})).await;
    assert_eq!(
        recreated.status, 200,
        "the name of a deleted user must be usable again, got {} with {:?}",
        recreated.status, recreated.body
    );
    let listed = list_group_members(&router, DIRECTORY, &json!({"SAMAccountName": "engineers"})).await;
    assert!(
        !listed_member_names(&listed).contains(&"jdoe".to_string()),
        "a recreated user must not inherit the deleted user's memberships, got {:?}",
        listed.body
    );
}

/// A `ClientToken` makes a call idempotent for the documented window: the same token
/// with the same parameters answers with the first call's result, the same token with
/// different parameters conflicts, and distinct tokens are distinct calls.
#[tokio::test]
async fn a_client_token_replays_the_call_it_first_served() {
    let router = emulator();

    let first = create_user(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "jdoe", "GivenName": "J", "ClientToken": "token-one"}),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first create must be accepted, got {} with {:?}",
        first.status, first.body
    );

    let replay = create_user(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "jdoe", "GivenName": "J", "ClientToken": "token-one"}),
    )
    .await;
    assert_eq!(
        replay.status, 200,
        "a replay of the same call must be accepted, got {} with {:?}",
        replay.status, replay.body
    );
    assert_eq!(
        replay.text("SID"),
        first.text("SID"),
        "a replay must answer with the object the token already created, got {:?}",
        replay.body
    );

    let changed = create_user(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "jdoe", "GivenName": "Jay", "ClientToken": "token-one"}),
    )
    .await;
    assert_eq!(
        changed.status, 409,
        "the same token with different parameters must conflict, got {} with {:?}",
        changed.status, changed.body
    );
    assert_eq!(
        changed.error_code(),
        "ConflictException",
        "expected ConflictException, got {:?}",
        changed.body
    );

    // A delete is idempotent the same way: the replay answers as the first call did
    // rather than as the 404 a second delete would be.
    let deleted = delete_user(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "jdoe", "ClientToken": "token-two"}),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteUser must be accepted, got {} with {:?}",
        deleted.status, deleted.body
    );
    let delete_replay = delete_user(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "jdoe", "ClientToken": "token-two"}),
    )
    .await;
    assert_eq!(
        delete_replay.status, 200,
        "a replayed delete must answer as the first one did, got {} with {:?}",
        delete_replay.status, delete_replay.body
    );

    // An add is too, and the replay adds nothing twice.
    create_user(&router, DIRECTORY, &json!({"SAMAccountName": "asmith"})).await;
    create_group(&router, DIRECTORY, &json!({"SAMAccountName": "engineers"})).await;
    for attempt in 0..2 {
        let added = add_group_member(
            &router,
            DIRECTORY,
            &json!({
                "GroupName": "engineers",
                "MemberName": "asmith",
                "ClientToken": "token-three",
            }),
        )
        .await;
        assert_eq!(
            added.status, 200,
            "attempt {attempt}: a replayed add must answer as the first one did, got {} with {:?}",
            added.status, added.body
        );
    }
    assert_eq!(
        listed_member_names(&list_group_members(&router, DIRECTORY, &json!({"SAMAccountName": "engineers"})).await),
        vec!["asmith".to_string()],
        "a replayed add must leave one membership, not two"
    );

    // Distinct tokens are distinct calls: two creates, two objects, two SIDs.
    let mut sids = Vec::new();
    for (index, token) in ["token-four", "token-five"].iter().enumerate() {
        let created = create_user(
            &router,
            DIRECTORY,
            &json!({"SAMAccountName": format!("user{index}"), "ClientToken": token}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "token {token:?}: the create must be accepted, got {} with {:?}",
            created.status, created.body
        );
        sids.push(created.text("SID"));
    }
    assert_ne!(
        sids[0], sids[1],
        "distinct client tokens must name distinct objects, got {sids:?}"
    );
}

/// A request outside a modelled constraint is refused with `ValidationException`
/// before it reaches the store; one inside every constraint is served.
#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = emulator();
    create_user(&router, DIRECTORY, &json!({"SAMAccountName": "stored"})).await;

    // DirectoryId: `^d-[0-9a-f]{10}$`. A value outside the pattern is the constraint
    // layer's refusal; a well-formed id the account does not hold is the authorization
    // gate's (403), which the `directory_authorization` suite owns.
    let directory_cases = [
        ("not a directory id at all", "bad-id", 400),
        ("too few hex digits", "d-123456789", 400),
        ("too many hex digits", "d-12345678901", 400),
        ("hex digits out of range", "d-zzzzzzzzzz", 400),
        ("upper-case hex", "d-123456789A", 400),
        ("a well-formed id the account does not hold", "d-ffffffffff", 403),
        ("a well-formed id the account holds", UNUSED_DIRECTORY, 200),
    ];
    for (case, directory, status) in directory_cases {
        let answer = list_users(&router, directory, &json!({})).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
        }
    }

    // SAMAccountName: `UserName` is 1..20 characters of `^[\w\-.]+$`.
    let name_cases = [
        ("the shortest name admitted", "a", 200),
        ("the longest name admitted", "12345678901234567890", 200),
        ("one character too long", "123456789012345678901", 400),
        ("empty", "", 400),
        ("a character the pattern excludes", "j@doe", 400),
        ("a space", "j doe", 400),
    ];
    for (case, name, status) in name_cases {
        let answer = create_user(&router, DIRECTORY, &json!({"SAMAccountName": name})).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 400 {
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
        }
    }

    // A required member that is absent is refused, not defaulted.
    for (case, request) in [
        ("no SAMAccountName", json!({})),
        ("only optional members", json!({"GivenName": "J"})),
    ] {
        let refused = create_user(&router, DIRECTORY, &request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a missing required member must be refused, got {} with {:?}",
            refused.status, refused.body
        );
    }

    // MaxResults: `@range(min: 1, max: 250)` — each published bound, and one past it.
    for (case, max_results, status) in [
        ("the lowest page size admitted", 1_i64, 200),
        ("the largest page size admitted", MAX_RESULTS_CEILING, 200),
        ("one below the lowest", 0_i64, 400),
        ("one above the largest", MAX_RESULTS_CEILING + 1, 400),
    ] {
        let answer = list_users(&router, DIRECTORY, &json!({"MaxResults": max_results})).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // An attribute map outside `Attributes`'s 1..25 is refused too.
    let mut too_many = serde_json::Map::new();
    for index in 0..26 {
        // `LdapDisplayName` is `^[A-Za-z*][A-Za-z-*]*$`, so the names carry no digits.
        let name: String = std::iter::repeat_n('a', index + 1).collect();
        too_many.insert(name, json!({"S": "value"}));
    }
    for (case, attributes, status) in [
        ("one attribute", json!({"description": {"S": "value"}}), 200),
        ("an empty attribute map", json!({}), 400),
        ("twenty-six attributes", Value::Object(too_many), 400),
    ] {
        let answer = create_user(
            &router,
            DIRECTORY,
            &json!({"SAMAccountName": format!("attr{}", case.len()), "OtherAttributes": attributes}),
        )
        .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // Nothing refused reached the store: only the users the accepted creates stored
    // are there.
    let stored = listed_user_names(&list_users(&router, DIRECTORY, &json!({})).await);
    assert!(
        !stored.iter().any(|name| name.contains('@') || name.contains(' ')),
        "a refused create must store nothing, got {stored:?}"
    );
}

/// `Realm` names the domain a call reads: naming the directory's own realm reads the
/// directory's objects, and naming another realm reads a domain this directory holds
/// no objects in.
#[tokio::test]
async fn a_realm_names_the_domain_a_call_reads() {
    let router = emulator();
    create_user(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"})).await;
    create_group(&router, DIRECTORY, &json!({"SAMAccountName": "engineers"})).await;
    add_group_member(
        &router,
        DIRECTORY,
        &json!({"GroupName": "engineers", "MemberName": "jdoe"}),
    )
    .await;

    let own_realm = describe_user(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"}))
        .await
        .text("Realm");
    assert!(
        looks_like_a_realm(&own_realm),
        "the directory must report a domain name, got {own_realm:?}"
    );

    // The directory's own realm, in any case, reads the directory's objects.
    for spelling in [own_realm.clone(), own_realm.to_uppercase()] {
        let described = describe_user(
            &router,
            DIRECTORY,
            &json!({"SAMAccountName": "jdoe", "Realm": spelling}),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {spelling:?}: the directory's own realm is case insensitive, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.text("SID"),
            describe_user(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"}))
                .await
                .text("SID"),
            "case {spelling:?}: it must read the same object"
        );

        let listed = list_users(&router, DIRECTORY, &json!({"Realm": spelling})).await;
        assert_eq!(
            listed_user_names(&listed),
            vec!["jdoe".to_string()],
            "case {spelling:?}: it must list the same objects, got {:?}",
            listed.body
        );
    }

    // Another realm is a domain this directory holds nothing in.
    let other_realm = "trusted.example.net";
    let described = describe_user(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "jdoe", "Realm": other_realm}),
    )
    .await;
    assert_eq!(
        described.status, 404,
        "a user of another realm is not held here, got {} with {:?}",
        described.status, described.body
    );
    let listed = list_users(&router, DIRECTORY, &json!({"Realm": other_realm})).await;
    assert_eq!(
        listed.status, 200,
        "listing another realm must be accepted, got {} with {:?}",
        listed.status, listed.body
    );
    assert!(
        listed_user_names(&listed).is_empty(),
        "another realm holds no users here, got {:?}",
        listed.body
    );
    assert_eq!(
        listed.body["Realm"],
        json!(other_realm),
        "the answer must report the realm it read, got {:?}",
        listed.body
    );
    let members = list_group_members(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "engineers", "MemberRealm": other_realm}),
    )
    .await;
    assert!(
        listed_member_names(&members).is_empty(),
        "another member realm holds no members here, got {:?}",
        members.body
    );
    assert_eq!(
        members.body["MemberRealm"],
        json!(other_realm),
        "the answer must report the member realm it read, got {:?}",
        members.body
    );
}

/// Two directories are two stores: an object of one is invisible in the other, and
/// their objects' SIDs come from different domains.
#[tokio::test]
async fn each_directory_holds_its_own_objects() {
    let router = emulator();
    let mut domains = Vec::new();
    for directory in [DIRECTORY, OTHER_DIRECTORY] {
        let created = create_user(&router, directory, &json!({"SAMAccountName": "jdoe"})).await;
        assert_eq!(
            created.status, 200,
            "directory {directory}: the same name is free in each directory, got {} with {:?}",
            created.status, created.body
        );
        let sid = created.text("SID");
        let (domain, _) = split_sid(&sid)
            .unwrap_or_else(|| panic!("directory {directory}: SID {sid:?} is not a domain SID"));
        domains.push(domain);
    }
    assert_ne!(
        domains[0], domains[1],
        "two directories must be two domains, got {domains:?}"
    );

    // Deleting one directory's user leaves the other's alone.
    delete_user(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"})).await;
    assert_eq!(
        describe_user(&router, OTHER_DIRECTORY, &json!({"SAMAccountName": "jdoe"}))
            .await
            .status,
        200,
        "a delete in one directory must not reach another's objects"
    );
}
