// SPDX-License-Identifier: CC0-1.0

//! How Directory Service Data refuses a request whose member falls outside a modelled
//! constraint, end to end over HTTP.
//!
//! The rule under test: a request refused before any handler runs is answered in this
//! service's own modelled `ValidationException` — HTTP 400, error type
//! `ValidationException`, and the prose in the member the model names, `Message`
//! (`smithy_query(query="shape", shape="ValidationException")` — members
//! `Message`/`Reason`) — and that prose names the rejected value, the member path and
//! the constraint it failed. The table below walks the constraint classes the model
//! declares (`@pattern`, `@range`, `@length` on a string and on a list, an enum's
//! value set, `@required`, and a constraint on a list's MEMBER) across seven
//! operations; the recorded `DescribeUser` case is one row of it.
//!
//! The sibling suite `validation_message_rendering` reads the same `Message` for the
//! three renderings that are this service's own rather than the framework's, over the
//! sets those rules generalise across; the harness below is shared with it.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A `DirectoryId` satisfying `^d-[0-9a-f]{10}$`.
pub(super) const DIRECTORY: &str = "d-1234567890";

/// A second one, for the suites that are about one directory not answering for another.
/// Both are directories this account holds; the gate's own case is an id held by nobody.
pub(super) const OTHER_DIRECTORY: &str = "d-00abcdef99";

/// A third, for the suites whose paging cases need a directory of their own, and a fourth
/// nothing is ever written to — held by the account all the same, so what it reports is
/// "this directory holds no objects" and not the gate's refusal.
pub(super) const PAGED_DIRECTORY: &str = "d-00000000aa";
pub(super) const UNUSED_DIRECTORY: &str = "d-aaaaaaaaaa";

/// The error code every refusal below carries: the shape
/// `com.amazonaws.directoryservicedata#ValidationException`, `@httpError` 400.
pub(super) const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `DirectoryId`'s `@pattern`, as the refusal states it: the model declares
/// `^d-[0-9a-f]{10}$` (`smithy_query(query="shape", shape="DirectoryId")`) and the
/// anchors bracketing it are not part of the stated expression.
const DIRECTORY_ID_PATTERN: &str = "d-[0-9a-f]{10}";

/// `GroupName`'s `@pattern`, stated the same way; model: `^[^:;|=+"*?<>/\\,\[\]@]+$`
/// (`smithy_query(query="shape", shape="GroupName")`).
const GROUP_NAME_PATTERN: &str = r#"[^:;|=+"*?<>/\\,\[\]@]+"#;

/// `Realm`'s `@pattern`; model: `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$`
/// (`smithy_query(query="shape", shape="Realm")`).
const REALM_PATTERN: &str = r"([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?";

/// `LdapDisplayName`'s `@pattern` — the constraint on the MEMBER of
/// `LdapDisplayNameList`; model: `^[A-Za-z*][A-Za-z-*]*$`
/// (`smithy_query(query="shape", shape="LdapDisplayName")`).
const LDAP_DISPLAY_NAME_PATTERN: &str = "[A-Za-z*][A-Za-z-*]*";

/// `LdapDisplayName`'s `@length` bounds — the constraint on the MEMBER of
/// `LdapDisplayNameList` (`smithy_query(query="shape", shape="LdapDisplayName")`).
pub(super) const LDAP_DISPLAY_NAME_FLOOR: usize = 1;
pub(super) const LDAP_DISPLAY_NAME_CEILING: usize = 63;

/// `LdapDisplayName`'s WHOLE declared constraint set, as a clause stated at the list
/// those names are elements of nests it: the `@length` maximum, its minimum, then the
/// `@pattern` (`smithy_query(query="shape", shape="LdapDisplayNameList")` — member
/// `LdapDisplayName`, len 1..63, pattern `^[A-Za-z*][A-Za-z-*]*$`).
pub(super) fn search_attribute_element_set() -> String {
    format!(
        "Member must satisfy constraint: [Member must have length less than or equal to 63, \
         Member must have length greater than or equal to 1, \
         Member must satisfy regular expression pattern: {LDAP_DISPLAY_NAME_PATTERN}]"
    )
}

