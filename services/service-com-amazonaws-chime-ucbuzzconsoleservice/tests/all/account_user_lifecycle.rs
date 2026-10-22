// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Chime account + user lifecycle.
//!
//! Every case drives real HTTP through [`MultiServiceRouter`] — the same front door the
//! router mounts in production — so the request walk (route key, deserialize, validate,
//! handle, serialize) is exercised exactly as an AWS SDK would exercise it. One shared
//! engine backs each router, so what one request writes is what the next one reads.
//!
//! The cases are tables: each row is one input in the class the rule covers, and every
//! assertion names its row and reports the actual value.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`chime`), which is what
/// the framework's route-key resolution reads to pick the claiming service. No
/// authenticator is registered on the test router, so the signature is never verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/chime/aws4_request, SignedHeaders=host, Signature=0000";

/// A router carrying only this service, over one fresh shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_chime_ucbuzzconsoleservice::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and its parsed JSON body (`Value::Null` for an
/// empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let body_bytes = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://chime.us-east-1.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(body_bytes)
        .expect("build request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.body().clone();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|err| {
            panic!(
                "{method} {path}: response body is not JSON ({err}): {}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// Create one account and return its `Account` object.
async fn create_account(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "POST", "/accounts", Some(json!({ "Name": name }))).await;
    assert_eq!(status, 201, "case {name:?}: CreateAccount answered {status}, body {body}");
    body.get("Account")
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: CreateAccount returned no Account: {body}"))
}

/// The `AccountId` of an `Account` object.
fn account_id(account: &Value) -> String {
    account
        .get("AccountId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("Account carries no AccountId: {account}"))
        .to_string()
}

/// The wire error code (`__type`) of an error body.
fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("error body carries no __type: {body}"))
        .to_string()
}

/// A created account is reported from the store, and reading it back by `AccountId`
/// returns the identical record — over a table of names spanning the modelled
/// `AccountName` class (shortest, longest, spaces, non-ASCII, duplicated).
#[tokio::test]
async fn created_accounts_round_trip_through_get_account() {
    let router = router();
    let longest = "x".repeat(100); // `Name` is @length(min:1, max:100)
    let cases: [&str; 6] = [
        "BootstrapAccount",
        "a",
        "name with inner spaces",
        &longest,
        "名前-ünïcode",
        "BootstrapAccount", // the same name twice: two distinct accounts
    ];

    let mut ids = BTreeSet::new();
    for name in cases {
        let created = create_account(&router, name).await;
        let id = account_id(&created);

        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {name:?}: CreateAccount echoed the wrong Name: {created}"
        );
        assert_eq!(
            created.get("AwsAccountId").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {name:?}: unexpected AwsAccountId: {created}"
        );
        assert!(
            created
                .get("CreatedTimestamp")
                .is_some_and(|ts| !ts.is_null()),
            "case {name:?}: CreateAccount set no CreatedTimestamp: {created}"
        );
        assert_eq!(
            created.get("AccountStatus").and_then(Value::as_str),
            Some("Active"),
            "case {name:?}: unexpected AccountStatus: {created}"
        );
        assert_eq!(
            created.get("AccountType").and_then(Value::as_str),
            Some("Team"),
            "case {name:?}: unexpected AccountType: {created}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {name:?}: AccountId {id} was handed out twice"
        );

        // The read-back must be the SAME record, not a re-derivation of it.
        let (status, body) = call(&router, "GET", &format!("/accounts/{id}"), None).await;
        assert_eq!(status, 200, "case {name:?}: GetAccount answered {status}, body {body}");
        assert_eq!(
            body.get("Account"),
            Some(&created),
            "case {name:?}: GetAccount returned a different Account than CreateAccount"
        );

        // And it is stable across reads: a read changes nothing.
        let (_, again) = call(&router, "GET", &format!("/accounts/{id}"), None).await;
        assert_eq!(
            again.get("Account"),
            Some(&created),
            "case {name:?}: a second GetAccount returned a different Account"
        );
    }

    assert_eq!(ids.len(), 6, "expected six distinct accounts, got {ids:?}");
}

