// SPDX-License-Identifier: CC0-1.0

//! What this service's refusals SAY, end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! A refusal's error code says a value was wrong; its MESSAGE is the documented way to
//! learn WHICH rule the request broke, because one code
//! (`InvalidParameterValueException`) covers every one of these rules. The rules under
//! test, each over a SET of inputs rather than one recorded request:
//!
//! * a name refusal states the service's ONE rule for that kind of resource: the length
//!   clause names the identifier the way the service names it (`User id`, `ACL name`, …),
//!   and the spelling clause names the REQUEST MEMBER for a parameter group ("The
//!   parameter ParameterGroupName is not a valid identifier. …"), the service's own label
//!   plus that verdict for a cluster, and that label alone for the other kinds — and says
//!   the same whether the name was caught by the model's half of the rule or by the
//!   service's tighter half;
//! * a password refusal states the documented length bound, a password-count refusal
//!   states the service's maximum, and an `AuthenticationMode` whose type and
//!   `Passwords` do not go together names the type it refused — for every operation
//!   that takes an `AuthenticationMode`;
//! * an already-exists refusal is the service's template for that kind of resource,
//!   built from the FOLDED name the request named;
//! * every mutation of a resource MemoryDB itself provides is refused by naming that
//!   resource and the action refused, and leaves it byte-identical;
//! * a parameter-value refusal is built from the `AllowedValues` the service itself
//!   PUBLISHES for that parameter, in the wording of the clause the value broke, over
//!   the whole published catalog of a family rather than a chosen parameter;
//! * `DeleteACL` removes the ACL and a later read answers the modelled not-found fault.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::state::{
    DEFAULT_USER_NAME, MAX_PARAMETERS_PER_UPDATE, MAX_PASSWORDS_PER_USER, MAX_PASSWORD_LEN,
    MAX_RESOURCE_NAME_LEN, MIN_PASSWORD_LEN, OPEN_ACCESS_ACL_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};

/// Longer than the service's `SETTLE_AFTER`, so a resource reported in a transient
/// status is certainly due its terminal one by the time the test looks again. The
/// emulator stores the due instant and never sleeps; this blocks the test thread rather
/// than awaiting, because the workspace's `tokio` is declared without the `time`
/// feature.
const PAST_SETTLE: Duration = Duration::from_millis(700);

/// A password that breaks none of the documented password rules.
const PASSWORD: &str = "Sim2realPassword-1";

/// An access string every user in this suite is created with.
const ACCESS: &str = "on ~* +@all";

/// A parameter group family the engine catalog publishes.
const FAMILY: &str = "memorydb_valkey7";

/// The clauses every spelling refusal of a MemoryDB identifier ends with, whichever
/// subject the kind of resource states them about.
const IDENTIFIER_RULE_CLAUSES: &str = "must begin with a letter; must contain only ASCII \
                                       letters, digits, and hyphens; and must not end \
                                       with a hyphen or contain two consecutive hyphens.";

/// What a kind of resource's spelling refusal states the identifier rule ABOUT.
#[derive(Debug, Clone, Copy)]
enum Subject {
    /// The service's own label for the identifier, and nothing else.
    Label,
    /// That label, followed by the service's "is not a valid identifier" verdict.
    LabelAndVerdict,
    /// The request member the name arrived in, followed by that verdict.
    Member(&'static str),
}

/// The `Message` of a failure body, under either spelling of the member.
fn error_message(body: &Value, case: &str) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

/// Call an operation that must be refused, and return `(status, code, message)`.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    case: &str,
) -> (u16, String, String) {
    let (status, answered) = call(router, operation, body).await;
    assert!(
        status >= 400,
        "case {case:?}: expected {operation} to refuse the request, got {status} with \
         {answered}"
    );
    (
        status,
        error_code(&answered),
        error_message(&answered, case),
    )
}

/// `CreateUser` with `name` and one valid password.
fn create_user_body(name: &str) -> Value {
    json!({
        "UserName": name,
        "AccessString": ACCESS,
        "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
    })
}

/// The create request each name-taking operation carries `name` in.
fn create_body(operation: &str, name: &str) -> Value {
    match operation {
        "CreateUser" => create_user_body(name),
        "CreateACL" => json!({ "ACLName": name }),
        "CreateParameterGroup" => json!({ "ParameterGroupName": name, "Family": FAMILY }),
        "CreateCluster" => json!({
            "ClusterName": name,
            "NodeType": "db.t4g.small",
            "ACLName": OPEN_ACCESS_ACL_NAME,
        }),
        other => panic!("case {other:?}: not a name-taking create"),
    }
}