/// `MaxResults`' `@range` (`smithy_query(query="shape", shape="MaxResults")`).
const MAX_RESULTS_FLOOR: i64 = 1;
const MAX_RESULTS_CEILING: i64 = 250;

/// `LdapDisplayNameList`'s own `@length`
/// (`smithy_query(query="shape", shape="LdapDisplayNameList")`).
pub(super) const SEARCH_ATTRIBUTES_FLOOR: usize = 1;
pub(super) const SEARCH_ATTRIBUTES_CEILING: usize = 25;

/// `SearchString`'s `@length` maximum, from `SearchUsers`' input
/// (`smithy_query(query="operation", operation="SearchUsers")` — len 1..64).
const SEARCH_STRING_CEILING: usize = 64;

/// A SigV4 credential scope naming this service: `ds-data` is its `sigv4Name`, and the
/// route key is what claims the request for this service.
const SIGNED_FOR_DS_DATA: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ds-data/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// One emulator: a router carrying only this service, over one shared engine.
///
/// The engine's shared AD directory inventory holds the directories these suites address
/// before any request is served: every operation of this data plane is gated on the account
/// HOLDING the directory a request names ([`hold_directories`]).
pub(super) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    hold_directories(
        &engine,
        &[DIRECTORY, OTHER_DIRECTORY, PAGED_DIRECTORY, UNUSED_DIRECTORY],
    );
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Record that the account holds `directories` with their DS Data feature ON, as the AWS
/// Directory Service CONTROL plane records a directory it creates — each id with the domain
/// name that create was given — and then `EnableDirectoryDataAccess` turns the feature on
/// for.
///
/// The data plane under test creates no directory — `CreateDirectory` / `CreateMicrosoftAD`
/// belong to the control plane, as does the switch that makes this API usable at all — so a
/// suite addressing a directory this API answers for puts it in the one shared inventory the
/// gate reads
/// (`service_com_amazonaws_directoryservicedata_directoryservicedata::state::ad_directory_inventory`),
/// which is exactly what a create plus an enable would have left there. An id NOT registered
/// here is an id the account does not hold, and a registered directory with the feature off
/// is the data-access gate's own case; both are suites of their own.
///
/// The registration names no DOMAIN, so these suites read the realm this plane falls back to
/// when the control plane recorded none — which is the realm every case here asserts. The
/// suite that is about the DERIVATION of a realm registers a domain of its own
/// ([`hold_directory_in_domain`]).
pub(super) fn hold_directories(engine: &engine::InMemoryEngine, directories: &[&str]) {
    let inventory = service_com_amazonaws_directoryservicedata_directoryservicedata::state::
        ad_directory_inventory(engine)
        .expect("the shared engine holds the AD directory inventory");
    let mut held = inventory.lock();
    let now = SystemTime::now();
    for directory in directories {
        held.register(directory);
        // In service from this instant on: the enable's own transition is the control
        // plane's, and these suites are about what a served directory answers.
        held.enable_data_access(directory, now, now);
    }
}

/// [`hold_directories`] for ONE directory, with the two names the control plane created it
/// with: the `Name` its create was called with, which this plane reports as the realm of
/// every object inside it, and the NetBIOS `ShortName` of the same create, which names the
/// organizational unit the objects inside it are placed in. `None` for a create that
/// recorded no short name.
pub(super) fn hold_directory_in_domain(
    engine: &engine::InMemoryEngine,
    directory: &str,
    domain_name: &str,
) {
    hold_directory_named(engine, directory, domain_name, None);
}