/// `ListAccounts` reports exactly the accounts in the store, and paging with
/// `max-results` walks every account exactly once — over a table of (account count,
/// page size) pairs, including a page size larger than the result set and the modelled
/// maximum.
#[tokio::test]
async fn list_accounts_pages_over_every_account_exactly_once() {
    for count in [1_usize, 2, 3, 5] {
        for max_results in [1_usize, 2, 3, 200] {
            let case = format!("{count} accounts, max-results={max_results}");
            let router = router();
            let mut created = BTreeSet::new();
            for index in 0..count {
                created.insert(account_id(
                    &create_account(&router, &format!("account-{index}")).await,
                ));
            }

            let mut seen: Vec<String> = Vec::new();
            let mut pages = 0_usize;
            let mut token: Option<String> = None;
            loop {
                let path = match &token {
                    Some(token) => {
                        format!("/accounts?max-results={max_results}&next-token={token}")
                    }
                    None => format!("/accounts?max-results={max_results}"),
                };
                let (status, body) = call(&router, "GET", &path, None).await;
                assert_eq!(status, 200, "case {case}: ListAccounts answered {status}, body {body}");
                let accounts = body
                    .get("Accounts")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| panic!("case {case}: ListAccounts returned no Accounts: {body}"))
                    .clone();
                pages += 1;
                assert!(
                    accounts.len() <= max_results,
                    "case {case}: a page carried {} accounts, more than max-results",
                    accounts.len()
                );
                for account in &accounts {
                    seen.push(account_id(account));
                }
                token = body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                if token.is_none() {
                    break;
                }
                assert_eq!(
                    accounts.len(),
                    max_results,
                    "case {case}: a page that carried a NextToken was not full"
                );
                assert!(pages <= count + 1, "case {case}: paging did not terminate");
            }

            let expected_pages = count.div_ceil(max_results);
            assert_eq!(
                pages, expected_pages,
                "case {case}: walked {pages} pages, expected {expected_pages}"
            );
            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                seen.len(),
                unique.len(),
                "case {case}: an account was reported on more than one page: {seen:?}"
            );
            assert_eq!(
                unique, created,
                "case {case}: ListAccounts did not report exactly the stored accounts"
            );
        }
    }
}

/// `ListAccounts` filters by the `name` prefix it documents, and by the email address of
/// a user stored under an account.
#[tokio::test]
async fn list_accounts_filters_by_name_prefix_and_user_email() {
    let router = router();
    let alpha = account_id(&create_account(&router, "alpha-one").await);
    let alpha_two = account_id(&create_account(&router, "alpha-two").await);
    let beta = account_id(&create_account(&router, "beta-one").await);

    // `Name` is documented as an account name PREFIX filter.
    let cases: [(&str, Vec<&String>); 4] = [
        ("alpha", vec![&alpha, &alpha_two]),
        ("alpha-one", vec![&alpha]),
        ("beta", vec![&beta]),
        ("gamma", vec![]),
    ];
    for (prefix, expected) in cases {
        let (status, body) = call(&router, "GET", &format!("/accounts?name={prefix}"), None).await;
        assert_eq!(status, 200, "case name={prefix}: answered {status}, body {body}");
        let got: BTreeSet<String> = body
            .get("Accounts")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case name={prefix}: no Accounts: {body}"))
            .iter()
            .map(account_id)
            .collect();
        let want: BTreeSet<String> = expected.into_iter().cloned().collect();
        assert_eq!(got, want, "case name={prefix}: wrong accounts reported");
    }

    // `UserEmail` returns the account the user with that address belongs to.
    create_user(&router, &beta, "bob", Some("bob@example.com"), None).await;
    let (status, body) = call(
        &router,
        "GET",
        "/accounts?user-email=bob%40example.com",
        None,
    )
    .await;
    assert_eq!(status, 200, "case user-email: answered {status}, body {body}");
    let got: Vec<String> = body
        .get("Accounts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case user-email: no Accounts: {body}"))
        .iter()
        .map(account_id)
        .collect();
    assert_eq!(
        got,
        vec![beta.clone()],
        "case user-email: expected only the account holding that user"
    );

    let (status, body) = call(
        &router,
        "GET",
        "/accounts?user-email=nobody%40example.com",
        None,
    )
    .await;
    assert_eq!(status, 200, "case user-email=unknown: answered {status}, body {body}");
    assert_eq!(
        body.get("Accounts").and_then(Value::as_array).map(Vec::len),
        Some(0),
        "case user-email=unknown: expected no accounts, body {body}"
    );
}

