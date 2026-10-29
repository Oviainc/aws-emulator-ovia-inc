// SPDX-License-Identifier: CC0-1.0

//! `ListUsers` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `ListUsers` REPORTS, from stored state, the users of the
//! pool named by `UserPoolId` that the submitted `Filter` selects — one
//! comparison, `=` for an exact match or `^=` for a prefix match, of one
//! searchable standard attribute against a quoted value, where an empty filter
//! selects every user — each entry rendered from the STORED user and carrying the
//! attributes `AttributesToGet` names, in one stable order, a page of at most
//! `Limit` entries at a time, where the `PaginationToken` of a non-final page
//! resumes that same search strictly after the last user the page served and the
//! final page carries none. It writes nothing: a repeated call answers the same,
//! and so does every other reader of the same state. An absent pool is a 400
//! `ResourceNotFoundException`; a filter that is not such a comparison, and a
//! `PaginationToken` this search never minted, are a 400
//! `InvalidParameterException`.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// `QueryLimitType`'s modelled range is 0..60
/// (`smithy_query(query="operation",operation="ListUsers")`), so 60 is the most
/// one page can hold and what a request naming no usable `Limit` gets.
const MAX_LIMIT: usize = 60;

/// A password satisfying a new pool's default policy (at least 8 characters with
/// an uppercase letter, a lowercase letter, a number and a symbol), used to
/// confirm a user through `AdminSetUserPassword`.
const PERMANENT_PASSWORD: &str = "Str0ng-Pass!";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, body: &Value) -> String {
    let created = ok(router, "CreateUserPool", body).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create `username` in `user_pool_id` with `attributes` and return the stored
/// user as `AdminCreateUser` rendered it.
async fn user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> Value {
    let user_attributes: Vec<Value> = attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    let created = ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": user_attributes,
        }),
    )
    .await;
    created["User"].clone()
}

/// The one sentence Amazon Cognito says for input it will not accept for a
/// member of this operation — a `Filter` that is no search, or an
/// `AttributesToGet` the results cannot serve: it states that the submitted
/// input missed the constraints, naming neither the member nor the value.
const CONSTRAINT_MESSAGE: &str = "Input fails to satisfy the constraints.";

/// The one sentence `AdminCreateUser` says for a username the pool already
/// holds, which names no username.
const USERNAME_EXISTS_MESSAGE: &str = "User account already exists";

/// The `message` of an error body, under either casing the wire uses.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// One `ListUsers` request body: the optional members a case leaves unset are
/// left OUT rather than sent as null.
fn list_request(
    user_pool_id: &str,
    filter: Option<&str>,
    limit: Option<i32>,
    pagination_token: Option<&str>,
    attributes_to_get: Option<&[&str]>,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    if let Some(filter) = filter {
        body.insert("Filter".to_string(), json!(filter));
    }
    if let Some(limit) = limit {
        body.insert("Limit".to_string(), json!(limit));
    }
    if let Some(token) = pagination_token {
        body.insert("PaginationToken".to_string(), json!(token));
    }
    if let Some(names) = attributes_to_get {
        body.insert("AttributesToGet".to_string(), json!(names));
    }
    Value::Object(body)
}

/// A plain listing of `user_pool_id` under `filter`.
fn search(user_pool_id: &str, filter: Option<&str>) -> Value {
    list_request(user_pool_id, filter, None, None, None)
}

