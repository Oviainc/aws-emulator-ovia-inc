// SPDX-License-Identifier: CC0-1.0

//! MemoryDB users, end to end over the wire.
//!
//! Every case here drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router, so what
//! is asserted is the emulator's HTTP behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * `CreateUser` persists the user and `DescribeUsers` answers from that state — same
//!   name, access string, authentication type, password count and ARN;
//! * a user's passwords are never reported: only `Authentication$PasswordCount`, whose
//!   documented maximum is two, so a request carrying more passwords is refused;
//! * a password is accepted exactly when it is printable ASCII and between 16 and 128
//!   characters long — tested at each published bound and one past it, and on the
//!   punctuation the Developer Guide's prose excludes but the service accepts;
//! * a resource name is a case-insensitive identifier: the create echoes the folded
//!   spelling and either spelling reads the one user back;
//! * `AuthenticationMode.Type` and `AuthenticationMode.Passwords` must agree: a
//!   password user needs passwords, an IAM user accepts none;
//! * ACL membership is stored once (on the ACL) and reported on both sides, so
//!   `User$ACLNames` lists every ACL whose `UserNames` names the user;
//! * a listing pages over the whole set in name order for any `MaxResults`, and a
//!   filter narrows it by a property of `User`;
//! * re-creating a user that exists is the modelled already-exists fault, and naming
//!   one that does not is the modelled not-found fault.
//!
//! `DescribeUsers` is the reader these suites round-trip through, and the store it
//! reads also holds the built-in `default` user MemoryDB puts in every account, so
//! every whole-set assertion below expects that user beside the created ones.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The suite-wide harness: ONE SigV4 request builder, ONE router constructor and ONE
// error-code reader for every suite in this crate.
use crate::cluster_lifecycle::{call, error_code, ok, router};

/// The region the suite-wide harness signs its requests for
/// (`crate::cluster_lifecycle::REGION`), which every ARN the emulator mints must carry.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The built-in user MemoryDB creates in every account
/// (`crate::state::DEFAULT_USER_NAME`), which every listing reports beside the users
/// these suites create.
const DEFAULT_USER: &str = "default";

/// The documented password length bounds for a MemoryDB user (AWS, "Authenticating
/// users with Access Control Lists": "must be at least 16 characters and no more than
/// 128 characters in length"), mirrored from `crate::state::MIN_PASSWORD_LEN` /
/// `MAX_PASSWORD_LEN`. Each is exercised AT the bound and one past it.
pub(crate) const MIN_PASSWORD_LEN: usize = 16;
pub(crate) const MAX_PASSWORD_LEN: usize = 128;

/// The most passwords one user may hold: `Authentication$PasswordCount` — "The number
/// of passwords belonging to the user. The maximum is two."
const MAX_PASSWORDS_PER_USER: usize = 2;

/// An access string that is valid for every user the suite creates (MemoryDB's
/// "full access" string: enabled, all keys, all channels, all commands).
pub(crate) const FULL_ACCESS: &str = "on ~* &* +@all";

/// An `AuthenticationMode` of type `password` carrying `passwords`.
///
/// `pub(crate)` so the update suite refuses the SAME modes through the same builder.
pub(crate) fn password_mode(passwords: &[&str]) -> Value {
    json!({ "Type": "password", "Passwords": passwords })
}

/// A password of exactly `length` characters that breaks none of the other rules.
pub(crate) fn password_of_length(length: usize) -> String {
    "Aa1!".chars().cycle().take(length).collect()
}

/// `CreateUser` with the full-access string and one password.
///
/// `pub(crate)` so the sibling suites create their users through the SAME call rather\n/// than each spelling out an `AuthenticationMode`.
pub(crate) async fn create_user(router: &MultiServiceRouter, name: &str) -> Value {
    ok(
        router,
        "CreateUser",
        &json!({
            "UserName": name,
            "AccessString": FULL_ACCESS,
            "AuthenticationMode": password_mode(&[&password_of_length(MIN_PASSWORD_LEN)]),
        }),
        name,
    )
    .await
}