/// A name refusal states the service's ONE name rule for that kind of resource: the
/// length clause names the published bound, the spelling clause names every part of the
/// rule, and each names what the SERVICE names when it refuses that kind of identifier —
/// its own label, that label with the "is not a valid identifier" verdict, or, for a
/// parameter group, the request member the name arrived in — for every kind of resource
/// that takes a name, and whichever half of the rule the name broke (the model's
/// `@pattern`, refused at the boundary, or the service's tighter rule, refused by the
/// handler).
#[tokio::test]
async fn a_name_refusal_states_the_services_own_rule_for_that_resource() {
    let router = router();
    let past_max: String = std::iter::once('a')
        .chain(std::iter::repeat_n('b', MAX_RESOURCE_NAME_LEN))
        .collect();
    let at_max: String = std::iter::once('a')
        .chain(std::iter::repeat_n('b', MAX_RESOURCE_NAME_LEN - 1))
        .collect();

    // Which clause of the rule each name breaks: `true` for the length clause, `false`
    // for the spelling clause. The rows differ along every clause, and in whether the
    // model's own pattern also refuses the name (a digit, an underscore, a dot or a
    // space) or only the service's rule does (a trailing or doubled hyphen, a length).
    let names: Vec<(&str, String, bool)> = vec![
        ("one past the length bound", past_max.clone(), true),
        ("beginning with a digit", "9lives".to_string(), false),
        ("beginning with a hyphen", "-leading".to_string(), false),
        ("holding an underscore", "under_score".to_string(), false),
        ("holding a dot", "dotted.name".to_string(), false),
        ("holding a space", "two words".to_string(), false),
        ("ending with a hyphen", "trailing-".to_string(), false),
        ("two consecutive hyphens", "double--hyphen".to_string(), false),
    ];
    // One row per name-taking create, with the label the service gives that kind of
    // resource's identifier and the SUBJECT its spelling refusal states the identifier
    // rule about: the label alone, the label with the service's verdict after it, or the
    // request member the name arrived in.
    let kinds: &[(&str, &str, Subject)] = &[
        ("CreateUser", "User id", Subject::Label),
        ("CreateACL", "ACL name", Subject::Label),
        (
            "CreateParameterGroup",
            "Parameter group name",
            Subject::Member("ParameterGroupName"),
        ),
        ("CreateCluster", "Cluster name", Subject::LabelAndVerdict),
    ];

    for (create, label, subject) in kinds {
        for (name_case, name, breaks_length) in &names {
            let case = format!("{create}/{name_case}");
            let wanted = if *breaks_length {
                format!("{label} length must be less than {MAX_RESOURCE_NAME_LEN}.")
            } else {
                match subject {
                    Subject::Label => format!("{label} {IDENTIFIER_RULE_CLAUSES}"),
                    Subject::LabelAndVerdict => format!(
                        "{label} specified is not a valid identifier. Identifiers \
                         {IDENTIFIER_RULE_CLAUSES}"
                    ),
                    Subject::Member(member) => format!(
                        "The parameter {member} is not a valid identifier. Identifiers \
                         {IDENTIFIER_RULE_CLAUSES}"
                    ),
                }
            };
            let (status, code, message) =
                refused(&router, create, &create_body(create, name), &case).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected the modelled invalid-value refusal, got {status} \
                 {code}"
            );
            assert_eq!(
                message, wanted,
                "case {case:?}: expected the service's own rule, got {message:?}"
            );
        }
        // AT the bound the name is accepted, so the length clause states a bound the
        // service really enforces.
        let case = format!("{create}/at the length bound");
        ok(&router, create, &create_body(create, &at_max), &case).await;
    }
}

