// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request it refuses, and which requests it refuses at all.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. A `@pattern` requirement is quoted WITHOUT the whole-value anchor pair the model
//!    writes around it: `^d[a-z0-9]+$` is stated `d[a-z0-9]+`. The pair is what is
//!    dropped, so an expression that carries only one of the two anchors is quoted as
//!    the model declares it. The CHECK is unchanged — the value still has to satisfy the
//!    model's own expression.
//! 2. An enumerated type's value set is stated in the order THIS SERVICE publishes for
//!    that type, which is not the order the model declares its members in. The order
//!    belongs to the TYPE, so every member targeting one enum states it the same way in
//!    any operation; a type this service publishes no order for keeps the model's.
//! 3. A branch name is drawn from letters, digits and the symbols `/_.-`, a rule the
//!    model's `pattern ^(?s).+$` does not express: `CreateBranch` refuses a name outside
//!    that set with HTTP 400 `BadRequestException`, naming the rejected name, and stores
//!    nothing.
//! 4. `StartDeployment` names the two members a caller has to pick between when a
//!    request carries neither, in text that carries nothing from the request.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives.

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::ops::start_deployment::NO_SOURCE_MESSAGE;
use service_com_amazonaws_amplify_amplify::ops::types::{PLATFORM_VALUES, STAGE_VALUES};

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, Reply};

/// The error code the framework answers a boundary constraint refusal with, which this
/// service keeps: a client branches on the code, so rewording the message must not move
/// it.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The envelope one constraint clause goes inside, as AWS writes it.
fn one_validation_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a value outside a `@pattern` takes: the value the caller submitted, the
/// member path it sat at, and the expression it had to match.
fn pattern_clause(value: &str, member_path: &str, stated_pattern: &str) -> String {
    format!(
        "Value '{value}' at '{member_path}' failed to satisfy constraint: Member must \
         satisfy regular expression pattern: {stated_pattern}"
    )
}

/// The clause a value outside an enumerated type takes: the value the caller submitted,
/// the member path, and the set that type admits.
fn enum_clause(value: &str, member_path: &str, stated_set: &[&str]) -> String {
    format!(
        "Value '{value}' at '{member_path}' failed to satisfy constraint: Member must \
         satisfy enum value set: [{}]",
        stated_set.join(", ")
    )
}

/// Assert `reply` is the boundary constraint refusal stating `message`.
fn assert_constraint_refusal(case: &str, reply: &Reply, message: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400 for a value outside a modelled constraint, \
         got {reply:?}"
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some(VALIDATION_EXCEPTION),
        "case {case:?}: expected the error code {VALIDATION_EXCEPTION:?}, got {:?}",
        reply.error_code
    );
    assert_eq!(
        reply.message().as_deref(),
        Some(message),
        "case {case:?}: expected {message:?}, got {:?}",
        reply.message()
    );
}

/// One request whose value a `@pattern` refuses, and how the refusal has to quote that
/// expression.
struct PatternCase {
    case: &'static str,
    method: &'static str,
    /// `{app}` is replaced by the app this suite created, so a row can put a bad value
    /// in a URI label without failing another member's constraint first.
    path: &'static str,
    body: Option<Value>,
    value: &'static str,
    member_path: &'static str,
    /// What the refusal has to quote. Every row's expectation is the model's own
    /// `@pattern` with the whole-value anchor PAIR dropped, and nothing else changed.
    stated_pattern: &'static str,
}