/// The `Name` of every user in a `DescribeUsers` body, in wire order. `pub(crate)`
/// for the sibling suites, so one reader serves every listing assertion.
pub(crate) fn user_names(body: &Value) -> Vec<String> {
    body["Users"]
        .as_array()
        .unwrap_or_else(|| panic!("expected Users in {body}"))
        .iter()
        .map(|user| {
            user["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("expected Name in {user}"))
                .to_string()
        })
        .collect()
}

/// A create persists the user and the describe answers from that state: the SAME name,
/// access string, authentication type, password count and ARN come back, for every
/// shape of `AuthenticationMode` the service accepts.
#[tokio::test]
async fn a_created_user_is_read_back_from_state() {
    let router = router();
    // (user name, access string, AuthenticationMode, expected Authentication, expected
    // stored AccessString, expected MinimumEngineVersion) — rows differ in
    // authentication type, password count, access string, the case the name was written
    // in, and whether the access string already names its pub/sub channels.
    let cases: Vec<(&str, &str, Value, Value, &str, &str)> = vec![
        (
            "one-password-user",
            FULL_ACCESS,
            password_mode(&[&password_of_length(MIN_PASSWORD_LEN)]),
            json!({ "Type": "password", "PasswordCount": 1 }),
            FULL_ACCESS,
            "6.2",
        ),
        (
            "two-password-user",
            "on ~app:* +@read",
            password_mode(&[
                &password_of_length(MIN_PASSWORD_LEN),
                &password_of_length(MAX_PASSWORD_LEN),
            ]),
            json!({ "Type": "password", "PasswordCount": 2 }),
            // The service canonicalises the rule list, which always states the pub/sub
            // channel permission.
            "on ~app:* resetchannels +@read",
            "6.2",
        ),
        (
            "IamUser1",
            "off ~* +@admin",
            json!({ "Type": "iam" }),
            json!({ "Type": "iam" }),
            "off ~* resetchannels +@admin",
            // IAM authentication arrived with engine version 7.0.
            "7.0",
        ),
    ];

    for (name, access_string, mode, expected_authentication, expected_access, expected_version) in
        &cases
    {
        let case = format!("create {name}");
        // A MemoryDB resource name is a case-insensitive identifier: the service stores
        // and reports the folded spelling.
        let stored_name = name.to_ascii_lowercase();
        let created = ok(
            &router,
            "CreateUser",
            &json!({
                "UserName": name,
                "AccessString": access_string,
                "AuthenticationMode": mode,
                "Tags": [{ "Key": "team", "Value": "platform" }],
            }),
            &case,
        )
        .await;
        let user = &created["User"];
        assert_eq!(
            user["Name"],
            json!(stored_name),
            "case {case:?}: expected Name {stored_name:?}, got {user}"
        );
        assert_eq!(
            user["AccessString"],
            json!(expected_access),
            "case {case:?}: expected AccessString {expected_access:?}, got {user}"
        );
        assert_eq!(
            user["Status"],
            json!("active"),
            "case {case:?}: a created user is usable at once, got {user}"
        );
        assert_eq!(
            user["Authentication"], *expected_authentication,
            "case {case:?}: expected Authentication {expected_authentication}, got {user}"
        );
        assert_eq!(
            user["MinimumEngineVersion"],
            json!(expected_version),
            "case {case:?}: expected MinimumEngineVersion {expected_version:?}, got {user}"
        );
        assert_eq!(
            user["ARN"],
            json!(format!(
                "arn:aws:memorydb:{REGION}:{ACCOUNT}:user/{stored_name}"
            )),
            "case {case:?}: the ARN must carry the request's region and the user name, \
             got {user}"
        );
        assert_eq!(
            user["ACLNames"],
            json!([]),
            "case {case:?}: a user no ACL names belongs to none, got {user}"
        );
        // The passwords are secrets: nothing about them but the count is reported.
        let rendered = created.to_string();
        for password in mode["Passwords"].as_array().unwrap_or(&Vec::new()) {
            let password = password.as_str().unwrap_or_default();
            assert!(
                !rendered.contains(password),
                "case {case:?}: a password must never be echoed, got {created}"
            );
        }

        // The round trip: the describe reports exactly what the create persisted, and
        // finds the user under either spelling of its name.
        for spelling in [*name, stored_name.as_str()] {
            let described = ok(
                &router,
                "DescribeUsers",
                &json!({ "UserName": spelling }),
                &case,
            )
            .await;
            assert_eq!(
                described["Users"],
                json!([user]),
                "case {case:?}: the describe of {spelling:?} must answer with the stored \
                 user, got {described}"
            );
        }
    }

    // And the unfiltered listing holds every one of them, in name order, beside the
    // built-in user the account already had.
    let listed = ok(&router, "DescribeUsers", &json!({}), "listing").await;
    let mut expected: Vec<String> = cases
        .iter()
        .map(|(name, ..)| name.to_ascii_lowercase())
        .chain([DEFAULT_USER.to_string()])
        .collect();
    expected.sort();
    assert_eq!(
        user_names(&listed),
        expected,
        "case \"listing\": expected every created user in name order, got {listed}"
    );
}

/// A password is accepted exactly when it is printable ASCII and between
/// [`MIN_PASSWORD_LEN`] and [`MAX_PASSWORD_LEN`] characters long. Each bound is
/// exercised AT the bound and one past it, and the punctuation the Developer Guide's
/// prose excludes but the SERVICE accepts is exercised character by character.
#[tokio::test]
async fn a_password_is_accepted_only_when_it_meets_every_documented_rule() {
    let router = router();
    let at_min = password_of_length(MIN_PASSWORD_LEN);
    let at_max = password_of_length(MAX_PASSWORD_LEN);
    let below_min = password_of_length(MIN_PASSWORD_LEN - 1);
    let above_max = password_of_length(MAX_PASSWORD_LEN + 1);
    // Every printable ASCII graphic character, plus the space: the only character rule
    // is that a password holds nothing outside this set.
    let punctuation = "Aa1!#$%^&*()-_=+[]{}|;:'<>.?~`\\$,\"/@ ";
    // (case, password, accepted) — rows differ in length, in which punctuation they
    // carry, and in whether they are printable ASCII at all.
    let mut cases: Vec<(String, String, bool)> = vec![
        ("at the minimum length".to_string(), at_min.clone(), true),
        ("at the maximum length".to_string(), at_max, true),
        ("one below the minimum length".to_string(), below_min, false),
        ("one above the maximum length".to_string(), above_max, false),
        (
            "every accepted character at once".to_string(),
            punctuation.to_string(),
            true,
        ),
        (
            "a control character".to_string(),
            format!("{at_min}\tTab"),
            false,
        ),
        (
            "a non-ASCII character".to_string(),
            format!("{at_min}café"),
            false,
        ),
        (
            "the DEL control character".to_string(),
            format!("{at_min}\u{7f}"),
            false,
        ),
    ];
    // The punctuation the Developer Guide's prose says a password "cannot contain",
    // which the service accepts: one row per character, so no one of them is excused
    // by another.
    for character in [',', '"', '/', '@', ' ', '\\'] {
        cases.push((
            format!("a password containing {character:?}"),
            format!("{at_min}{character}x"),
            true,
        ));
    }

    for (index, (case, password, accepted)) in cases.iter().enumerate() {
        let name = format!("pw-user-{index}");
        let (status, body) = call(
            &router,
            "CreateUser",
            &json!({
                "UserName": name,
                "AccessString": FULL_ACCESS,
                "AuthenticationMode": password_mode(&[password.as_str()]),
            }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the password meets every rule, so the create must \
                 succeed; got {status} with {body}"
            );
            assert_eq!(
                body["User"]["Authentication"]["PasswordCount"],
                json!(1),
                "case {case:?}: expected one password, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: the password breaks a rule, so the create must be \
                 refused; got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected the modelled invalid-value fault, got {body}"
            );
            // A refused create persists nothing.
            let (status, described) =
                call(&router, "DescribeUsers", &json!({ "UserName": name })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused create must leave no user behind, got \
                 {status} with {described}"
            );
        }
    }
}

/// `AuthenticationMode.Type` and `AuthenticationMode.Passwords` must agree, and a user
/// holds at most [`MAX_PASSWORDS_PER_USER`] passwords.
#[tokio::test]
async fn an_authentication_mode_must_hold_together() {
    let router = router();
    let password = password_of_length(MIN_PASSWORD_LEN);
    // (case, AuthenticationMode, expected error code or None for a 200)
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        ("one password", password_mode(&[&password]), None),
        (
            "two passwords, the documented maximum",
            password_mode(&[&password, &format!("{password}x")]),
            None,
        ),
        (
            "one password past the documented maximum",
            password_mode(&[&password, &format!("{password}x"), &format!("{password}y")]),
            Some("InvalidParameterValueException"),
        ),
        (
            "a password user with no passwords",
            json!({ "Type": "password" }),
            Some("InvalidParameterCombinationException"),
        ),
        ("an iam user", json!({ "Type": "iam" }), None),
        (
            "an iam user carrying passwords",
            json!({ "Type": "iam", "Passwords": [password.clone()] }),
            Some("InvalidParameterCombinationException"),
        ),
        (
            "no authentication type at all",
            json!({}),
            Some("InvalidParameterValueException"),
        ),
    ];

    for (index, (case, mode, expected_code)) in cases.iter().enumerate() {
        let name = format!("mode-user-{index}");
        let (status, body) = call(
            &router,
            "CreateUser",
            &json!({
                "UserName": name,
                "AccessString": FULL_ACCESS,
                "AuthenticationMode": mode,
            }),
        )
        .await;
        match expected_code {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to succeed, got {status} with {body}"
                );
                let expected_count = mode["Passwords"].as_array().map(Vec::len);
                assert!(
                    expected_count.is_none_or(|count| count <= MAX_PASSWORDS_PER_USER),
                    "case {case:?}: the accepted rows must stay within the documented \
                     maximum, got {expected_count:?}"
                );
                assert_eq!(
                    body["User"]["Authentication"]["PasswordCount"],
                    expected_count.map_or(Value::Null, |count| json!(count)),
                    "case {case:?}: expected PasswordCount {expected_count:?}, got {body}"
                );
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the create to be refused, got {status} \
                     with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    *code,
                    "case {case:?}: expected error code {code}, got {body}"
                );
            }
        }
    }
}