/// [`hold_directory_in_domain`], naming the directory's NetBIOS short name too.
pub(super) fn hold_directory_named(
    engine: &engine::InMemoryEngine,
    directory: &str,
    domain_name: &str,
    short_name: Option<&str>,
) {
    let inventory = service_com_amazonaws_directoryservicedata_directoryservicedata::state::
        ad_directory_inventory(engine)
        .expect("the shared engine holds the AD directory inventory");
    let mut held = inventory.lock();
    held.register_in_domain(directory, domain_name, short_name);
    let now = SystemTime::now();
    held.enable_data_access(directory, now, now);
}

/// The `DC=` form of a domain name: `corp.example.com` → `DC=corp,DC=example,DC=com`.
///
/// Derived from the domain name an answer reports, so what a case pins is the derivation
/// rule — a distinguished name names its object inside its own domain — and not whichever
/// domain name the directory under test carries. A name may be written with the root's
/// trailing dot, which names no label of its own.
pub(super) fn domain_components(domain: &str) -> String {
    domain
        .trim_end_matches('.')
        .split('.')
        .map(|label| format!("DC={label}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// What the emulator answered: the status, the `x-amzn-errortype` header and the body.
pub(super) struct Answer {
    pub(super) status: u16,
    pub(super) error_type: String,
    pub(super) body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub(super) fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `Message` member of the answer, or `None` when the body carries none.
    pub(super) fn message(&self) -> Option<&str> {
        self.body.get("Message").and_then(Value::as_str)
    }
}

/// Every operation of this service is `POST <route>` with a JSON body; `DirectoryId`
/// rides in the query string (`@httpQuery`).
pub(super) async fn call(
    router: &MultiServiceRouter,
    route: &str,
    directory: &str,
    body: &Value,
) -> Answer {
    let uri = format!("{route}?DirectoryId={directory}");
    let request = ::http::Request::builder()
        .method("POST")
        .uri(&uri)
        .header("host", "ds-data.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_DS_DATA)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
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
    Answer {
        status,
        error_type,
        body,
    }
}

/// The envelope a single-violation refusal states, built the way the service builds it.
pub(super) fn one_violation(value: &str, member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value {value} at '{member}' \
         failed to satisfy constraint: {requirement}"
    )
}

/// The same envelope for a member whose value the refusal WITHHOLDS: the value half of the
/// clause is gone, and the member path and the requirement are still stated.
///
/// That is what the model's `@sensitive` decides (`SearchUsersRequest$SearchString`,
/// `CreateUserRequest$GivenName` / `$Surname` / `$EmailAddress` and every other member
/// carrying the trait); a member without it is stated through [`one_violation`].
pub(super) fn one_withheld_violation(member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' \
         failed to satisfy constraint: {requirement}"
    )
}

/// One row of the table: a request with exactly one member outside its constraint, and
/// the whole `Message` the refusal must carry.
struct Case {
    what: &'static str,
    route: &'static str,
    directory: String,
    body: Value,
    message: String,
}

/// The table: one row per constraint class the model declares for this service, spread
/// over the operations that declare them.
fn cases() -> Vec<Case> {
    let longest_search_string = "x".repeat(SEARCH_STRING_CEILING + 1);
    // Attribute names inside `LdapDisplayName`'s pattern and length, so only the list's
    // own `@length` is violated.
    let over_long_attributes: Vec<String> = ('a'..='z')
        .take(SEARCH_ATTRIBUTES_CEILING + 1)
        .map(|letter| letter.to_string())
        .collect();

    vec![
        Case {
            what: "a DirectoryId outside its @pattern",
            route: "/Users/DescribeUser",
            directory: "not-a-directory-id".to_string(),
            body: json!({"SAMAccountName": "jdoe"}),
            message: one_violation(
                "'not-a-directory-id'",
                "directoryId",
                &format!("Member must satisfy regular expression pattern: {DIRECTORY_ID_PATTERN}"),
            ),
        },
        Case {
            what: "a group SAMAccountName outside its @pattern",
            route: "/Groups/DescribeGroup",
            directory: DIRECTORY.to_string(),
            body: json!({"SAMAccountName": "sim2real;group"}),
            message: one_violation(
                "'sim2real;group'",
                "sAMAccountName",
                &format!("Member must satisfy regular expression pattern: {GROUP_NAME_PATTERN}"),
            ),
        },
        Case {
            what: "a Realm outside its @pattern",
            route: "/Groups/ListGroups",
            directory: DIRECTORY.to_string(),
            body: json!({"Realm": "not a realm!"}),
            message: one_violation(
                "'not a realm!'",
                "realm",
                &format!("Member must satisfy regular expression pattern: {REALM_PATTERN}"),
            ),
        },
        Case {
            what: "a MaxResults one above its @range ceiling",
            route: "/Users/ListUsers",
            directory: DIRECTORY.to_string(),
            body: json!({"MaxResults": MAX_RESULTS_CEILING + 1}),
            message: one_violation(
                &format!("'{}'", MAX_RESULTS_CEILING + 1),
                "maxResults",
                &format!("Member must have value less than or equal to {MAX_RESULTS_CEILING}"),
            ),
        },
        Case {
            what: "a MaxResults one below its @range floor",
            route: "/Groups/ListGroups",
            directory: DIRECTORY.to_string(),
            body: json!({"MaxResults": MAX_RESULTS_FLOOR - 1}),
            message: one_violation(
                &format!("'{}'", MAX_RESULTS_FLOOR - 1),
                "maxResults",
                &format!("Member must have value greater than or equal to {MAX_RESULTS_FLOOR}"),
            ),
        },
        Case {
            what: "a GroupType outside the enum's value set",
            route: "/Groups/CreateGroup",
            directory: DIRECTORY.to_string(),
            body: json!({"SAMAccountName": "engineering", "GroupType": "NotAGroupType"}),
            message: one_violation(
                "'NotAGroupType'",
                "groupType",
                // `GroupType`'s modelled values, in the order this service publishes
                // the set in (`crate::service::validation::GROUP_TYPE_VALUE_SET`),
                // which is not the order the model declares them in.
                "Member must satisfy enum value set: [Security, Distribution]",
            ),
        },
        Case {
            what: "a GroupScope outside the enum's value set",
            route: "/Groups/CreateGroup",
            directory: DIRECTORY.to_string(),
            body: json!({"SAMAccountName": "engineering", "GroupScope": "Planetary"}),
            message: one_violation(
                "'Planetary'",
                "groupScope",
                // `GroupScope`'s modelled values, in model order
                // (`smithy_query(query="shape", shape="GroupScope")`).
                "Member must satisfy enum value set: [DomainLocal, Global, Universal, BuiltinLocal]",
            ),
        },
        Case {
            what: "a SearchAttributes element outside the list MEMBER's @pattern",
            route: "/Users/SearchUsers",
            directory: DIRECTORY.to_string(),
            body: json!({"SearchString": "j", "SearchAttributes": ["1BadAttribute"]}),
            message: one_violation(
                // The list, not the element: the clause is stated at the member the
                // request carries, quoting the whole list it held.
                "'[1BadAttribute]'",
                "searchAttributes",
                &search_attribute_element_set(),
            ),
        },
        Case {
            what: "a SearchAttributes list one above its @length ceiling",
            route: "/Groups/SearchGroups",
            directory: DIRECTORY.to_string(),
            body: json!({"SearchString": "e", "SearchAttributes": over_long_attributes.clone()}),
            message: one_violation(
                // The value is the list the request carried, every element of it.
                &format!("'[{}]'", over_long_attributes.join(", ")),
                "searchAttributes",
                &format!(
                    "Member must have length less than or equal to {SEARCH_ATTRIBUTES_CEILING}"
                ),
            ),
        },
        Case {
            what: "a SearchAttributes list one below its @length floor",
            route: "/Users/SearchUsers",
            directory: DIRECTORY.to_string(),
            body: json!({"SearchString": "j", "SearchAttributes": []}),
            message: one_violation(
                // An empty list renders as an empty list, not as a count of none.
                "'[]'",
                "searchAttributes",
                &format!(
                    "Member must have length greater than or equal to {SEARCH_ATTRIBUTES_FLOOR}"
                ),
            ),
        },
        Case {
            what: "a SearchString one character over its @length ceiling",
            route: "/Users/SearchUsers",
            directory: DIRECTORY.to_string(),
            body: json!({
                "SearchString": longest_search_string.clone(),
                "SearchAttributes": ["cn"],
            }),
            // `SearchString` is `@sensitive`, so the clause states no value at all.
            message: one_withheld_violation(
                "searchString",
                &format!("Member must have length less than or equal to {SEARCH_STRING_CEILING}"),
            ),
        },
        Case {
            what: "a @required member absent altogether",
            route: "/Users/CreateUser",
            directory: DIRECTORY.to_string(),
            body: json!({"GivenName": "J"}),
            // An absent member has no value to echo, so the clause says `null`.
            message: one_violation("null", "sAMAccountName", "Member must not be null"),
        },
    ]
}

/// Every member-constraint refusal answers 400 `ValidationException` with a `Message`
/// naming the value, the member and the constraint — in the member the model names.
#[tokio::test]
async fn every_member_constraint_refusal_states_its_reason_in_message() {
    let router = emulator();

    for case in cases() {
        let what = case.what;
        let refused = call(&router, case.route, &case.directory, &case.body).await;

        assert_eq!(
            refused.status, 400,
            "case {what:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            VALIDATION_EXCEPTION,
            "case {what:?}: expected error type {VALIDATION_EXCEPTION}, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.error_type, VALIDATION_EXCEPTION,
            "case {what:?}: expected x-amzn-errortype {VALIDATION_EXCEPTION}, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.message(),
            Some(case.message.as_str()),
            "case {what:?}: expected Message {:?}, got {:?}",
            case.message,
            refused.body
        );
        // The model names this member `Message`: a body carrying the prose anywhere
        // else leaves an SDK's `ValidationException` with an empty reason.
        assert!(
            refused.body.get("message").is_none(),
            "case {what:?}: the prose must ride in Message alone, got {:?}",
            refused.body
        );
        // `Reason` names a rule beyond the modelled constraints; a constraint refusal
        // has none, and an absent member is not a null one.
        assert!(
            refused.body.get("Reason").is_none(),
            "case {what:?}: a constraint refusal states no Reason, got {:?}",
            refused.body
        );
    }
}

/// A value INSIDE every constraint is served, so the refusal above is the constraint's
/// doing and not the envelope's: each published bound of `MaxResults` is accepted and
/// carries no error member at all.
#[tokio::test]
async fn a_value_inside_every_constraint_is_served() {
    let router = emulator();

    for (what, max_results) in [
        ("the @range floor", MAX_RESULTS_FLOOR),
        ("the @range ceiling", MAX_RESULTS_CEILING),
        ("a page size between the bounds", MAX_RESULTS_CEILING / 2),
    ] {
        let served = call(
            &router,
            "/Users/ListUsers",
            DIRECTORY,
            &json!({"MaxResults": max_results}),
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            served.status, served.body
        );
        assert!(
            served.message().is_none() && served.body.get("__type").is_none(),
            "case {what:?}: a served request carries no error member, got {:?}",
            served.body
        );
    }
}

/// A refusal a HANDLER states keeps its own envelope: the modelled `ValidationException`
/// carries both `Message` and the `Reason` the handler chose, so re-rendering a
/// framework refusal has not flattened the modelled one.
#[tokio::test]
async fn a_handler_stated_validation_exception_keeps_its_reason() {
    let router = emulator();

    let refused = call(
        &router,
        "/Users/ListUsers",
        DIRECTORY,
        &json!({"NextToken": "not-a-token-this-listing-issued"}),
    )
    .await;

    assert_eq!(
        refused.status, 400,
        "a token this listing did not issue must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        VALIDATION_EXCEPTION,
        "expected error type {VALIDATION_EXCEPTION}, got {:?}",
        refused.body
    );
    assert_eq!(
        refused.body.get("Reason").and_then(Value::as_str),
        Some("INVALID_NEXT_TOKEN"),
        "the handler's Reason must survive, got {:?}",
        refused.body
    );
    assert!(
        refused.message().is_some_and(|m| !m.is_empty()),
        "the handler's Message must survive, got {:?}",
        refused.body
    );
}

/// A request that names no modelled operation is not claimed by the envelope wrapper:
/// it reaches this service's own request layer, which answers the unknown-operation
/// envelope rather than any handler (the layer itself is covered in
/// `request_layer_envelopes`).
#[tokio::test]
async fn a_request_naming_no_modelled_operation_is_not_claimed() {
    let router = emulator();

    let answer = call(&router, "/Users/NoSuchOperation", DIRECTORY, &json!({})).await;
    assert_eq!(
        answer.status, 200,
        "an unmodelled route must reach the request layer, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer
            .body
            .get("Output")
            .and_then(|output| output.get("__type"))
            .and_then(Value::as_str),
        Some("com.amazon.coral.service#UnknownOperationException"),
        "an unmodelled route must answer the unknown-operation envelope, got {:?}",
        answer.body
    );
}

/// EVERY operation this service binds states its refusal the same way, so none is
/// left answering the framework's `Message`-less envelope.
///
/// One row per route in `OPERATIONS.md` (17 of them), each sending its `@required`
/// members inside their constraints and only `DirectoryId` outside its `@pattern`, so
/// every row must answer the one same clause.
#[tokio::test]
async fn every_bound_operation_states_the_same_refusal() {
    let router = emulator();

    // `@required` members per route, from `OPERATIONS.md`; every value here is inside
    // its own constraint so `DirectoryId` is the only violation.
    let group = json!({"SAMAccountName": "engineering"});
    let user = json!({"SAMAccountName": "jdoe"});
    let membership = json!({"GroupName": "engineering", "MemberName": "jdoe"});
    let member_name = json!({"SAMAccountName": "jdoe"});
    let search = json!({"SearchString": "j", "SearchAttributes": ["cn"]});
    let routes: [(&str, &Value); 17] = [
        ("/GroupMemberships/AddGroupMember", &membership),
        ("/GroupMemberships/ListGroupMembers", &group),
        ("/GroupMemberships/ListGroupsForMember", &member_name),
        ("/GroupMemberships/RemoveGroupMember", &membership),
        ("/Groups/CreateGroup", &group),
        ("/Groups/DeleteGroup", &group),
        ("/Groups/DescribeGroup", &group),
        ("/Groups/ListGroups", &Value::Null),
        ("/Groups/SearchGroups", &search),
        ("/Groups/UpdateGroup", &group),
        ("/Users/CreateUser", &user),
        ("/Users/DeleteUser", &user),
        ("/Users/DescribeUser", &user),
        ("/Users/DisableUser", &user),
        ("/Users/ListUsers", &Value::Null),
        ("/Users/SearchUsers", &search),
        ("/Users/UpdateUser", &user),
    ];
    let expected = one_violation(
        "'not-a-directory-id'",
        "directoryId",
        &format!("Member must satisfy regular expression pattern: {DIRECTORY_ID_PATTERN}"),
    );

    for (route, required) in routes {
        let body = if required.is_null() {
            json!({})
        } else {
            required.clone()
        };
        let refused = call(&router, route, "not-a-directory-id", &body).await;
        assert_eq!(
            refused.status, 400,
            "route {route:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.message(),
            Some(expected.as_str()),
            "route {route:?}: expected Message {expected:?}, got {:?}",
            refused.body
        );
    }
}