/// A password outside the documented length bound is refused with the service's own
/// sentence for that bound, by every operation that takes an `AuthenticationMode` —
/// and a password AT either bound is accepted, so the sentence states a bound the
/// service really enforces.
#[tokio::test]
async fn a_password_refusal_states_the_documented_length_bound() {
    let router = router();
    let wanted =
        format!("Passwords length must be between {MIN_PASSWORD_LEN}-{MAX_PASSWORD_LEN} characters.");
    // The rows span the length class on both sides of both bounds.
    let lengths: &[usize] = &[1, MIN_PASSWORD_LEN - 1, MAX_PASSWORD_LEN + 1, 200];
    // A user the update cases can act on.
    ok(
        &router,
        "CreateUser",
        &create_user_body("pw-rules"),
        "setup",
    )
    .await;

    for length in lengths {
        let password: String = std::iter::repeat_n('A', *length).collect();
        let mode = json!({ "Type": "password", "Passwords": [password] });
        let requests: &[(&str, Value)] = &[
            (
                "CreateUser",
                json!({
                    "UserName": format!("pw-{length}"),
                    "AccessString": ACCESS,
                    "AuthenticationMode": mode,
                }),
            ),
            (
                "UpdateUser",
                json!({ "UserName": "pw-rules", "AuthenticationMode": mode }),
            ),
        ];
        for (operation, body) in requests {
            let case = format!("{operation}/{length} characters");
            let (status, code, message) = refused(&router, operation, body, &case).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected the modelled invalid-value refusal, got {status} \
                 {code}"
            );
            assert_eq!(
                message, wanted,
                "case {case:?}: expected the service's password-length sentence, got \
                 {message:?}"
            );
        }
        // A refused create persisted nothing.
        let (status, answered) = call(
            &router,
            "DescribeUsers",
            &json!({ "UserName": format!("pw-{length}") }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "UserNotFoundFault"),
            "case {length:?}: a refused create must leave no user behind, got {answered}"
        );
    }

    // At each bound the password is accepted.
    for length in [MIN_PASSWORD_LEN, MAX_PASSWORD_LEN] {
        let password: String = std::iter::repeat_n('A', length).collect();
        let case = format!("at {length} characters");
        ok(
            &router,
            "CreateUser",
            &json!({
                "UserName": format!("pw-ok-{length}"),
                "AccessString": ACCESS,
                "AuthenticationMode": { "Type": "password", "Passwords": [password] },
            }),
            &case,
        )
        .await;
    }
}

/// An `AuthenticationMode` whose type and `Passwords` do not go together is refused
/// with the service's own sentence for that combination, naming the authentication type
/// the request asked for — for every operation that takes one.
#[tokio::test]
async fn an_authentication_mode_combination_refusal_names_the_type_it_refused() {
    let router = router();
    ok(
        &router,
        "CreateUser",
        &create_user_body("auth-modes"),
        "setup",
    )
    .await;

    // (case, the mode asked for, the code the service answers with, the sentence).
    //
    // An EMPTY `Passwords` list is a different row: `PasswordListInput` carries
    // `@length(min: 1)`, so the model's own half of the rule refuses it at the boundary
    // with this service's invalid-value code and the clause naming the member and the
    // bound it broke.
    let rows: &[(&str, Value, &str, Option<&str>)] = &[
        (
            "iam with one password",
            json!({ "Type": "iam", "Passwords": [PASSWORD] }),
            "InvalidParameterCombinationException",
            Some("Password field is not allowed with authentication type: iam"),
        ),
        (
            "iam with two passwords",
            json!({ "Type": "iam", "Passwords": [PASSWORD, "Sim2realPassword-2"] }),
            "InvalidParameterCombinationException",
            Some("Password field is not allowed with authentication type: iam"),
        ),
        (
            "password with no passwords",
            json!({ "Type": "password" }),
            "InvalidParameterCombinationException",
            Some(
                "A user with Authentication Mode: password, must have at least one \
                 password",
            ),
        ),
        (
            "password with an empty list",
            json!({ "Type": "password", "Passwords": [] }),
            "InvalidParameterValueException",
            None,
        ),
    ];

    for (row, mode, wanted_code, wanted_message) in rows {
        let requests: &[(&str, Value)] = &[
            (
                "CreateUser",
                json!({
                    "UserName": format!("auth-{}", row.replace(' ', "-")),
                    "AccessString": ACCESS,
                    "AuthenticationMode": mode,
                }),
            ),
            (
                "UpdateUser",
                json!({ "UserName": "auth-modes", "AuthenticationMode": mode }),
            ),
        ];
        for (operation, body) in requests {
            let case = format!("{operation}/{row}");
            let (status, code, message) = refused(&router, operation, body, &case).await;
            assert_eq!(
                (status, code.as_str()),
                (400, *wanted_code),
                "case {case:?}: expected the modelled {wanted_code}, got {status} {code}"
            );
            if let Some(wanted) = wanted_message {
                assert_eq!(
                    message, *wanted,
                    "case {case:?}: expected the service's combination sentence, got \
                     {message:?}"
                );
            } else {
                // The boundary's clause: it names the member and the bound the request
                // broke, which is what the model declares rather than a sentence this
                // service chooses.
                assert!(
                    message.contains("length"),
                    "case {case:?}: expected the clause to state the broken length \
                     bound, got {message:?}"
                );
            }
        }
    }
}

