// SPDX-License-Identifier: CC0-1.0

//! `DescribeUsers`, end to end over the wire.
//!
//! Every case here drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.DescribeUsers` through the multi-service router, so
//! what is asserted is the emulator's HTTP behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * the listing is the users STATE holds, in name order, each member answered from
//!   what the store persisted — including the built-in `default` user MemoryDB puts in
//!   every account before any call;
//! * `User$ACLNames` is DERIVED from the ACLs whose `UserNames` list the user, so it
//!   follows every ACL created afterwards;
//! * `UserName` narrows the listing to that one user, and a name state does not hold
//!   is the modelled `UserNotFoundFault`;
//! * a `Filters` entry selects the users whose named property carries one of its
//!   values, filters combine conjunctively, and a filter whose `Name` is not one of the
//!   service's filterable properties is the modelled `InvalidParameterValueException`;
//! * the listing pages over its whole set in name order for any `MaxResults`;
//! * a describe is a REPORT: two identical calls answer identically.
//!
//! `CreateUser` cannot produce a user in `modifying` or `deleting` status (`UpdateUser`
//! produces `modifying`, and only for the half-second the clock owes it; `DeleteUser` is
//! still a scaffold), so the rows covering the whole `User$Status` and
//! `AuthenticationType` catalogs are written straight into the same store the handler
//! reads. That is SETUP only: every assertion below is made on a response that came back
//! over the wire, the create path is driven end-to-end in `tests/all/user_lifecycle.rs`,
//! and the update path in `tests/all/user_updates.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::ops::types::AUTHENTICATION_TYPE_VALUES;
use service_com_amazonaws_memorydb_amazonmemorydb::state::{user_arn, StoredUser};
use service_com_amazonaws_memorydb_amazonmemorydb::{
    ComAmazonawsMemorydbAmazonmemorydbState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

// The suite-wide helpers: ONE SigV4 request builder and ONE error-code reader for
// every suite in this crate.
use crate::cluster_lifecycle::error_code;

/// The region the test's requests are signed for. Every ARN the emulator mints must
/// carry it, rather than a constant baked into the service.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The built-in user MemoryDB creates in every account, and the access string it gives
/// it (`crate::state::DEFAULT_USER_NAME` / `DEFAULT_USER_ACCESS_STRING`), plus the
/// built-in ACL that holds it (`crate::state::OPEN_ACCESS_ACL_NAME`).
const DEFAULT_USER: &str = "default";
const DEFAULT_USER_ACCESS_STRING: &str = "on ~* &* +@all";
const OPEN_ACCESS_ACL: &str = "open-access";

/// `User$Status`: "Indicates the user status. Can be \"active\", \"modifying\" or
/// \"deleting\"." — the catalog the rows below draw from, so a describe is shown to
/// report the STORED status rather than a constant.
const USER_STATUS_VALUES: &[&str] = &["active", "modifying", "deleting"];

/// A router with just this service mounted, plus the engine it was built on — what
/// `core/router` builds, narrowed to the service under test and keeping the shared
/// store reachable for setup.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn harness() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Harness { router, engine }
}