/// ACL membership is stored ONCE, on the ACL's `UserNames`, and reported on both
/// sides: `User$ACLNames` lists every ACL that names the user, however many there are.
#[tokio::test]
async fn acl_membership_is_reflected_on_the_user() {
    let router = router();
    for name in ["in-both", "in-one", "in-none"] {
        create_user(&router, name).await;
    }
    // Two ACLs with overlapping membership, so a user can belong to 0, 1 or 2 of them.
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "acl-alpha", "UserNames": ["in-both", "in-one"] }),
        "acl-alpha",
    )
    .await;
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "acl-beta", "UserNames": ["in-both"] }),
        "acl-beta",
    )
    .await;

    for (name, expected) in [
        ("in-both", json!(["acl-alpha", "acl-beta"])),
        ("in-one", json!(["acl-alpha"])),
        ("in-none", json!([])),
    ] {
        let described = ok(
            &router,
            "DescribeUsers",
            &json!({ "UserName": name }),
            name,
        )
        .await;
        assert_eq!(
            described["Users"][0]["ACLNames"], expected,
            "case {name:?}: expected ACLNames {expected}, got {described}"
        );
    }

    // A user created AFTER the ACL that names it reports the same membership: the
    // listing derives it from the ACLs rather than from the order of the calls.
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "acl-later", "UserNames": ["created-last"] }),
        "acl-later",
    )
    .await;
    let created = create_user(&router, "created-last").await;
    assert_eq!(
        created["User"]["ACLNames"],
        json!(["acl-later"]),
        "case \"created-last\": a create must report the membership already in state, \
         got {created}"
    );

    // The unfiltered listing agrees with every by-name describe.
    let listed = ok(&router, "DescribeUsers", &json!({}), "listing").await;
    for user in listed["Users"].as_array().expect("Users is a list") {
        let name = user["Name"].as_str().unwrap_or_default();
        let described = ok(
            &router,
            "DescribeUsers",
            &json!({ "UserName": name }),
            name,
        )
        .await;
        assert_eq!(
            described["Users"][0], *user,
            "case {name:?}: the listing and the by-name describe must agree, got \
             {described}"
        );
    }
}