/// More passwords than a user may hold is refused with the service's own sentence for
/// that maximum, whichever operation asks for them and however many too many are asked
/// for.
#[tokio::test]
async fn a_password_count_refusal_states_the_services_maximum() {
    let router = router();
    let wanted = format!(
        "Maximum number of passwords allowed in this service is {MAX_PASSWORDS_PER_USER}."
    );
    ok(
        &router,
        "CreateUser",
        &create_user_body("pw-count"),
        "setup",
    )
    .await;

    // One row per count past the maximum.
    for count in [MAX_PASSWORDS_PER_USER + 1, MAX_PASSWORDS_PER_USER + 2, 8] {
        let passwords: Vec<String> = (0..count)
            .map(|index| format!("Sim2realPassword-{index:03}"))
            .collect();
        let mode = json!({ "Type": "password", "Passwords": passwords });
        let requests: &[(&str, Value)] = &[
            (
                "CreateUser",
                json!({
                    "UserName": format!("pw-count-{count}"),
                    "AccessString": ACCESS,
                    "AuthenticationMode": mode,
                }),
            ),
            (
                "UpdateUser",
                json!({ "UserName": "pw-count", "AuthenticationMode": mode }),
            ),
        ];
        for (operation, body) in requests {
            let case = format!("{operation}/{count} passwords");
            let (status, code, message) = refused(&router, operation, body, &case).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: expected the modelled invalid-value refusal, got {status} \
                 {code}"
            );
            assert_eq!(
                message, wanted,
                "case {case:?}: expected the service's password-quota sentence, got \
                 {message:?}"
            );
        }
    }

    // The refused updates left the user exactly as it was: one password, still active.
    let described = ok(
        &router,
        "DescribeUsers",
        &json!({ "UserName": "pw-count" }),
        "unchanged",
    )
    .await;
    assert_eq!(
        (
            described["Users"][0]["Authentication"]["PasswordCount"].clone(),
            described["Users"][0]["Status"].clone()
        ),
        (json!(1), json!("active")),
        "case {:?}: a refused count must leave the settled user alone, got {described}",
        "unchanged"
    );
}

/// Re-creating a resource that exists is refused with the service's template for THAT
/// kind of resource, built from the folded name the request named — whichever spelling
/// of the name the second create used.
#[tokio::test]
async fn an_already_exists_refusal_uses_the_template_of_its_resource_kind() {
    let router = router();
    // (create, the fault, the name as first created, the expected message) — one row per
    // kind of resource that takes a name, each with the service's own template.
    let kinds: &[(&str, &str, &str, &str)] = &[
        (
            "CreateUser",
            "UserAlreadyExistsFault",
            "dup-user",
            "User dup-user already exists.",
        ),
        (
            "CreateACL",
            "ACLAlreadyExistsFault",
            "dup-acl",
            "ACL dup-acl already exists. Please retry with another name.",
        ),
        (
            "CreateParameterGroup",
            "ParameterGroupAlreadyExistsFault",
            "dup-group",
            "Parameter Group already exists.",
        ),
        (
            "CreateCluster",
            "ClusterAlreadyExistsFault",
            "dup-cluster",
            "Cluster dup-cluster already exists.",
        ),
    ];

    for (create, fault, name, wanted) in kinds {
        ok(&router, create, &create_body(create, name), name).await;
        // A resource name is a case-insensitive identifier, so EVERY spelling collides
        // with the one resource and the refusal names the folded spelling.
        for spelling in [
            name.to_string(),
            name.to_ascii_uppercase(),
            format!("{}{}", name[..1].to_ascii_uppercase(), &name[1..]),
        ] {
            let case = format!("{create}/{spelling}");
            let (status, code, message) =
                refused(&router, create, &create_body(create, &spelling), &case).await;
            assert_eq!(
                (status, code.as_str()),
                (400, *fault),
                "case {case:?}: expected the modelled {fault}, got {status} {code}"
            );
            assert_eq!(
                message, *wanted,
                "case {case:?}: expected the service's already-exists template, got \
                 {message:?}"
            );
        }
    }
}