/// The `@pattern` table: one row per shape of expression this model declares, read off
/// the members' own constraints
/// (`smithy_query(query="operation",operation="GetApp,DeleteApp,CreateBranch,StartDeployment,GetJob")`).
///
/// Rows 1-3 are `AppId` (`^d[a-z0-9]+$`), each a different way to miss it, and only one
/// of them is the value the issue cited. Rows 4-5 are other members with the same anchor
/// pair in their own expressions (`JobId` `^[0-9]+$`, `TTL` `^\d*$`), so the rule is not
/// one member's. The last row is `SourceUrl` (`^(s3|https|http)://`), which carries the
/// opening anchor and NO closing one: there is no pair to drop, so it is quoted exactly
/// as the model declares it.
fn pattern_cases() -> Vec<PatternCase> {
    vec![
        PatternCase {
            case: "an appId carrying a hyphen",
            method: "GET",
            path: "/apps/frontend-app",
            body: None,
            value: "frontend-app",
            member_path: "appId",
            stated_pattern: "d[a-z0-9]+",
        },
        PatternCase {
            case: "an appId in upper case",
            method: "GET",
            path: "/apps/DEADBEEF01",
            body: None,
            value: "DEADBEEF01",
            member_path: "appId",
            stated_pattern: "d[a-z0-9]+",
        },
        PatternCase {
            case: "an appId that does not open with d",
            method: "DELETE",
            path: "/apps/x1y2z3",
            body: None,
            value: "x1y2z3",
            member_path: "appId",
            stated_pattern: "d[a-z0-9]+",
        },
        PatternCase {
            case: "a jobId that is not decimal",
            method: "GET",
            path: "/apps/{app}/branches/main/jobs/latest",
            body: None,
            value: "latest",
            member_path: "jobId",
            stated_pattern: "[0-9]+",
        },
        PatternCase {
            case: "a ttl that is not a number of seconds",
            method: "POST",
            path: "/apps/{app}/branches",
            body: Some(json!({ "branchName": "ttl-row", "ttl": "soon" })),
            value: "soon",
            member_path: "ttl",
            stated_pattern: r"\d*",
        },
        PatternCase {
            case: "a sourceUrl whose scheme is not one of the three",
            method: "POST",
            path: "/apps/{app}/branches/main/deployments/start",
            body: Some(json!({ "sourceUrl": "ftp://example.invalid/site.zip" })),
            value: "ftp://example.invalid/site.zip",
            member_path: "sourceUrl",
            // One anchor, no pair: quoted as the model writes it.
            stated_pattern: "^(s3|https|http)://",
        },
    ]
}

/// Rule 1: a `@pattern` requirement is quoted without the model's anchor pair.
#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_its_anchor_pair() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("pattern-refusals").await;
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": "main" }))
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateBranch, got {created:?}"
    );

    for row in pattern_cases() {
        let path = row.path.replace("{app}", &app_id);
        let reply = emulator.call(row.method, &path, row.body.as_ref()).await;
        assert_constraint_refusal(
            row.case,
            &reply,
            &one_validation_error(&pattern_clause(
                row.value,
                row.member_path,
                row.stated_pattern,
            )),
        );
    }
}

/// One request whose value an enumerated type refuses, and the set the refusal has to
/// list.
struct EnumCase {
    case: &'static str,
    path: &'static str,
    body: Value,
    value: &'static str,
    member_path: &'static str,
    stated_set: &'static [&'static str],
}

/// The order this service states `Stage` in, which is NOT the order the model declares
/// its members in (`PRODUCTION, BETA, DEVELOPMENT, EXPERIMENTAL, PULL_REQUEST`,
/// `smithy_query(query="shape",shape="Stage")`).
const STATED_STAGE_SET: &[&str] = &[
    "EXPERIMENTAL",
    "BETA",
    "PULL_REQUEST",
    "PRODUCTION",
    "DEVELOPMENT",
];

/// The order this service states `Platform` in, against the model's `WEB, WEB_DYNAMIC,
/// WEB_COMPUTE` (`smithy_query(query="shape",shape="Platform")`).
const STATED_PLATFORM_SET: &[&str] = &["WEB_COMPUTE", "WEB_DYNAMIC", "WEB"];

/// The order `SourceUrlType` is stated in: this service publishes none for it, so the
/// set keeps the order the model declares (`ZIP, BUCKET_PREFIX`,
/// `smithy_query(query="shape",shape="SourceUrlType")`) — the row that shows the rule is
/// per TYPE and not "reorder every set".
const MODELLED_SOURCE_URL_TYPE_SET: &[&str] = &["ZIP", "BUCKET_PREFIX"];