/// Create one user on `account_id` and return its `User` object.
async fn create_user(
    router: &MultiServiceRouter,
    account_id: &str,
    username: &str,
    email: Option<&str>,
    user_type: Option<&str>,
) -> Value {
    let mut request = json!({ "Username": username });
    if let Some(email) = email {
        request["Email"] = json!(email);
    }
    if let Some(user_type) = user_type {
        request["UserType"] = json!(user_type);
    }
    let (status, body) = call(
        router,
        "POST",
        &format!("/accounts/{account_id}/users?operation=create"),
        Some(request),
    )
    .await;
    assert_eq!(status, 201, "case {username:?}: CreateUser answered {status}, body {body}");
    body.get("User")
        .cloned()
        .unwrap_or_else(|| panic!("case {username:?}: CreateUser returned no User: {body}"))
}

/// A created user is stored under its account: `GetUser` returns the identical record and
/// `ListUsers` reports it — over a table of users spanning both modelled `UserType`
/// values, a user with no email and a user with no type.
#[tokio::test]
async fn created_users_round_trip_through_get_user_and_list_users() {
    let router = router();
    let account = account_id(&create_account(&router, "users-account").await);

    // `UserType` is an enum of exactly PrivateUser | SharedDevice; `Username` and
    // `Email` are optional, so a row leaves each out in turn.
    let cases: [(&str, Option<&str>, Option<&str>, &str); 4] = [
        ("alice", Some("alice@example.com"), Some("PrivateUser"), "PrivateUser"),
        ("lobby-phone", Some("lobby@example.com"), Some("SharedDevice"), "SharedDevice"),
        ("carol", Some("carol@example.co.uk"), None, "PrivateUser"),
        ("dave", None, Some("PrivateUser"), "PrivateUser"),
    ];

    let mut expected_ids = BTreeSet::new();
    for (username, email, user_type, expected_type) in cases {
        let created = create_user(&router, &account, username, email, user_type).await;
        let user_id = created
            .get("UserId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {username}: User carries no UserId: {created}"))
            .to_string();

        assert_eq!(
            created.get("AccountId").and_then(Value::as_str),
            Some(account.as_str()),
            "case {username}: User reports the wrong AccountId: {created}"
        );
        assert_eq!(
            created.get("PrimaryEmail").and_then(Value::as_str),
            email,
            "case {username}: wrong PrimaryEmail: {created}"
        );
        assert_eq!(
            created.get("DisplayName").and_then(Value::as_str),
            Some(username),
            "case {username}: wrong DisplayName: {created}"
        );
        assert_eq!(
            created.get("UserType").and_then(Value::as_str),
            Some(expected_type),
            "case {username}: wrong UserType: {created}"
        );
        assert_eq!(
            created.get("LicenseType").and_then(Value::as_str),
            Some("Pro"),
            "case {username}: wrong LicenseType: {created}"
        );
        assert!(
            created
                .get("PersonalPIN")
                .and_then(Value::as_str)
                .is_some_and(|pin| pin.len() == 10 && pin.chars().all(char::is_numeric)),
            "case {username}: PersonalPIN is not a 10-digit PIN: {created}"
        );
        assert!(
            expected_ids.insert(user_id.clone()),
            "case {username}: UserId {user_id} was handed out twice"
        );

        let (status, body) = call(
            &router,
            "GET",
            &format!("/accounts/{account}/users/{user_id}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {username}: GetUser answered {status}, body {body}");
        assert_eq!(
            body.get("User"),
            Some(&created),
            "case {username}: GetUser returned a different User than CreateUser"
        );
    }

    let (status, body) = call(&router, "GET", &format!("/accounts/{account}/users"), None).await;
    assert_eq!(status, 200, "ListUsers answered {status}, body {body}");
    let listed: BTreeSet<String> = body
        .get("Users")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListUsers returned no Users: {body}"))
        .iter()
        .map(|user| {
            user.get("UserId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        listed, expected_ids,
        "ListUsers did not report exactly the users created on the account"
    );
}

/// `ListUsers` honours the `user-email` and `user-type` filters and pages with
/// `max-results`/`next-token`.
#[tokio::test]
async fn list_users_filters_and_pages() {
    let router = router();
    let account = account_id(&create_account(&router, "filtered").await);
    let alice = create_user(
        &router,
        &account,
        "alice",
        Some("alice@example.com"),
        Some("PrivateUser"),
    )
    .await;
    let phone = create_user(
        &router,
        &account,
        "phone",
        Some("phone@example.com"),
        Some("SharedDevice"),
    )
    .await;
    let eve = create_user(
        &router,
        &account,
        "eve",
        Some("eve@example.com"),
        Some("PrivateUser"),
    )
    .await;
    let user_id = |user: &Value| {
        user.get("UserId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    let cases: [(&str, Vec<String>); 5] = [
        ("", vec![user_id(&alice), user_id(&phone), user_id(&eve)]),
        ("?user-type=PrivateUser", vec![user_id(&alice), user_id(&eve)]),
        ("?user-type=SharedDevice", vec![user_id(&phone)]),
        ("?user-email=eve%40example.com", vec![user_id(&eve)]),
        ("?user-email=nobody%40example.com", vec![]),
    ];
    for (query, expected) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/accounts/{account}/users{query}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {query:?}: ListUsers answered {status}, body {body}");
        let got: BTreeSet<String> = body
            .get("Users")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {query:?}: no Users: {body}"))
            .iter()
            .map(user_id)
            .collect();
        let want: BTreeSet<String> = expected.into_iter().collect();
        assert_eq!(got, want, "case {query:?}: wrong users reported");
    }

    // Paging over the three users, one per page.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..3 {
        let path = match &token {
            Some(token) => format!("/accounts/{account}/users?max-results=1&next-token={token}"),
            None => format!("/accounts/{account}/users?max-results=1"),
        };
        let (status, body) = call(&router, "GET", &path, None).await;
        assert_eq!(status, 200, "case page {page}: ListUsers answered {status}, body {body}");
        let users = body
            .get("Users")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case page {page}: no Users: {body}"))
            .clone();
        assert_eq!(
            users.len(),
            1,
            "case page {page}: expected one user per page, got {}",
            users.len()
        );
        seen.push(user_id(&users[0]));
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        assert_eq!(
            token.is_some(),
            page < 2,
            "case page {page}: NextToken presence is wrong (last page must carry none)"
        );
    }
    let unique: BTreeSet<String> = seen.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        3,
        "paging reported a user twice or missed one: {seen:?}"
    );
}