/// Every mutation of a resource MemoryDB itself provides is refused by naming that
/// resource and the action it refused, whichever spelling of the name the request used
/// — and the built-ins read back exactly as they did before.
#[tokio::test]
async fn a_built_in_resource_refuses_every_mutation_by_its_own_name() {
    let router = router();
    let user_before = ok(
        &router,
        "DescribeUsers",
        &json!({ "UserName": DEFAULT_USER_NAME }),
        "built-in user",
    )
    .await;
    let acl_before = ok(
        &router,
        "DescribeACLs",
        &json!({ "ACLName": OPEN_ACCESS_ACL_NAME }),
        "built-in ACL",
    )
    .await;

    // (operation, the member carrying the name, the extra members, the fault, the
    // message) — every write path over a built-in, including the creates that collide
    // with one.
    let refusals: &[(&str, &str, Value, &str, &str)] = &[
        (
            "UpdateUser",
            "UserName",
            json!({ "AccessString": "off -@all" }),
            "InvalidParameterValueException",
            "default user can't be modified.",
        ),
        (
            "DeleteUser",
            "UserName",
            json!({}),
            "InvalidParameterValueException",
            "default user can't be deleted.",
        ),
        (
            "UpdateACL",
            "ACLName",
            json!({ "UserNamesToRemove": [DEFAULT_USER_NAME] }),
            "InvalidParameterValueException",
            "open-access ACL can't be modified.",
        ),
        (
            "DeleteACL",
            "ACLName",
            json!({}),
            "InvalidParameterValueException",
            "open-access ACL can't be deleted.",
        ),
        (
            "CreateUser",
            "UserName",
            json!({
                "AccessString": ACCESS,
                "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
            }),
            "UserAlreadyExistsFault",
            "User default already exists.",
        ),
        (
            "CreateACL",
            "ACLName",
            json!({}),
            "ACLAlreadyExistsFault",
            "ACL open-access already exists. Please retry with another name.",
        ),
    ];

    for (operation, member, extra, fault, wanted) in refusals {
        let built_in = if *member == "UserName" {
            DEFAULT_USER_NAME
        } else {
            OPEN_ACCESS_ACL_NAME
        };
        // The same refusal under every spelling of the built-in's name: the name is a
        // case-insensitive identifier, so the refusal names the service's spelling of
        // it rather than the request's.
        for spelling in [
            built_in.to_string(),
            built_in.to_ascii_uppercase(),
            format!(
                "{}{}",
                built_in[..1].to_ascii_uppercase(),
                &built_in[1..]
            ),
        ] {
            let case = format!("{operation}/{spelling}");
            let mut body = extra.clone();
            body[*member] = json!(spelling);
            let (status, code, message) = refused(&router, operation, &body, &case).await;
            assert_eq!(
                (status, code.as_str()),
                (400, *fault),
                "case {case:?}: expected the modelled {fault}, got {status} {code}"
            );
            assert_eq!(
                message, *wanted,
                "case {case:?}: expected the service's built-in refusal, got {message:?}"
            );
        }
    }

    // Nothing the refusals touched: both built-ins read back exactly as before.
    assert_eq!(
        ok(
            &router,
            "DescribeUsers",
            &json!({ "UserName": DEFAULT_USER_NAME }),
            "built-in user"
        )
        .await,
        user_before,
        "case {:?}: the built-in user must be untouched by every refusal",
        "built-in user"
    );
    assert_eq!(
        ok(
            &router,
            "DescribeACLs",
            &json!({ "ACLName": OPEN_ACCESS_ACL_NAME }),
            "built-in ACL"
        )
        .await,
        acl_before,
        "case {:?}: the built-in ACL must be untouched by every refusal",
        "built-in ACL"
    );
}

/// One `UpdateParameterGroup` call carrying `pairs`.
async fn update_parameters(
    router: &MultiServiceRouter,
    group: &str,
    pairs: &[(&str, &str)],
) -> (u16, Value) {
    let body = json!({
        "ParameterGroupName": group,
        "ParameterNameValues": pairs
            .iter()
            .map(|(name, value)| json!({ "ParameterName": name, "ParameterValue": value }))
            .collect::<Vec<Value>>(),
    });
    call(router, "UpdateParameterGroup", &body).await
}

/// Every parameter one group reports, as `(name, DataType, AllowedValues, Value)`.
async fn published_parameters(
    router: &MultiServiceRouter,
    group: &str,
) -> Vec<(String, String, Option<String>, String)> {
    let body = ok(
        router,
        "DescribeParameters",
        &json!({ "ParameterGroupName": group }),
        group,
    )
    .await;
    body["Parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("case {group:?}: expected Parameters in {body}"))
        .iter()
        .map(|parameter| {
            let text = |member: &str| {
                parameter[member]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {group:?}: expected {member} in {parameter}")
                    })
                    .to_string()
            };
            (
                text("Name"),
                text("DataType"),
                parameter
                    .get("AllowedValues")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                text("Value"),
            )
        })
        .collect()
}