/// The enum table: two members per published type, in DIFFERENT operations, so the order
/// is shown to belong to the type rather than to the member or the call; plus a type this
/// service publishes no order for.
fn enum_cases() -> Vec<EnumCase> {
    vec![
        EnumCase {
            case: "a stage in lower case, on CreateBranch",
            path: "/apps/{app}/branches",
            body: json!({ "branchName": "enum-row", "stage": "production" }),
            value: "production",
            member_path: "stage",
            stated_set: STATED_STAGE_SET,
        },
        EnumCase {
            case: "a stage nothing models, on UpdateBranch",
            path: "/apps/{app}/branches/main",
            body: json!({ "stage": "STAGING" }),
            value: "STAGING",
            member_path: "stage",
            stated_set: STATED_STAGE_SET,
        },
        EnumCase {
            case: "a platform in lower case, on CreateApp",
            path: "/apps",
            body: json!({ "name": "enum-platform-row", "platform": "web" }),
            value: "web",
            member_path: "platform",
            stated_set: STATED_PLATFORM_SET,
        },
        EnumCase {
            case: "a platform nothing models, on UpdateApp",
            path: "/apps/{app}",
            body: json!({ "platform": "WEB_STATIC" }),
            value: "WEB_STATIC",
            member_path: "platform",
            stated_set: STATED_PLATFORM_SET,
        },
        EnumCase {
            case: "a sourceUrlType nothing models, on StartDeployment",
            path: "/apps/{app}/branches/main/deployments/start",
            body: json!({ "sourceUrl": "s3://bucket/site.zip", "sourceUrlType": "TAR" }),
            value: "TAR",
            member_path: "sourceUrlType",
            stated_set: MODELLED_SOURCE_URL_TYPE_SET,
        },
    ]
}

/// Rule 2: an enumerated value set is stated in this service's published order.
#[tokio::test]
async fn an_enum_value_set_is_stated_in_this_services_order() {
    // Each published order holds EXACTLY the values the model declares for that type,
    // reordered and nothing else, so no refusal can advertise a value the service does
    // not accept.
    for (set, declared) in [
        (STATED_STAGE_SET, STAGE_VALUES),
        (STATED_PLATFORM_SET, PLATFORM_VALUES),
    ] {
        let mut stated: Vec<&str> = set.to_vec();
        stated.sort_unstable();
        let mut modelled: Vec<&str> = declared.to_vec();
        modelled.sort_unstable();
        assert_eq!(
            stated, modelled,
            "a published order must hold exactly the modelled values, got {set:?} \
             against {declared:?}"
        );
    }

    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("enum-refusals").await;
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": "main" }))
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateBranch, got {created:?}"
    );

    for row in enum_cases() {
        let path = row.path.replace("{app}", &app_id);
        let reply = emulator.call("POST", &path, Some(&row.body)).await;
        assert_constraint_refusal(
            row.case,
            &reply,
            &one_validation_error(&enum_clause(row.value, row.member_path, row.stated_set)),
        );
    }
}

/// The branch names this service ACCEPTS: letters, digits and the symbols `/_.-`, over
/// the length `BranchName` models (`length 1..255`,
/// `smithy_query(query="operation",operation="CreateBranch")`).
fn accepted_branch_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest modelled name", "x".to_string()),
        ("a slashed feature name", "feature/login-v2".to_string()),
        ("a dotted release name", "release-1.2.3".to_string()),
        ("underscored mixed case", "Feature_Login_v2".to_string()),
        ("digits only", "20260101".to_string()),
        ("every accepted symbol at once", "a/b_c.d-e".to_string()),
        ("the modelled length maximum", "b".repeat(255)),
    ]
}