/// The listing pages over the whole set in name order for ANY page size, and a filter
/// narrows it by a property of `User`.
#[tokio::test]
async fn the_listing_pages_and_filters_over_every_user() {
    let router = router();
    // Names deliberately NOT created in sorted order, so a page order that follows the
    // calls rather than the names fails. `IamFiltered` is written in mixed case: the
    // service stores the folded spelling, which is what the listing reports.
    let names = ["user-d", "user-a", "user-c", "user-b", "IamFiltered"];
    for name in names {
        if name == "IamFiltered" {
            ok(
                &router,
                "CreateUser",
                &json!({
                    "UserName": name,
                    "AccessString": FULL_ACCESS,
                    "AuthenticationMode": { "Type": "iam" },
                }),
                name,
            )
            .await;
        } else {
            create_user(&router, name).await;
        }
    }
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "paged-acl", "UserNames": ["user-a", "user-c"] }),
        "paged-acl",
    )
    .await;
    let mut sorted: Vec<String> = names
        .iter()
        .map(|name| name.to_ascii_lowercase())
        // The built-in user the account already had pages with the created ones.
        .chain([DEFAULT_USER.to_string()])
        .collect();
    sorted.sort();

    // Every page size from 1 to one past the number of users covers the whole set,
    // in name order, exactly once.
    for page_size in 1..=sorted.len() + 1 {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = ok(&router, "DescribeUsers", &request, &case).await;
            let page_names = user_names(&page);
            assert!(
                page_names.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} users, got {page}"
            );
            seen.extend(page_names);
            token = page["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, sorted,
            "case {case:?}: the pages together must cover every user once, in name \
             order, got {seen:?}"
        );
    }

    // A filter narrows the listing by the property it names, under MemoryDB's own
    // hyphenated, lower-case filter names.
    let filter_cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "by user name",
            json!([{ "Name": "user-name", "Values": ["user-a", "user-b"] }]),
            vec!["user-a", "user-b"],
        ),
        (
            "by acl name",
            json!([{ "Name": "acl-name", "Values": ["paged-acl"] }]),
            vec!["user-a", "user-c"],
        ),
        (
            // `DescribeUsers` filters on `user-name` and `acl-name`; a name outside that
            // set is refused below rather than treated as a property of `User`.
            "by acl name, nothing belongs to it",
            json!([{ "Name": "acl-name", "Values": ["no-such-acl"] }]),
            vec![],
        ),
        (
            "two filters, both must hold",
            json!([
                { "Name": "acl-name", "Values": ["paged-acl"] },
                { "Name": "user-name", "Values": ["user-c"] },
            ]),
            vec!["user-c"],
        ),
        (
            "a filter nothing matches",
            json!([{ "Name": "user-name", "Values": ["absent"] }]),
            vec![],
        ),
    ];
    for (case, filters, expected) in filter_cases {
        let filtered = ok(
            &router,
            "DescribeUsers",
            &json!({ "Filters": filters }),
            case,
        )
        .await;
        assert_eq!(
            user_names(&filtered),
            expected,
            "case {case:?}: expected {expected:?}, got {filtered}"
        );
    }

    // A filter naming no property of `User`, or carrying no value at all, is refused
    // rather than silently answered with an unfiltered or empty page. The empty-values
    // case is caught at the boundary (`FilterValueList` is `len 1..`), and this service
    // states a refused VALUE with the one error code it models for that, whether the
    // rule was the model's or its own.
    for (case, filters, expected_code) in [
        (
            "unknown filter",
            json!([{ "Name": "NoSuchProperty", "Values": ["x"] }]),
            "InvalidParameterValueException",
        ),
        (
            "filter with no values",
            json!([{ "Name": "user-name", "Values": [] }]),
            "InvalidParameterValueException",
        ),
    ] {
        let (status, body) = call(&router, "DescribeUsers", &json!({ "Filters": filters })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected error code {expected_code}, got {body}"
        );
    }
}