/// The first number an integer parameter's published `AllowedValues` names — the start
/// of its lowest range — so one BELOW it is a value that domain cannot hold, whatever
/// shape the rest of it has.
fn lowest_published_bound(allowed: &str, case: &str) -> i64 {
    allowed
        .split(',')
        .next()
        .and_then(|range| range.split('-').next())
        .and_then(|min| min.parse::<i64>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric bound in {allowed:?}"))
}

/// A parameter-value refusal is built from the `AllowedValues` the service itself
/// PUBLISHES for that parameter: the value set for a token parameter, the range for a
/// numeric one, and the value itself when it is not a whole number at all — over every
/// parameter a family publishes rather than a chosen one.
#[tokio::test]
async fn a_parameter_value_refusal_is_built_from_the_published_allowed_values() {
    let router = router();
    const GROUP: &str = "refusal-values";
    ok(
        &router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": GROUP, "Family": FAMILY }),
        GROUP,
    )
    .await;
    let published = published_parameters(&router, GROUP).await;
    assert!(
        !published.is_empty(),
        "case {GROUP:?}: the family must publish engine defaults"
    );

    for (name, data_type, allowed, value) in &published {
        let case = format!("{FAMILY}/{name}");
        match (data_type.as_str(), allowed.as_deref()) {
            ("integer", Some(allowed)) => {
                // Not a whole number at all: the refusal names the value it could not
                // read as one.
                let (status, answered) =
                    update_parameters(&router, GROUP, &[(name, "not-a-number")]).await;
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "InvalidParameterValueException"),
                    "case {case:?}: a non-integer must be refused, got {answered}"
                );
                assert_eq!(
                    error_message(&answered, &case),
                    "invalid integer value: not-a-number",
                    "case {case:?}: expected the service's integer-parse sentence"
                );
                // Outside every published range: the refusal names the value and the
                // range the service publishes for it.
                let outside = (lowest_published_bound(allowed, &case) - 1).to_string();
                let (status, answered) =
                    update_parameters(&router, GROUP, &[(name, outside.as_str())]).await;
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "InvalidParameterValueException"),
                    "case {case:?}: {outside} is outside {allowed:?}, got {answered}"
                );
                assert_eq!(
                    error_message(&answered, &case),
                    format!("value:{outside} is outside of range:{allowed}"),
                    "case {case:?}: expected the published range in the refusal"
                );
            }
            ("string", Some(allowed)) => {
                let tokens: Vec<&str> = allowed.split(',').collect();
                let outside = "not-a-published-token";
                assert!(
                    !tokens.contains(&outside),
                    "case {case:?}: the probe must be outside {allowed:?}"
                );
                let (status, answered) =
                    update_parameters(&router, GROUP, &[(name, outside)]).await;
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "InvalidParameterValueException"),
                    "case {case:?}: a token outside the set must be refused, got {answered}"
                );
                assert_eq!(
                    error_message(&answered, &case),
                    format!("invalid parameter value, allowed values are:{allowed}"),
                    "case {case:?}: expected the published value set in the refusal"
                );
            }
            // Free text: the engine parses the value, so the service publishes no set to
            // hold it to and there is no refusal to word.
            _ => {
                let (status, answered) =
                    update_parameters(&router, GROUP, &[(name, "anything")]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a parameter publishing no value set accepts any \
                     string, got {status} with {answered}"
                );
                continue;
            }
        }
        // Every refusal above left the parameter at the value it already reported.
        let after = published_parameters(&router, GROUP).await;
        let reported = after
            .iter()
            .find(|(candidate, ..)| candidate == name)
            .map(|(.., current)| current.clone())
            .unwrap_or_else(|| panic!("case {case:?}: expected {name} in {after:?}"));
        assert_eq!(
            &reported, value,
            "case {case:?}: a refused value must leave the parameter unchanged"
        );
    }
}