/// A deleted account, and everything under it, is gone for every later reader.
#[tokio::test]
async fn deleting_an_account_removes_it_and_its_users() {
    let router = router();
    let kept = account_id(&create_account(&router, "kept").await);
    let doomed = account_id(&create_account(&router, "doomed").await);
    let doomed_user = create_user(&router, &doomed, "alice", Some("alice@example.com"), None).await;
    let doomed_user_id = doomed_user
        .get("UserId")
        .and_then(Value::as_str)
        .expect("UserId")
        .to_string();

    let (status, body) = call(&router, "DELETE", &format!("/accounts/{doomed}"), None).await;
    assert_eq!(status, 204, "DeleteAccount answered {status}, body {body}");
    // `DeleteAccountResponse` models no members, so the 204 carries nothing a client can
    // read — the framework renders that as the empty JSON document.
    assert!(
        body.as_object().is_some_and(serde_json::Map::is_empty) || body.is_null(),
        "DeleteAccount returned response members: {body}"
    );

    let gone: [(&str, String, u16); 4] = [
        ("GET", format!("/accounts/{doomed}"), 404),
        ("GET", format!("/accounts/{doomed}/users"), 404),
        (
            "GET",
            format!("/accounts/{doomed}/users/{doomed_user_id}"),
            404,
        ),
        ("DELETE", format!("/accounts/{doomed}"), 404),
    ];
    for (method, path, expected) in gone {
        let (status, body) = call(&router, method, &path, None).await;
        assert_eq!(
            status, expected,
            "case {method} {path}: answered {status}, body {body}"
        );
        assert_eq!(
            error_type(&body),
            "NotFoundException",
            "case {method} {path}: wrong error shape: {body}"
        );
        assert_eq!(
            body.get("Code").and_then(Value::as_str),
            Some("NotFound"),
            "case {method} {path}: wrong modelled Code: {body}"
        );
    }

    // The surviving account is untouched.
    let (status, body) = call(&router, "GET", "/accounts", None).await;
    assert_eq!(status, 200, "ListAccounts answered {status}, body {body}");
    let listed: Vec<String> = body
        .get("Accounts")
        .and_then(Value::as_array)
        .expect("Accounts")
        .iter()
        .map(account_id)
        .collect();
    assert_eq!(
        listed,
        vec![kept],
        "ListAccounts still reports the deleted account: {body}"
    );
}