/// Naming a user that is not in state is the modelled not-found fault, and re-creating
/// one that is is the modelled already-exists fault.
#[tokio::test]
async fn a_users_name_is_unique_and_must_exist_to_be_described() {
    let router = router();
    let (status, body) = call(
        &router,
        "DescribeUsers",
        &json!({ "UserName": "never-created" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"unknown user\": expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "UserNotFoundFault",
        "case \"unknown user\": expected the modelled not-found fault, got {body}"
    );

    create_user(&router, "taken-name").await;
    // A second create with a DIFFERENT body but the same name must not overwrite it.
    let (status, body) = call(
        &router,
        "CreateUser",
        &json!({
            "UserName": "taken-name",
            "AccessString": "off ~* +@read",
            "AuthenticationMode": { "Type": "iam" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"duplicate name\": expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "UserAlreadyExistsFault",
        "case \"duplicate name\": expected the modelled already-exists fault, got {body}"
    );
    let described = ok(
        &router,
        "DescribeUsers",
        &json!({ "UserName": "taken-name" }),
        "duplicate name",
    )
    .await;
    assert_eq!(
        described["Users"][0]["AccessString"],
        json!(FULL_ACCESS),
        "case \"duplicate name\": the refused create must not have changed the stored \
         user, got {described}"
    );

    // A page size below 1 names no page at all, and a token that names no position in
    // the listing is reported rather than served as an empty page.
    for (case, request) in [
        ("MaxResults 0", json!({ "MaxResults": 0 })),
        ("a token past the end", json!({ "NextToken": "zzzz" })),
    ] {
        let (status, body) = call(&router, "DescribeUsers", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {body}"
        );
    }
}