/// The branch names this service REFUSES: one row per class of character outside the
/// set, none of them the name the issue cited twice.
fn refused_branch_names() -> Vec<(&'static str, String)> {
    vec![
        ("a space", "main site".to_string()),
        ("a question mark", "what?now".to_string()),
        ("a hash", "feature#42".to_string()),
        ("an at sign", "release@2026".to_string()),
        ("a colon", "ref:main".to_string()),
        ("a tab", "tabbed\tname".to_string()),
        ("a non-ASCII letter", "naïve".to_string()),
        ("a name that is only symbols AWS refuses", "+++".to_string()),
        (
            "an accepted name with one refused character at the end",
            "feature/login!".to_string(),
        ),
    ]
}

/// The sentence a branch name outside the accepted set is refused with: fixed text
/// apart from the name the request sent.
fn expected_branch_name_message(branch_name: &str) -> String {
    format!(
        "The requested branch name is not supported. Failed to create branch. The branch \
         name {branch_name} is invalid. The name can only contain letters, numbers, and \
         the symbols /_.-"
    )
}

/// Rule 3: `CreateBranch` enforces the character set a branch name is drawn from.
#[tokio::test]
async fn create_branch_refuses_a_name_outside_the_accepted_character_set() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("branch-name-rule").await;

    for (case, name) in accepted_branch_names() {
        let created = emulator
            .create_branch(&app_id, &json!({ "branchName": name }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: a name inside the accepted set must be created, got {created:?}"
        );
    }

    for (case, name) in refused_branch_names() {
        let refused = emulator
            .create_branch(&app_id, &json!({ "branchName": name }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 for {name:?}, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            refused.error_code
        );
        let expected = expected_branch_name_message(&name);
        assert_eq!(
            refused.message().as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.message()
        );
    }

    // The refused creates stored nothing: the app holds exactly the accepted names.
    let listed = emulator.list_branches(&app_id, "?maxResults=50").await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListBranches, got {listed:?}"
    );
    let mut stored: Vec<String> = list_member(&listed.body, "branches")
        .iter()
        .map(|branch| string_member(branch, "branchName"))
        .collect();
    stored.sort();
    let mut accepted: Vec<String> = accepted_branch_names()
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    accepted.sort();
    assert_eq!(
        stored, accepted,
        "a refused name must not reach the branch list, got {stored:?}"
    );
}

/// The sentence `StartDeployment` refuses a request that names neither member with, as
/// AWS words it: entirely fixed text naming the request shape and the two members, with
/// nothing from the request or the store in it.
const EXPECTED_NO_SOURCE_MESSAGE: &str = "Must specify jobId or sourceUrl for StartDeploymentRequest.";

/// Rule 4: `StartDeployment` states the two members a request has to pick between.
#[tokio::test]
async fn start_deployment_names_the_members_a_request_must_pick_between() {
    // The crate's own constant is the one every caller of it states, so it has to BE
    // AWS's sentence rather than merely be used consistently.
    assert_eq!(
        NO_SOURCE_MESSAGE, EXPECTED_NO_SOURCE_MESSAGE,
        "the refusal this service states must be AWS's sentence"
    );

    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("start-without-a-source").await;
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": "main" }))
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateBranch, got {created:?}"
    );

    // One row per way a request can name neither member: nothing at all, and the other
    // optional members on their own. The sentence carries nothing from the request, so
    // every row takes the identical text.
    for (case, body) in [
        ("an empty request", json!({})),
        ("only a sourceUrlType", json!({ "sourceUrlType": "ZIP" })),
        (
            "only a sourceUrlType naming a prefix",
            json!({ "sourceUrlType": "BUCKET_PREFIX" }),
        ),
    ] {
        let refused = emulator
            .call(
                "POST",
                &format!("/apps/{app_id}/branches/main/deployments/start"),
                Some(&body),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the modelled HTTP 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message().as_deref(),
            Some(EXPECTED_NO_SOURCE_MESSAGE),
            "case {case:?}: expected {EXPECTED_NO_SOURCE_MESSAGE:?}, got {:?}",
            refused.message()
        );
    }
}