/// The remaining refusals of a parameter update, each in the service's own template: a
/// name the family does not carry, the same parameter named twice, more parameters than
/// one request may carry, a group MemoryDB itself provides, and a group that is not in
/// state.
#[tokio::test]
async fn the_other_parameter_update_refusals_use_the_services_templates() {
    let router = router();
    const GROUP: &str = "refusal-shapes";
    ok(
        &router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": GROUP, "Family": FAMILY }),
        GROUP,
    )
    .await;
    let before = published_parameters(&router, GROUP).await;

    // A name the family does not carry — rows differing in why it carries none.
    for name in ["no-such-param", "hash-max-ziplist-entries", ""] {
        let case = format!("unknown parameter {name:?}");
        let (status, answered) = update_parameters(&router, GROUP, &[(name, "1")]).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: an unknown parameter must be refused, got {answered}"
        );
        assert_eq!(
            error_message(&answered, &case),
            format!("Could not find parameter with name: {name}"),
            "case {case:?}: expected the service's unknown-parameter template"
        );
    }

    // The same parameter named twice in one call, with equal and with different values.
    for (case, pairs) in [
        (
            "twice with one value",
            vec![("timeout", "30"), ("timeout", "30")],
        ),
        (
            "twice with two values",
            vec![("timeout", "30"), ("timeout", "40")],
        ),
        (
            "twice around another pair",
            vec![
                ("timeout", "30"),
                ("maxmemory-policy", "allkeys-lru"),
                ("timeout", "40"),
            ],
        ),
    ] {
        let (status, answered) = update_parameters(&router, GROUP, &pairs).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: a duplicated parameter must be refused, got {answered}"
        );
        assert_eq!(
            error_message(&answered, case),
            "Duplicate parameters specified in the same call",
            "case {case:?}: expected the service's duplicate template"
        );
    }

    // More parameters than one request may carry, at one past the bound and well past
    // it. The pairs are DISTINCT names, so what is refused is the count.
    let carried: Vec<(String, String)> = before
        .iter()
        .map(|(name, ..)| (name.clone(), String::new()))
        .collect();
    assert!(
        carried.len() > MAX_PARAMETERS_PER_UPDATE + 1,
        "case {:?}: the family must publish more than {MAX_PARAMETERS_PER_UPDATE} \
         parameters, got {}",
        "over the batch bound",
        carried.len()
    );
    for count in [MAX_PARAMETERS_PER_UPDATE + 1, MAX_PARAMETERS_PER_UPDATE + 2] {
        let case = format!("{count} parameters");
        let pairs: Vec<(&str, &str)> = carried
            .iter()
            .take(count)
            .map(|(name, _)| (name.as_str(), "1"))
            .collect();
        let (status, answered) = update_parameters(&router, GROUP, &pairs).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidParameterCombinationException"),
            "case {case:?}: too many parameters must be refused, got {answered}"
        );
        assert_eq!(
            error_message(&answered, &case),
            format!(
                "Cannot modify more than {MAX_PARAMETERS_PER_UPDATE} parameters in a \
                 single request"
            ),
            "case {case:?}: expected the service's batch-bound template"
        );
    }
    // AT the bound the request is accepted, so the bound the message names is the one
    // the service enforces. Each pair carries that parameter's own current value, so
    // the accepted call changes nothing.
    let at_bound: Vec<(&str, &str)> = before
        .iter()
        .take(MAX_PARAMETERS_PER_UPDATE)
        .map(|(name, _, _, value)| (name.as_str(), value.as_str()))
        .collect();
    let (status, answered) = update_parameters(&router, GROUP, &at_bound).await;
    assert_eq!(
        status, 200,
        "case {:?}: {MAX_PARAMETERS_PER_UPDATE} parameters must be accepted, got \
         {status} with {answered}",
        "at the batch bound"
    );

    // A group MemoryDB itself provides is not the caller's to change.
    for group in ["default.memorydb-valkey7", "DEFAULT.MEMORYDB-REDIS7"] {
        let case = format!("default group {group:?}");
        let (status, answered) = update_parameters(&router, group, &[("timeout", "30")]).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: a default group must refuse the update, got {answered}"
        );
        assert_eq!(
            error_message(&answered, &case),
            "Cannot modify a default parameter group.",
            "case {case:?}: expected the service's default-group template"
        );
    }

    // A group that is not in state is answered with the template of the operation that
    // resolved it: the update reports the name it could not resolve, the read states the
    // condition alone.
    for group in ["no-such-group", "refusal-shape", "refusal-shapes-2"] {
        let case = format!("absent group {group:?}");
        let (status, answered) = update_parameters(&router, group, &[("timeout", "30")]).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ParameterGroupNotFoundFault"),
            "case {case:?}: an absent group must answer not-found, got {answered}"
        );
        assert_eq!(
            error_message(&answered, &case),
            format!("ParameterGroup not found: {group}"),
            "case {case:?}: expected the service's not-found template"
        );
        let (status, answered) = call(
            &router,
            "DescribeParameters",
            &json!({ "ParameterGroupName": group }),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ParameterGroupNotFoundFault"),
            "case {case:?}: the read must answer not-found too, got {answered}"
        );
        assert_eq!(
            error_message(&answered, &case),
            "Parameter Group not found.",
            "case {case:?}: expected the service's not-found template on the read"
        );
    }
}