/// An operation that names a resource the store does not hold answers 404
/// `NotFoundException` — never a 501 and never a success.
#[tokio::test]
async fn unknown_resources_answer_not_found() {
    let router = router();
    let account = account_id(&create_account(&router, "present").await);

    let cases: [(&str, String, Option<Value>); 4] = [
        ("GET", "/accounts/no-such-account".to_string(), None),
        (
            "POST",
            "/accounts/no-such-account/users?operation=create".to_string(),
            Some(json!({ "Username": "alice", "Email": "alice@example.com" })),
        ),
        ("GET", "/accounts/no-such-account/users".to_string(), None),
        (
            "GET",
            format!("/accounts/{account}/users/no-such-user"),
            None,
        ),
    ];
    for (method, path, body) in cases {
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 404,
            "case {method} {path}: answered {status}, body {response}"
        );
        assert_eq!(
            error_type(&response),
            "NotFoundException",
            "case {method} {path}: wrong error shape: {response}"
        );
    }
}

/// Two users cannot hold the same email address within one account, and the refusal is
/// the modelled 409 `ConflictException`; the same address on a different account is fine.
#[tokio::test]
async fn duplicate_user_email_conflicts_within_one_account() {
    let router = router();
    let first = account_id(&create_account(&router, "first").await);
    let second = account_id(&create_account(&router, "second").await);
    create_user(&router, &first, "alice", Some("alice@example.com"), None).await;

    let (status, body) = call(
        &router,
        "POST",
        &format!("/accounts/{first}/users?operation=create"),
        Some(json!({ "Username": "alice-again", "Email": "alice@example.com" })),
    )
    .await;
    assert_eq!(status, 409, "duplicate email answered {status}, body {body}");
    assert_eq!(
        error_type(&body),
        "ConflictException",
        "duplicate email: wrong error shape: {body}"
    );

    // Another account is a different namespace.
    let elsewhere = create_user(&router, &second, "alice", Some("alice@example.com"), None).await;
    assert_eq!(
        elsewhere.get("AccountId").and_then(Value::as_str),
        Some(second.as_str()),
        "the same address on another account must be accepted: {elsewhere}"
    );
}