impl Harness {
    /// Call one operation, signed for [`REGION`], and return `(status, body)`.
    ///
    /// The request itself is built by the suite-wide helper in
    /// [`crate::cluster_lifecycle`] — ONE SigV4 request builder serves every suite in
    /// this crate, and the region is passed to it explicitly so this suite's ARN
    /// assertions and its requests cannot disagree.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        crate::cluster_lifecycle::call_signed(&self.router, REGION, operation, body).await
    }

    /// Call an operation that must succeed, returning its body.
    async fn ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// The names the response reports, in the order it reported them.
    async fn user_names(&self, request: &Value, case: &str) -> Vec<String> {
        let body = self.ok("DescribeUsers", request, case).await;
        body["Users"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Users is a list, got {body}"))
            .iter()
            .map(|user| user["Name"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// Write a user into the shared store, for the statuses no create can produce.
    /// The handler reads THIS store; nothing here is asserted.
    fn seed_user(&self, user: &Seeded) {
        self.engine
            .state::<ComAmazonawsMemorydbAmazonmemorydbState>()
            .lock()
            .users
            .insert(
                user.name.to_string(),
                StoredUser {
                    name: user.name.to_string(),
                    status: user.status.to_string(),
                    access_string: user.access_string.to_string(),
                    authentication_type: user.authentication_type.to_string(),
                    password_count: user.password_count,
                    // Seeded users are written already in the status the row names, so
                    // the clock owes none of them a transition.
                    settles_at: None,
                    arn: user_arn(REGION, user.name),
                },
            );
    }
}

/// One row of the stored-user table: a user the store holds, spanning the name forms a
/// folded `UserName` takes, the statuses `User$Status` lists and every
/// `AuthenticationType` the model declares.
struct Seeded {
    name: &'static str,
    status: &'static str,
    access_string: &'static str,
    authentication_type: &'static str,
    password_count: Option<i32>,
}

/// The table every suite below seeds: one row per (name form, status, authentication
/// type) combination this service has to report back unchanged.
fn seeded_users() -> Vec<Seeded> {
    vec![
        Seeded {
            name: "a",
            status: "active",
            access_string: "on ~app::* resetchannels +@read",
            authentication_type: "password",
            // `Authentication$PasswordCount`: "The number of passwords belonging to
            // the user. The maximum is two."
            password_count: Some(2),
        },
        Seeded {
            name: "app",
            status: "modifying",
            access_string: "on ~* &* +@all",
            authentication_type: "password",
            password_count: Some(1),
        },
        Seeded {
            name: "data-reader-1",
            status: "deleting",
            access_string: "off ~* resetchannels -@all",
            authentication_type: "iam",
            password_count: None,
        },
        Seeded {
            name: "zeta-service-account",
            status: "active",
            access_string: "on ~metrics:* resetchannels +get +mget",
            authentication_type: "no-password",
            password_count: None,
        },
    ]
}

/// The user the account ALREADY HAD: MemoryDB creates a `default` user in every
/// account, so a `DescribeUsers` against a store nobody has written reports it, with
/// its documented access string, a region- and account-correct ARN, and no ACL until
/// one lists it. And a describe REPORTS: the second identical call answers the first's
/// body exactly.
#[tokio::test]
async fn the_builtin_default_user_is_reported_from_an_untouched_store() {
    let harness = harness();
    let case = "untouched store";
    let first = harness.ok("DescribeUsers", &json!({}), case).await;
    assert_eq!(
        first["Users"].as_array().map(Vec::len),
        Some(1),
        "case {case:?}: expected exactly the built-in user, got {first}"
    );
    let user = &first["Users"][0];
    for (member, expected) in [
        ("Name", json!(DEFAULT_USER)),
        ("Status", json!("active")),
        ("AccessString", json!(DEFAULT_USER_ACCESS_STRING)),
        // The built-in ACL every account already has lists the built-in user.
        ("ACLNames", json!([OPEN_ACCESS_ACL])),
        ("Authentication", json!({ "Type": "no-password" })),
        // A password-less user works from MemoryDB's first engine version on.
        ("MinimumEngineVersion", json!("6.2")),
        (
            "ARN",
            json!(format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:user/{DEFAULT_USER}")),
        ),
    ] {
        assert_eq!(
            user[member], expected,
            "case {case:?}: expected {member} to be {expected}, got {user}"
        );
    }
    assert!(
        first.get("NextToken").is_none(),
        "case {case:?}: a listing that fits in one page has no token, got {first}"
    );

    let second = harness.ok("DescribeUsers", &json!({}), case).await;
    assert_eq!(
        first, second,
        "case {case:?}: a describe must not change what the next caller sees"
    );
}

/// The listing is the STORE: every user it holds is reported in name order with the
/// members that were persisted, and `UserName` narrows it to exactly one of them.
#[tokio::test]
async fn every_stored_user_is_reported_with_the_members_state_holds() {
    let harness = harness();
    let rows = seeded_users();
    for row in &rows {
        harness.seed_user(row);
    }

    // The table spans the whole `AuthenticationType` catalog the model declares, so no
    // authentication type goes unexercised.
    for value in AUTHENTICATION_TYPE_VALUES {
        assert!(
            rows.iter().any(|row| row.authentication_type == *value),
            "the table must cover AuthenticationType {value:?}"
        );
    }
    // The same for the statuses `User$Status` documents.
    for value in USER_STATUS_VALUES {
        assert!(
            rows.iter().any(|row| row.status == *value),
            "the table must cover User$Status {value:?}"
        );
    }

    // Name order, with the built-in user among them: `BTreeMap` order is byte order,
    // so capitalised names sort before lower-case ones.
    let mut expected: Vec<String> = rows
        .iter()
        .map(|row| row.name.to_string())
        .chain([DEFAULT_USER.to_string()])
        .collect();
    expected.sort();
    assert_eq!(
        harness.user_names(&json!({}), "full listing").await,
        expected,
        "case {:?}: the listing must be every stored user, in name order",
        "full listing"
    );

    for row in &rows {
        let case = format!("UserName={}", row.name);
        let body = harness
            .ok("DescribeUsers", &json!({ "UserName": row.name }), &case)
            .await;
        assert_eq!(
            body["Users"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: a named lookup reports exactly that user, got {body}"
        );
        let user = &body["Users"][0];
        let mut authentication = json!({ "Type": row.authentication_type });
        if let Some(count) = row.password_count {
            authentication["PasswordCount"] = json!(count);
        }
        for (member, expected) in [
            ("Name", json!(row.name)),
            ("Status", json!(row.status)),
            ("AccessString", json!(row.access_string)),
            ("Authentication", authentication),
            // DERIVED from how the user authenticates: IAM authentication needs 7.0,
            // a password or no password works from 6.2 on.
            (
                "MinimumEngineVersion",
                json!(if row.authentication_type == "iam" {
                    "7.0"
                } else {
                    "6.2"
                }),
            ),
            (
                "ARN",
                json!(format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:user/{}", row.name)),
            ),
        ] {
            assert_eq!(
                user[member], expected,
                "case {case:?}: expected {member} to be {expected}, got {user}"
            );
        }
    }
}

/// `User$ACLNames` is DERIVED from the ACLs whose `UserNames` list the user, so it
/// follows every ACL created afterwards and never reports one that does not list it.
#[tokio::test]
async fn acl_names_are_derived_from_the_acls_that_list_the_user() {
    let harness = harness();
    for row in &seeded_users() {
        harness.seed_user(row);
    }

    // Before any ACL of this test's own exists, only the built-in ACL names a user.
    for name in ["a", "app"] {
        let body = harness
            .ok("DescribeUsers", &json!({ "UserName": name }), "no acls")
            .await;
        assert_eq!(
            body["Users"][0]["ACLNames"],
            json!([]),
            "case {:?}: {name} belongs to no ACL yet, got {body}",
            "no acls"
        );
    }
    let body = harness
        .ok(
            "DescribeUsers",
            &json!({ "UserName": DEFAULT_USER }),
            "no acls",
        )
        .await;
    assert_eq!(
        body["Users"][0]["ACLNames"],
        json!([OPEN_ACCESS_ACL]),
        "case {:?}: the built-in user belongs to the built-in ACL, got {body}",
        "no acls"
    );

    // One row per ACL: the users it lists, in the order the request named them.
    let acls: &[(&str, &[&str])] = &[
        ("shared-acl", &["a", "app"]),
        ("solo-acl", &["app"]),
        ("default-acl", &[DEFAULT_USER]),
    ];
    for (acl_name, user_names) in acls {
        harness
            .ok(
                "CreateACL",
                &json!({ "ACLName": acl_name, "UserNames": user_names }),
                acl_name,
            )
            .await;
    }

    // One row per user: the ACLs listing it, in name order.
    let expected: &[(&str, &[&str])] = &[
        ("a", &["shared-acl"]),
        ("app", &["shared-acl", "solo-acl"]),
        ("data-reader-1", &[]),
        (DEFAULT_USER, &["default-acl", OPEN_ACCESS_ACL]),
    ];
    for (name, acl_names) in expected {
        let case = format!("ACLNames of {name}");
        let body = harness
            .ok("DescribeUsers", &json!({ "UserName": name }), &case)
            .await;
        assert_eq!(
            body["Users"][0]["ACLNames"],
            json!(acl_names),
            "case {case:?}: expected the ACLs listing it in name order, got {body}"
        );
    }
}

/// `Filters` selects the users whose named property carries one of the entry's values,
/// several filters combine conjunctively, and a `UserName` given alongside them
/// narrows the same way.
#[tokio::test]
async fn filters_select_the_users_whose_named_property_matches() {
    let harness = harness();
    for row in &seeded_users() {
        harness.seed_user(row);
    }
    harness
        .ok(
            "CreateACL",
            &json!({ "ACLName": "reader-acl", "UserNames": ["a", "data-reader-1"] }),
            "setup",
        )
        .await;

    // One row per filter shape: the request, and the names it must select. The filter
    // names are MemoryDB's own hyphenated, lower-case spellings of the properties, and
    // the values are deliberately written in mixed case, which a filter folds like any
    // other reference to a resource name.
    let cases: &[(&str, Value, &[&str])] = &[
        (
            "one user-name value",
            json!({ "Filters": [{ "Name": "user-name", "Values": ["App"] }] }),
            &["app"],
        ),
        (
            "several user-name values",
            json!({ "Filters": [{ "Name": "user-name", "Values": ["App", "a", "nobody"] }] }),
            &["a", "app"],
        ),
        (
            "an acl-name value",
            json!({ "Filters": [{ "Name": "acl-name", "Values": ["Reader-ACL"] }] }),
            &["a", "data-reader-1"],
        ),
        (
            "an acl-name nothing belongs to",
            json!({ "Filters": [{ "Name": "acl-name", "Values": ["empty-acl"] }] }),
            &[],
        ),
        (
            "two filters are conjunctive",
            json!({
                "Filters": [
                    { "Name": "acl-name", "Values": ["reader-acl"] },
                    { "Name": "user-name", "Values": ["data-reader-1", "App"] },
                ],
            }),
            &["data-reader-1"],
        ),
        (
            "UserName beside a filter it satisfies",
            json!({
                "UserName": "a",
                "Filters": [{ "Name": "acl-name", "Values": ["reader-acl"] }],
            }),
            &["a"],
        ),
        (
            "UserName beside a filter it fails",
            json!({
                "UserName": "App",
                "Filters": [{ "Name": "acl-name", "Values": ["reader-acl"] }],
            }),
            &[],
        ),
    ];
    for (case, request, expected) in cases {
        let names = harness.user_names(request, case).await;
        assert_eq!(
            names, *expected,
            "case {case:?}: expected the filtered names in name order, got {names:?}"
        );
    }
}

/// The listing pages over its whole set, in name order, for any `MaxResults`: each
/// page is at most `MaxResults` long, the token resumes exactly where the page ended,
/// and the pages concatenated are the full set with nothing repeated or dropped.
#[tokio::test]
async fn users_page_over_the_whole_set_for_any_page_size() {
    let harness = harness();
    let seeded: Vec<Seeded> = (0..7)
        .map(|index| Seeded {
            name: match index {
                0 => "page-a",
                1 => "page-b",
                2 => "page-c",
                3 => "page-d",
                4 => "page-e",
                5 => "page-f",
                _ => "page-g",
            },
            status: "active",
            access_string: "on ~* +@read",
            authentication_type: "iam",
            password_count: None,
        })
        .collect();
    for row in &seeded {
        harness.seed_user(row);
    }
    let mut all: Vec<String> = seeded
        .iter()
        .map(|row| row.name.to_string())
        .chain([DEFAULT_USER.to_string()])
        .collect();
    all.sort();

    for page_size in 1..=all.len() + 1 {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let body = harness.ok("DescribeUsers", &request, &case).await;
            let page: Vec<String> = body["Users"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: Users is a list, got {body}"))
                .iter()
                .map(|user| user["Name"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page is at most MaxResults long, got {page:?}"
            );
            seen.extend(page);
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: paging must yield every user once, in name order"
        );
    }
}

/// The filter NAME vocabulary: `Filters` is keyed by MemoryDB's own hyphenated,
/// lower-case spelling of the property being filtered, and any other spelling of it —
/// including the model doc's `UserName` example, the ACL property's `ACLName` form, an
/// underscored or upper-cased variant, and one carrying stray whitespace — names no
/// filterable property and is refused with the modelled `InvalidParameterValueException`.
#[tokio::test]
async fn only_the_services_own_filter_names_are_accepted() {
    let harness = harness();
    for row in &seeded_users() {
        harness.seed_user(row);
    }
    harness
        .ok(
            "CreateACL",
            &json!({ "ACLName": "vocabulary-acl", "UserNames": ["a"] }),
            "setup",
        )
        .await;

    // (filter name, the value it is given, whether the service accepts the NAME). The
    // accepted rows are the whole vocabulary; the refused ones differ from them along
    // letter case, the separator, whitespace and the property named.
    let cases: &[(&str, &str, bool)] = &[
        ("user-name", "a", true),
        ("acl-name", "vocabulary-acl", true),
        // The model's doc example spelling is prose, not the wire vocabulary.
        ("UserName", "a", false),
        ("ACLName", "vocabulary-acl", false),
        ("username", "a", false),
        ("User-Name", "a", false),
        ("USER-NAME", "a", false),
        ("user_name", "a", false),
        ("acl_name", "vocabulary-acl", false),
        ("user-name ", "a", false),
        // A property `User` does not expose at all.
        ("access-string", "on ~* +@all", false),
        ("engine", "valkey", false),
        ("name", "a", false),
    ];
    for (name, value, accepted) in cases {
        let case = format!("Filters=[{{Name:{name:?}}}]");
        let request = json!({ "Filters": [{ "Name": name, "Values": [value] }] });
        let (status, body) = harness.call("DescribeUsers", &request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the filter name to be accepted, got {status} \
                 with {body}"
            );
            assert_eq!(
                body["Users"].as_array().map(Vec::len),
                Some(1),
                "case {case:?}: expected the filter to select the one matching user, got \
                 {body}"
            );
            assert_eq!(
                body["Users"][0]["Name"],
                json!("a"),
                "case {case:?}: expected the filter to select user \"a\", got {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected 400 InvalidParameterValueException, got {status} \
                 with {body}"
            );
        }
    }
}

/// The refusals, one rule per row: a `UserName` state does not hold is the modelled
/// not-found fault, a filter whose `Name` is not a filterable property is the modelled
/// invalid-value error, and a page size or token that names nothing is a 400.
#[tokio::test]
async fn refusals_come_from_state_and_from_the_filterable_properties() {
    let harness = harness();
    for row in &seeded_users() {
        harness.seed_user(row);
    }

    let cases: &[(Value, u16, &str)] = &[
        // Absent names, in each form a folded `UserName` takes.
        (json!({ "UserName": "nobody" }), 400, "UserNotFoundFault"),
        (json!({ "UserName": "Z" }), 400, "UserNotFoundFault"),
        (
            json!({ "UserName": "data-reader-2" }),
            400,
            "UserNotFoundFault",
        ),
        // A name that is not one of the service's filterable properties — including the
        // model doc's own `UserName` spelling, which is not the wire vocabulary.
        (
            json!({ "Filters": [{ "Name": "Engine", "Values": ["valkey"] }] }),
            400,
            "InvalidParameterValueException",
        ),
        (
            json!({ "Filters": [{ "Name": "AccessString", "Values": ["on ~* +@all"] }] }),
            400,
            "InvalidParameterValueException",
        ),
        (
            json!({ "Filters": [{ "Name": "UserName", "Values": ["a"] }] }),
            400,
            "InvalidParameterValueException",
        ),
        (
            json!({ "Filters": [{ "Name": "ACLName", "Values": ["reader-acl"] }] }),
            400,
            "InvalidParameterValueException",
        ),
        (
            json!({ "Filters": [{ "Name": "username", "Values": ["a"] }] }),
            400,
            "InvalidParameterValueException",
        ),
        // One good filter does not excuse a bad one.
        (
            json!({
                "Filters": [
                    { "Name": "user-name", "Values": ["a"] },
                    { "Name": "Nope", "Values": ["a"] },
                ],
            }),
            400,
            "InvalidParameterValueException",
        ),
        // The page size and token rules the other listings in this slice follow.
        (
            json!({ "MaxResults": 0 }),
            400,
            "InvalidParameterValueException",
        ),
        (
            json!({ "MaxResults": -3 }),
            400,
            "InvalidParameterValueException",
        ),
        (
            json!({ "MaxResults": 51 }),
            400,
            "InvalidParameterValueException",
        ),
        (
            json!({ "NextToken": "zzz-no-such-position" }),
            400,
            "InvalidParameterValueException",
        ),
    ];
    for (request, expected_status, expected_code) in cases {
        let case = format!("DescribeUsers {request}");
        let (status, body) = harness.call("DescribeUsers", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (*expected_status, *expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} with {body}"
        );
    }
}