/// `DeleteACL` removes the ACL from state: the answer reports it in the terminal
/// `deleting` status, a later read answers the modelled not-found fault, no user still
/// reports it in `ACLNames`, and an ACL a cluster still references, or one that has not
/// settled, is refused instead of removed.
#[tokio::test]
async fn delete_acl_removes_the_acl_and_a_later_read_answers_not_found() {
    let router = router();
    ok(
        &router,
        "CreateUser",
        &create_user_body("acl-member"),
        "setup",
    )
    .await;

    // An ACL still creating is not in a state to be deleted.
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "unsettled-acl", "UserNames": ["acl-member"] }),
        "unsettled",
    )
    .await;
    let (status, code, _) = refused(
        &router,
        "DeleteACL",
        &json!({ "ACLName": "unsettled-acl" }),
        "unsettled",
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidACLStateFault"),
        "case {:?}: an ACL that has not settled must refuse the delete",
        "unsettled"
    );

    // Each row is one spelling of the name the delete names the ACL by, and each ACL
    // starts with a different membership.
    let rows: &[(&str, &str, Vec<&str>)] = &[
        ("empty", "gone-acl", vec![]),
        ("one member", "gone-acl-2", vec!["acl-member"]),
        ("named in mixed case", "GONE-ACL-3", vec!["acl-member"]),
    ];
    for (case, name, members) in rows {
        let mut body = json!({ "ACLName": name });
        if !members.is_empty() {
            body["UserNames"] = json!(members);
        }
        ok(&router, "CreateACL", &body, case).await;
        std::thread::sleep(PAST_SETTLE);
        let folded = name.to_ascii_lowercase();

        let deleted = ok(&router, "DeleteACL", &json!({ "ACLName": name }), case).await;
        assert_eq!(
            (
                deleted["ACL"]["Name"].clone(),
                deleted["ACL"]["Status"].clone()
            ),
            (json!(folded), json!("deleting")),
            "case {case:?}: the delete must answer the stored ACL in the terminal \
             status, got {deleted}"
        );
        assert_eq!(
            deleted["ACL"]["UserNames"],
            json!(members
                .iter()
                .map(|member| member.to_ascii_lowercase())
                .collect::<Vec<String>>()),
            "case {case:?}: the delete must report the membership the ACL held, got \
             {deleted}"
        );

        // Gone from state: the read answers the modelled fault and the second delete
        // answers it too.
        for operation in ["DescribeACLs", "DeleteACL"] {
            let probe = format!("{case}/{operation}");
            let (status, code, message) =
                refused(&router, operation, &json!({ "ACLName": name }), &probe).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ACLNotFoundFault"),
                "case {probe:?}: a removed ACL must answer not-found, got {status} {code}"
            );
            assert_eq!(
                message,
                format!("ACL {folded} not found"),
                "case {probe:?}: the not-found refusal must name the folded name in the \
                 one sentence this service states a missing ACL with"
            );
        }
        // Membership is held ONCE, on the ACL, so the user no longer reports it.
        let described = ok(
            &router,
            "DescribeUsers",
            &json!({ "UserName": "acl-member" }),
            case,
        )
        .await;
        let acl_names = described["Users"][0]["ACLNames"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !acl_names.contains(&json!(folded)),
            "case {case:?}: a deleted ACL must leave no membership behind, got \
             {acl_names:?}"
        );
    }

    // An ACL a cluster still references cannot go away under it.
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "in-use-acl", "UserNames": ["acl-member"] }),
        "in use",
    )
    .await;
    std::thread::sleep(PAST_SETTLE);
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "acl-user-cluster",
            "NodeType": "db.t4g.small",
            "ACLName": "in-use-acl",
        }),
        "in use",
    )
    .await;
    let (status, code, message) = refused(
        &router,
        "DeleteACL",
        &json!({ "ACLName": "in-use-acl" }),
        "in use",
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidACLStateFault"),
        "case {:?}: an ACL a cluster references must refuse the delete, got {message:?}",
        "in use"
    );
    assert!(
        message.contains("acl-user-cluster"),
        "case {:?}: the refusal must name the cluster still holding it, got {message:?}",
        "in use"
    );
    // Refused, so the ACL is still there.
    let described = ok(
        &router,
        "DescribeACLs",
        &json!({ "ACLName": "in-use-acl" }),
        "in use",
    )
    .await;
    assert_eq!(
        described["ACLs"][0]["Name"],
        json!("in-use-acl"),
        "case {:?}: a refused delete must leave the ACL in state, got {described}",
        "in use"
    );
}