/// A value outside what the model declares is refused with HTTP 400
/// `BadRequestException`, at every published bound and one past it.
#[tokio::test]
async fn values_outside_the_model_are_refused_as_bad_requests() {
    let router = router();
    let account = account_id(&create_account(&router, "validation").await);
    let at_max = "x".repeat(100); // `Name` @length max
    let past_max = "x".repeat(101);

    // Accepted: both published bounds of `Name`'s @length(1..100) and of
    // `MaxResults`'s @range(1..200).
    let accepted: [(&str, String, Option<Value>); 4] = [
        ("POST", "/accounts".to_string(), Some(json!({ "Name": "a" }))),
        (
            "POST",
            "/accounts".to_string(),
            Some(json!({ "Name": at_max })),
        ),
        ("GET", "/accounts?max-results=1".to_string(), None),
        ("GET", "/accounts?max-results=200".to_string(), None),
    ];
    for (method, path, body) in accepted {
        let (status, response) = call(&router, method, &path, body).await;
        assert!(
            (200..300).contains(&status),
            "case {method} {path}: a value inside the declared bound was refused with \
             {status}, body {response}"
        );
    }

    let refused: [(&str, String, Option<Value>); 7] = [
        // `Name` is @required.
        ("POST", "/accounts".to_string(), Some(json!({}))),
        // @length(min:1).
        ("POST", "/accounts".to_string(), Some(json!({ "Name": "" }))),
        // @pattern(\S): a blank name carries no non-whitespace character.
        ("POST", "/accounts".to_string(), Some(json!({ "Name": "   " }))),
        // One past @length(max:100).
        (
            "POST",
            "/accounts".to_string(),
            Some(json!({ "Name": past_max })),
        ),
        // One below and one above `MaxResults`'s @range(1..200).
        ("GET", "/accounts?max-results=0".to_string(), None),
        ("GET", "/accounts?max-results=201".to_string(), None),
        // `Email` is @pattern(^.+@.+\..+$).
        (
            "POST",
            format!("/accounts/{account}/users?operation=create"),
            Some(json!({ "Username": "alice", "Email": "not-an-email" })),
        ),
    ];
    for (method, path, body) in refused {
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {method} {path}: answered {status}, body {response}"
        );
        assert_eq!(
            error_type(&response),
            "BadRequestException",
            "case {method} {path}: wrong error shape: {response}"
        );
    }

    // A `NextToken` this service never issued is a bad request, not a panic or a page —
    // including one too long to be a token it could have issued (the model puts no
    // @length on NextToken, so the service imposes its own ceiling).
    let overlong = "61".repeat(400);
    for path in [
        "/accounts?next-token=not-a-token".to_string(),
        format!("/accounts?next-token={overlong}"),
        format!("/accounts/{account}/users?next-token=not-a-token"),
        format!("/accounts/{account}/users?next-token={overlong}"),
    ] {
        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 400,
            "case {path}: answered {status}, body {response}"
        );
        assert_eq!(
            error_type(&response),
            "BadRequestException",
            "case {path}: wrong error shape: {response}"
        );
    }
}

/// The store has a ceiling: a create past this emulator's capacity guard is refused with
/// the service's own `ResourceLimitExceededException` (HTTP 400) rather than growing the
/// in-memory store without bound, and the accounts already created stay readable.
#[tokio::test]
async fn creating_past_the_store_capacity_is_refused() {
    let router = router();
    let mut first = None;
    for index in 0..service_com_amazonaws_chime_ucbuzzconsoleservice::state::MAX_ACCOUNTS {
        let account = create_account(&router, &format!("account-{index}")).await;
        if first.is_none() {
            first = Some(account_id(&account));
        }
    }

    let (status, body) = call(
        &router,
        "POST",
        "/accounts",
        Some(json!({ "Name": "one-too-many" })),
    )
    .await;
    assert_eq!(status, 400, "at capacity: answered {status}, body {body}");
    assert_eq!(
        error_type(&body),
        "ResourceLimitExceededException",
        "at capacity: wrong error shape: {body}"
    );
    assert_eq!(
        body.get("Code").and_then(Value::as_str),
        Some("ResourceLimitExceeded"),
        "at capacity: wrong modelled Code: {body}"
    );

    // The refusal did not disturb what was already stored.
    let id = first.expect("at least one account was created");
    let (status, body) = call(&router, "GET", &format!("/accounts/{id}"), None).await;
    assert_eq!(
        status, 200,
        "after the refusal, GetAccount answered {status}, body {body}"
    );
}