/// The `Users` entries of one response, requiring the member to be present.
fn users_of(payload: &Value, label: &str) -> Vec<Value> {
    payload["Users"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: Users must be a list, got {payload}"))
        .clone()
}

/// The `Username`s of one response, in the order it served them.
fn names_of(payload: &Value, label: &str) -> Vec<String> {
    users_of(payload, label)
        .iter()
        .map(|entry| {
            entry["Username"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: an entry must name a user, got {entry}"))
                .to_string()
        })
        .collect()
}

/// The names of the attributes one entry carries.
fn attribute_names_of(entry: &Value, label: &str) -> Vec<String> {
    entry["Attributes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: Attributes must be a list, got {entry}"))
        .iter()
        .map(|attribute| {
            attribute["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: an attribute is named, got {attribute}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of `user_pool_id`'s listing under `filter` with `limit`,
/// returning the entries in the order they were served and how many pages it
/// took, asserting on the way that no page holds more than `limit` entries and
/// that only a non-final page carries a `PaginationToken`.
async fn walk(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    filter: Option<&str>,
    limit: Option<i32>,
    label: &str,
) -> (Vec<Value>, usize) {
    // A request naming no `Limit`, or naming `0` — a page that may hold no item
    // terminates no pagination loop — takes the modelled maximum.
    let page_size = match limit {
        Some(limit) if limit > 0 => (limit as usize).min(MAX_LIMIT),
        _ => MAX_LIMIT,
    };
    let mut served: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let payload = ok(
            router,
            "ListUsers",
            &list_request(user_pool_id, filter, limit, token.as_deref(), None),
        )
        .await;
        pages += 1;
        let page = users_of(&payload, label);
        assert!(
            page.len() <= page_size,
            "case {label:?}: page {pages} holds at most {page_size} entries, got {}",
            page.len()
        );
        served.extend(page);
        token = payload
            .get("PaginationToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match &token {
            None => break,
            Some(token) => assert!(
                !token.trim().is_empty() && !token.contains(char::is_whitespace),
                "case {label:?}: a SearchPaginationTokenType matches ^[\\S]+$, got {token:?}"
            ),
        }
        assert!(
            pages <= 1 + MAX_LIMIT * 4,
            "case {label:?}: the listing must terminate, served {pages} pages"
        );
    }
    (served, pages)
}

/// The users every search case below runs against: `(username, attributes)`.
/// They differ along the attributes set, the case of the username and of the
/// values, and whether a value is set at all.
const FIXTURE_USERS: [(&str, &[(&str, &str)]); 5] = [
    (
        "alice",
        &[
            ("email", "alice@example.com"),
            ("given_name", "Alice"),
            ("family_name", "Anderson"),
            ("name", "Alice Anderson"),
            ("preferred_username", "ali"),
            ("nickname", "al"),
        ],
    ),
    (
        "bob",
        &[
            ("email", "bob@example.com"),
            ("given_name", "Bob"),
            ("family_name", "Brown"),
            ("name", "Bob Brown"),
            ("phone_number", "+15550100"),
        ],
    ),
    (
        "Carol",
        &[("given_name", "Carol"), ("family_name", "Anderson")],
    ),
    ("dave", &[("email", "dave@example.com")]),
    ("erin", &[]),
];

/// The fixture pool: every user above, with `dave` deactivated and `erin`
/// confirmed, so the searches on the console's `Enabled` and on
/// `cognito:user_status` have more than one answer. Returns the pool id and each
/// created user's `sub`, keyed by username.
async fn fixture(router: &MultiServiceRouter, pool_name: &str) -> (String, Vec<(String, String)>) {
    let user_pool_id = pool(router, &json!({ "PoolName": pool_name })).await;
    let mut subs: Vec<(String, String)> = Vec::new();
    for (username, attributes) in FIXTURE_USERS {
        let created = user(router, &user_pool_id, username, attributes).await;
        let sub = created["Attributes"]
            .as_array()
            .expect("a created user carries its attributes")
            .iter()
            .find(|attribute| attribute["Name"] == json!("sub"))
            .and_then(|attribute| attribute["Value"].as_str())
            .expect("a created user carries its sub")
            .to_string();
        subs.push((username.to_string(), sub));
    }
    ok(
        router,
        "AdminDisableUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "dave" }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "erin",
            "Password": PERMANENT_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    (user_pool_id, subs)
}

#[tokio::test]
async fn a_listing_reports_every_stored_user_rendered_from_the_stored_user() {
    let router = router();
    let (user_pool_id, _) = fixture(&router, "listing-pool").await;

    let payload = ok(&router, "ListUsers", &search(&user_pool_id, None)).await;
    let actual = names_of(&payload, "no filter at all");
    let mut expected: Vec<String> = FIXTURE_USERS
        .iter()
        .map(|(username, _)| (*username).to_string())
        .collect();
    expected.sort();
    assert_eq!(
        actual, expected,
        "case \"no filter at all\": the listing reports every stored user exactly once: expected \
         {expected:?}, got {actual:?}"
    );
    assert!(
        payload.get("PaginationToken").is_none(),
        "case \"no filter at all\": a listing that served everything carries no PaginationToken, \
         got {payload}"
    );

    // Each entry is the STORED user, member for member — the same thing the
    // pool's own reader of that one user renders.
    for entry in users_of(&payload, "no filter at all") {
        let username = entry["Username"].as_str().expect("an entry names a user");
        let read = ok(
            &router,
            "AdminGetUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        // `AdminGetUser` renders the same stored user under its own member
        // names: `UserType.Attributes` is `AdminGetUser`'s `UserAttributes`.
        for (entry_member, read_member) in [
            ("Username", "Username"),
            ("Attributes", "UserAttributes"),
            ("UserStatus", "UserStatus"),
            ("Enabled", "Enabled"),
            ("UserCreateDate", "UserCreateDate"),
            ("UserLastModifiedDate", "UserLastModifiedDate"),
        ] {
            assert_eq!(
                entry[entry_member], read[read_member],
                "case \"no filter at all\": the entry's {entry_member} for {username:?} must be \
                 the {read_member} AdminGetUser renders, got {entry} and {read}"
            );
        }
    }

    // The listing is a READ: calling it again answers the same.
    let again = ok(&router, "ListUsers", &search(&user_pool_id, None)).await;
    assert_eq!(
        again, payload,
        "case \"no filter at all\": a repeated listing answers the same, got {again}"
    );
}

#[tokio::test]
async fn a_filter_selects_the_stored_users_whose_searched_value_matches() {
    let router = router();
    let (user_pool_id, subs) = fixture(&router, "search-pool").await;
    let alice_sub = subs
        .iter()
        .find(|(username, _)| username == "alice")
        .map(|(_, sub)| sub.clone())
        .expect("alice was created above");

    // (case, filter, the usernames it selects, in the listing's order)
    //
    // The rows differ along the searched attribute (every entry of the
    // documented catalog in `ListUsers.Filter`), the comparison (`=` and `^=`),
    // the number of users selected (none, one, several), the spacing of the
    // filter, and the case of the value.
    let rows: Vec<(&str, Option<String>, Vec<&str>)> = vec![
        (
            "an absent filter selects every user",
            None,
            vec!["Carol", "alice", "bob", "dave", "erin"],
        ),
        (
            "an empty filter selects every user",
            Some(String::new()),
            vec!["Carol", "alice", "bob", "dave", "erin"],
        ),
        (
            "a filter of nothing but space selects every user",
            Some("   ".to_string()),
            vec!["Carol", "alice", "bob", "dave", "erin"],
        ),
        (
            "an exact match on the username",
            Some("username = \"alice\"".to_string()),
            vec!["alice"],
        ),
        (
            "an exact match on the username in another case, which is case-sensitive here",
            Some("username = \"ALICE\"".to_string()),
            vec![],
        ),
        (
            "a prefix match on the username",
            Some("username ^= \"a\"".to_string()),
            vec!["alice"],
        ),
        (
            "a prefix match that selects nobody",
            Some("username ^= \"zz\"".to_string()),
            vec![],
        ),
        (
            "an exact match on email",
            Some("email = \"bob@example.com\"".to_string()),
            vec!["bob"],
        ),
        (
            "a prefix match on email",
            Some("email ^= \"dave\"".to_string()),
            vec!["dave"],
        ),
        (
            "an exact match on family_name shared by two users",
            Some("family_name = \"Anderson\"".to_string()),
            vec!["Carol", "alice"],
        ),
        (
            "an exact match on given_name",
            Some("given_name = \"Bob\"".to_string()),
            vec!["bob"],
        ),
        (
            "an exact match on name, whose value holds a space",
            Some("name = \"Bob Brown\"".to_string()),
            vec!["bob"],
        ),
        (
            "an exact match on phone_number",
            Some("phone_number = \"+15550100\"".to_string()),
            vec!["bob"],
        ),
        (
            "a prefix match on phone_number",
            Some("phone_number ^= \"+1555\"".to_string()),
            vec!["bob"],
        ),
        (
            "an exact match on preferred_username",
            Some("preferred_username = \"ali\"".to_string()),
            vec!["alice"],
        ),
        (
            "an exact match on sub, the user's own id",
            Some(format!("sub = \"{alice_sub}\"")),
            vec!["alice"],
        ),
        (
            "the confirmation status, which an administrator-created user carries",
            Some("cognito:user_status = \"FORCE_CHANGE_PASSWORD\"".to_string()),
            vec!["Carol", "alice", "bob", "dave"],
        ),
        (
            "the confirmation status in another case, which is case-insensitive",
            Some("cognito:user_status = \"force_change_password\"".to_string()),
            vec!["Carol", "alice", "bob", "dave"],
        ),
        (
            "the confirmation status of the user whose password was made permanent",
            Some("cognito:user_status = \"CONFIRMED\"".to_string()),
            vec!["erin"],
        ),
        (
            "a prefix match on the confirmation status",
            Some("cognito:user_status ^= \"FORCE\"".to_string()),
            vec!["Carol", "alice", "bob", "dave"],
        ),
        (
            "the sign-in state the console calls Enabled",
            Some("status = \"Enabled\"".to_string()),
            vec!["Carol", "alice", "bob", "erin"],
        ),
        (
            "the sign-in state of the deactivated user",
            Some("status = \"Disabled\"".to_string()),
            vec!["dave"],
        ),
        (
            "no space around the comparison",
            Some("given_name=\"Alice\"".to_string()),
            vec!["alice"],
        ),
        (
            "space around the whole filter",
            Some("  given_name = \"Alice\"  ".to_string()),
            vec!["alice"],
        ),
        (
            "a value no user holds",
            Some("email = \"nobody@example.com\"".to_string()),
            vec![],
        ),
        (
            "an attribute only some users have set, where an unset value matches nothing",
            Some("preferred_username ^= \"\"".to_string()),
            vec!["alice"],
        ),
    ];

    for (label, filter, expected) in rows {
        let payload = ok(&router, "ListUsers", &search(&user_pool_id, filter.as_deref())).await;
        let actual = names_of(&payload, label);
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            actual, expected,
            "case {label:?} (filter {filter:?}): expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_case_insensitive_pool_searches_its_usernames_without_case() {
    let router = router();
    // `UsernameConfigurationType.CaseSensitive` false makes "both
    // preferred_username and email alias ... in addition to the username
    // attribute" case-insensitive for this pool.
    let user_pool_id = pool(
        &router,
        &json!({
            "PoolName": "case-insensitive-pool",
            "UsernameConfiguration": { "CaseSensitive": false },
        }),
    )
    .await;
    user(
        &router,
        &user_pool_id,
        "Alice",
        &[("preferred_username", "Ali"), ("family_name", "Anderson")],
    )
    .await;

    for (label, filter, expected) in [
        (
            "the username as stored",
            "username = \"Alice\"",
            vec!["Alice"],
        ),
        (
            "the username in another case",
            "username = \"ALICE\"",
            vec!["Alice"],
        ),
        (
            "a prefix of the username in another case",
            "username ^= \"al\"",
            vec!["Alice"],
        ),
        (
            "preferred_username in another case, which this pool aliases without case",
            "preferred_username = \"ali\"",
            vec!["Alice"],
        ),
        (
            "an attribute that is no alias, which this setting does not reach",
            "family_name = \"anderson\"",
            vec![],
        ),
    ] {
        let payload = ok(&router, "ListUsers", &search(&user_pool_id, Some(filter))).await;
        let actual = names_of(&payload, label);
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            actual, expected,
            "case {label:?} (filter {filter:?}): expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn attributes_to_get_names_the_attributes_each_entry_carries() {
    let router = router();
    let (user_pool_id, _) = fixture(&router, "projection-pool").await;

    // (case, AttributesToGet, the attribute names alice's entry carries)
    //
    // `alice` holds `email`, `given_name`, `family_name`, `name`,
    // `preferred_username`, `nickname` and the `sub` the pool minted, and every
    // row below selects her alone, so every requested attribute is one the
    // results hold.
    for (label, attributes_to_get, expected) in [
        (
            "no AttributesToGet at all, which returns every attribute",
            None,
            vec![
                "email",
                "family_name",
                "given_name",
                "name",
                "nickname",
                "preferred_username",
                "sub",
            ],
        ),
        ("one attribute", Some(vec!["email"]), vec!["email"]),
        (
            "several attributes, where the response keeps the stored order",
            Some(vec!["given_name", "email"]),
            vec!["email", "given_name"],
        ),
        (
            "an attribute no server-side search covers, set for every user in the results",
            Some(vec!["nickname"]),
            vec!["nickname"],
        ),
        ("no attribute at all", Some(vec![]), vec![]),
    ] {
        let body = list_request(
            &user_pool_id,
            Some("username = \"alice\""),
            None,
            None,
            attributes_to_get.as_deref(),
        );
        let payload = ok(&router, "ListUsers", &body).await;
        let entries = users_of(&payload, label);
        assert_eq!(
            entries.len(),
            1,
            "case {label:?}: the search selects exactly alice, got {payload}"
        );
        let actual = attribute_names_of(&entries[0], label);
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
        // Projecting the attributes changes nothing else about the entry.
        assert_eq!(
            entries[0]["UserStatus"],
            json!("FORCE_CHANGE_PASSWORD"),
            "case {label:?}: every other member is the stored user's, got {}",
            entries[0]
        );
    }

    // A SEARCHABLE attribute is served whoever holds it: `email` is one, and the
    // users that have set none simply carry no attribute at all.
    let payload = ok(
        &router,
        "ListUsers",
        &list_request(&user_pool_id, None, None, None, Some(&["email"])),
    )
    .await;
    for entry in users_of(&payload, "a searchable attribute some users leave unset") {
        let username = entry["Username"].as_str().expect("an entry names a user");
        let expected: Vec<String> = FIXTURE_USERS
            .iter()
            .find(|(stored, _)| *stored == username)
            .map(|(_, attributes)| attributes)
            .expect("every entry is a fixture user")
            .iter()
            .filter(|(name, _)| *name == "email")
            .map(|(name, _)| (*name).to_string())
            .collect();
        let actual = attribute_names_of(&entry, username);
        assert_eq!(
            actual, expected,
            "case \"a searchable attribute some users leave unset\" ({username:?}): expected \
             {expected:?}, got {actual:?}"
        );
    }

    // An attribute no server-side search covers has to be set for every user in
    // the results before it can be asked for at all.
    for (label, filter, attributes_to_get) in [
        (
            "an attribute no user in the results holds",
            Some("username = \"alice\""),
            vec!["address"],
        ),
        (
            "one held attribute beside one no user in the results holds",
            Some("username = \"alice\""),
            vec!["email", "address"],
        ),
        (
            "an attribute only some of the selected users hold",
            None,
            vec!["nickname"],
        ),
    ] {
        let body = list_request(
            &user_pool_id,
            filter,
            None,
            None,
            Some(&attributes_to_get),
        );
        let (status, payload) = call(&router, "ListUsers", &body).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", CONSTRAINT_MESSAGE),
            "case {label:?} (AttributesToGet {attributes_to_get:?}): got {status} with {payload}"
        );
    }

    // The rule is about the users the SEARCH selects, not the page served, so
    // one search answers the same whatever page of it a request asks for: this
    // first page holds only `alice`, who HAS a nickname, and is refused because
    // a later page's user has none.
    let (status, payload) = call(
        &router,
        "ListUsers",
        &list_request(
            &user_pool_id,
            Some("email ^= \"\""),
            Some(1),
            None,
            Some(&["nickname"]),
        ),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload), error_message(&payload)),
        (400, "InvalidParameterException", CONSTRAINT_MESSAGE),
        "case \"the first page of a search a later page's user cannot serve\": got {status} with \
         {payload}"
    );
}

#[tokio::test]
async fn a_search_pages_over_one_stable_order_under_every_limit() {
    let router = router();
    let user_pool_id = pool(&router, &json!({ "PoolName": "paging-pool" })).await;
    // Seven users, so the page sizes below split them evenly, unevenly and not
    // at all. They are created in an order that is neither ascending nor
    // descending, so the served order comes from stored state, not from arrival.
    // Four of them share a `family_name`, so a filtered listing pages too.
    let usernames = [
        "mallory", "alice", "zoe", "bob", "trent", "carol", "peggy",
    ];
    for (index, username) in usernames.iter().enumerate() {
        let family_name = if index % 2 == 0 { "Even" } else { "Odd" };
        user(
            &router,
            &user_pool_id,
            username,
            &[("family_name", family_name)],
        )
        .await;
    }

    for (label, filter) in [
        ("every user", None),
        ("the users one search selects", Some("family_name = \"Even\"")),
    ] {
        // Whatever order the implementation stores, paging must serve the SAME
        // sequence under every page size — the order an unpaged listing serves.
        let whole = ok(&router, "ListUsers", &search(&user_pool_id, filter)).await;
        let expected = names_of(&whole, label);

        // The rows span the published bounds of `Limit` (0 and 60), sizes that
        // divide and do not divide the selection, and an absent `Limit`.
        for (limit_label, limit) in [
            ("no Limit at all", None),
            ("Limit 1, the smallest page that makes progress", Some(1)),
            ("Limit 2", Some(2)),
            ("Limit 3", Some(3)),
            ("Limit 60, the modelled maximum", Some(60)),
            ("Limit 0, the modelled minimum", Some(0)),
        ] {
            let case = format!("{label} with {limit_label}");
            let (served, pages) = walk(&router, &user_pool_id, filter, limit, &case).await;
            let actual: Vec<String> = served
                .iter()
                .map(|entry| entry["Username"].as_str().expect("a name").to_string())
                .collect();
            assert_eq!(
                actual, expected,
                "case {case:?}: paging serves every selected user exactly once in the listing's \
                 order: expected {expected:?}, got {actual:?}"
            );
            let expected_pages = match limit {
                Some(limit) if limit > 0 => {
                    expected.len().div_ceil(limit as usize).max(1)
                }
                _ => 1,
            };
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages, got {pages}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_reports_one_pool() {
    let router = router();
    let first = pool(&router, &json!({ "PoolName": "first-pool" })).await;
    let second = pool(&router, &json!({ "PoolName": "second-pool" })).await;
    user(&router, &first, "alice", &[("given_name", "Alice")]).await;
    user(&router, &second, "bob", &[("given_name", "Bob")]).await;

    for (label, user_pool_id, filter, expected) in [
        ("the pool a user was created in", &first, None, vec!["alice"]),
        ("another pool", &second, None, vec!["bob"]),
        (
            "a search for the other pool's user",
            &second,
            Some("username = \"alice\""),
            vec![],
        ),
    ] {
        let payload = ok(&router, "ListUsers", &search(user_pool_id, filter)).await;
        let actual = names_of(&payload, label);
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_user_leaves_the_listing_and_a_created_one_joins_it() {
    let router = router();
    let user_pool_id = pool(&router, &json!({ "PoolName": "roundtrip-pool" })).await;
    for username in ["alice", "bob"] {
        user(&router, &user_pool_id, username, &[]).await;
    }

    ok(
        &router,
        "AdminDeleteUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice" }),
    )
    .await;
    let payload = ok(&router, "ListUsers", &search(&user_pool_id, None)).await;
    let actual = names_of(&payload, "after one user was deleted");
    assert_eq!(
        actual,
        vec!["bob".to_string()],
        "case \"after one user was deleted\": the deleted user is gone from the listing, got \
         {actual:?}"
    );

    user(&router, &user_pool_id, "carol", &[("given_name", "Carol")]).await;
    let payload = ok(
        &router,
        "ListUsers",
        &search(&user_pool_id, Some("given_name = \"Carol\"")),
    )
    .await;
    let actual = names_of(&payload, "after another user was created");
    assert_eq!(
        actual,
        vec!["carol".to_string()],
        "case \"after another user was created\": the created user is in the listing, got \
         {actual:?}"
    );
}

#[tokio::test]
async fn an_absent_pool_a_filter_that_is_no_search_and_an_unissued_token_are_refused() {
    let router = router();
    let (user_pool_id, _) = fixture(&router, "refusal-pool").await;

    // A token this search DID mint, for the users the console calls Enabled: the
    // rows below echo it back to a listing it names no page of.
    let first_page = ok(
        &router,
        "ListUsers",
        &list_request(&user_pool_id, Some("status = \"Enabled\""), Some(1), None, None),
    )
    .await;
    let enabled_token = first_page["PaginationToken"]
        .as_str()
        .expect("a non-final page carries a PaginationToken")
        .to_string();

    for (label, body, expected) in [
        (
            "an absent user pool",
            search("us-east-1_absent", None),
            (400, "ResourceNotFoundException"),
        ),
        (
            "a filter that is a bare attribute name",
            search(&user_pool_id, Some("username")),
            (400, "InvalidParameterException"),
        ),
        (
            "a comparison this operation does not accept",
            search(&user_pool_id, Some("email ~= \"bob@example.com\"")),
            (400, "InvalidParameterException"),
        ),
        (
            "a value that is not quoted",
            search(&user_pool_id, Some("email = bob@example.com")),
            (400, "InvalidParameterException"),
        ),
        (
            "a value whose quotation mark is never closed",
            search(&user_pool_id, Some("email = \"bob@example.com")),
            (400, "InvalidParameterException"),
        ),
        (
            "two comparisons, where a server-side filter matches no more than one attribute",
            search(
                &user_pool_id,
                Some("email = \"bob@example.com\" and given_name = \"Bob\""),
            ),
            (400, "InvalidParameterException"),
        ),
        (
            "a custom attribute, which is not searchable",
            search(&user_pool_id, Some("custom:department = \"finance\"")),
            (400, "InvalidParameterException"),
        ),
        (
            "a standard attribute outside the searchable catalog",
            search(&user_pool_id, Some("nickname = \"al\"")),
            (400, "InvalidParameterException"),
        ),
        (
            "a PaginationToken no listing of this service minted",
            list_request(&user_pool_id, None, None, Some("not-a-token"), None),
            (400, "InvalidParameterException"),
        ),
        (
            "a PaginationToken that is a username rather than a cursor",
            list_request(&user_pool_id, None, None, Some("alice"), None),
            (400, "InvalidParameterException"),
        ),
        (
            "a PaginationToken minted for another search",
            list_request(&user_pool_id, None, None, Some(&enabled_token), None),
            (400, "InvalidParameterException"),
        ),
    ] {
        let (status, payload) = call(&router, "ListUsers", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }

    // The refusals persisted nothing: the pool still holds every user.
    let payload = ok(&router, "ListUsers", &search(&user_pool_id, None)).await;
    let actual = names_of(&payload, "after the refusals");
    let mut expected: Vec<String> = FIXTURE_USERS
        .iter()
        .map(|(username, _)| (*username).to_string())
        .collect();
    expected.sort();
    assert_eq!(
        actual, expected,
        "case \"after the refusals\": a refused listing changes no stored user, got {actual:?}"
    );
}

#[tokio::test]
async fn every_filter_that_is_no_search_is_refused_with_one_sentence() {
    let router = router();
    let (user_pool_id, _) = fixture(&router, "filter-message-pool").await;

    // The rows differ along WHY the filter is no search: an attribute no
    // server-side search covers (custom or standard), a comparison this
    // operation does not accept, a value that is not one quoted string, and a
    // string carrying more than one comparison. All of them are the same refusal.
    for (label, filter) in [
        ("a bare attribute name", "username"),
        ("a comparison this operation does not accept", "email ~= \"a\""),
        ("a value that is not quoted", "email = bob@example.com"),
        (
            "a value whose quotation mark is never closed",
            "email = \"bob@example.com",
        ),
        (
            "two comparisons in one filter",
            "email = \"bob@example.com\" and given_name = \"Bob\"",
        ),
        (
            "a custom attribute, which is not searchable",
            "custom:department = \"finance\"",
        ),
        (
            "a standard attribute outside the searchable catalog",
            "nickname = \"al\"",
        ),
        (
            "a searchable name in another case, which is not that name",
            "Username = \"alice\"",
        ),
    ] {
        let (status, payload) = call(&router, "ListUsers", &search(&user_pool_id, Some(filter))).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", CONSTRAINT_MESSAGE),
            "case {label:?} (filter {filter:?}): got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn a_username_the_pool_already_holds_is_refused_with_one_fixed_sentence() {
    // Every user of the searches above is created through `AdminCreateUser`, so
    // the refusal that operation answers for a username the pool already holds
    // belongs to this suite: it is ONE fixed sentence naming no username, so the
    // refusal reports nothing about which usernames a pool holds. The rows differ
    // along the FORM of the username.
    let router = router();
    let user_pool_id = pool(&router, &json!({ "PoolName": "exists-pool" })).await;

    for (label, username) in [
        ("a plain username", "alice"),
        ("a username holding punctuation", "ada.lovelace+1"),
        ("a username in mixed case", "Carol"),
    ] {
        user(&router, &user_pool_id, username, &[]).await;
        let (status, payload) = call(
            &router,
            "AdminCreateUser",
            &json!({ "UserPoolId": user_pool_id, "Username": username }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "UsernameExistsException", USERNAME_EXISTS_MESSAGE),
            "case {label:?} ({username:?}): got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, &json!({ "PoolName": "constraint-pool" })).await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="ListUsers")`): `Limit` range
    // 0..60, `Filter` length 0..256, `UserPoolId` length 1..55 with pattern
    // `^[\w-]+_[0-9a-zA-Z]+$`, and `PaginationToken` length 1.. with pattern
    // `^[\S]+$`.
    for (label, body) in [
        (
            "Limit 61, one past the range maximum 60",
            list_request(&user_pool_id, None, Some(61), None, None),
        ),
        (
            "Limit -1, one below the range minimum 0",
            list_request(&user_pool_id, None, Some(-1), None, None),
        ),
        (
            "a filter of 257 characters, past the length maximum 256",
            search(
                &user_pool_id,
                Some(&format!("given_name = \"{}\"", "v".repeat(242))),
            ),
        ),
        (
            "a user pool id the pattern excludes",
            search("no-underscore", None),
        ),
        (
            "an empty PaginationToken, below the length minimum 1",
            list_request(&user_pool_id, None, None, Some(""), None),
        ),
        (
            "a PaginationToken holding a space, which the pattern excludes",
            list_request(&user_pool_id, None, None, Some("two words"), None),
        ),
        ("a request missing the required UserPoolId", json!({})),
    ] {
        let (status, payload) = call(&router, "ListUsers", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with \
             {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }
}
