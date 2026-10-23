// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a request that breaks a modelled `@pattern`.
//!
//! THE RULE: a boundary refusal is `HTTP 400` / `ValidationException` carrying the AWS
//! envelope — `1 validation error detected: Value '<value>' at '<member path>' failed to
//! satisfy constraint: Member must satisfy regular expression pattern: <expression>` — and
//! `<expression>` is the member's modelled `@pattern` MINUS the leading `^` and the
//! trailing `$` the model writes around the expression a value has to satisfy. The check
//! still runs the model's pattern as written; only the quoting drops the pair.
//!
//! The rows below span every input member of this service that carries a `@pattern` and
//! both expressions those members target, and they vary along what the rule generalises
//! over — which member, which operation, which modelled expression, and how the submitted
//! value misses it (length, character class, non-ASCII, a wrong ARN segment):
//!
//! * `CreateNetworkConnectorRequest$Name` -> `com.amazonaws.lambdacore#NetworkConnectorName`,
//!   whose expression ALTERNATES at the top level, so the anchors the model wraps it in are
//!   the only thing dropped and the alternation is quoted verbatim;
//! * `CreateNetworkConnectorRequest$OperatorRole` and
//!   `UpdateNetworkConnectorRequest$OperatorRole` ->
//!   `com.amazonaws.lambdacore#NetworkConnectorRoleArn`, the same expression reached through
//!   two different operations.
//!
//! `NetworkConnectorTags`' key and value also declare a `@pattern`, and no row covers them:
//! the framework's constraint walk captures a map's collection-level traits only (a map key
//! is recorded as the entry's path segment and never checked, and a map value is checked
//! against the map MEMBER's schema), so no request can produce a pattern violation at a tag
//! today. `NetworkConnectorIdentifier` — the only other string member bound to a request —
//! declares no `@pattern` at all.
//!
//! The requirement for every OTHER constraint class, the member path and the envelope stay
//! the framework's, which the length rows pin.
//!
//! The harness (router, request helpers) is the lifecycle suite's; it is `pub(crate)` there
//! so every suite drives the emulator the same way.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::network_connector_lifecycle::{
    call, create, emulator, minimal_configuration, text, update, Answer, PREFIX,
};

/// `NetworkConnectorName`'s `@pattern` as the MODEL spells it
/// (`smithy_query(query="shape",shape="NetworkConnectorName")`).
const NAME_PATTERN_MODELLED: &str = r"^(arn:aws[a-zA-Z-]*:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:network-connector:[a-zA-Z0-9-_]+(:[1-9][0-9]*)?)|[a-zA-Z0-9_-]{1,64}$";

/// The same expression as a refusal QUOTES it: the model's text minus the leading `^` and
/// the trailing `$`. The pair is dropped even though the expression alternates at the top
/// level — the anchors are what the model writes around the service's own expression, so
/// what is left is the expression the service advertises.
const NAME_PATTERN_QUOTED: &str = r"(arn:aws[a-zA-Z-]*:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:network-connector:[a-zA-Z0-9-_]+(:[1-9][0-9]*)?)|[a-zA-Z0-9_-]{1,64}";

/// `NetworkConnectorRoleArn`'s `@pattern` as the MODEL spells it
/// (`smithy_query(query="shape",shape="NetworkConnectorRoleArn")`).
const ROLE_PATTERN_MODELLED: &str = r"^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$";

/// The same expression as a refusal quotes it.
const ROLE_PATTERN_QUOTED: &str = r"arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+";

/// An `OperatorRole` the modelled expression accepts, for a row that is refused for a
/// DIFFERENT member.
const VALID_ROLE: &str = "arn:aws:iam::000000000000:role/LambdaNetworkConnectorOperator";

/// The connector the `UpdateNetworkConnector` rows name. A boundary refusal is answered
/// before any handler runs, but naming a connector that exists keeps each row a request a
/// client would really send.
const UPDATE_HOST: &str = "lnc-wording-host";

/// The error code every constraint refusal of this restJson1 service carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One request that breaks exactly one modelled constraint, and the clause the refusal has
/// to state for it.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// `POST` creates; `PUT` updates [`UPDATE_HOST`].
    method: &'static str,
    /// The request body.
    body: Value,
    /// The member path the clause names (lowerCamel, as the framework spells a path).
    member: &'static str,
    /// The value the clause echoes back — the caller's own bytes.
    value: String,
    /// The requirement the clause must state.
    requirement: String,
    /// Model text that must NOT reach the wire (the anchored source of a quoted pattern),
    /// or `None` for a row whose requirement quotes no pattern.
    withheld: Option<&'static str>,
}

/// A `@pattern` requirement, quoted the way a refusal quotes it.
fn pattern_requirement(quoted: &str) -> String {
    format!("Member must satisfy regular expression pattern: {quoted}")
}

/// Send one row's request.
async fn send(router: &MultiServiceRouter, row: &Row) -> Answer {
    match row.method {
        "POST" => create(router, row.body.clone()).await,
        "PUT" => update(router, UPDATE_HOST, row.body.clone()).await,
        other => panic!("case {:?}: unsupported method {other:?}", row.case),
    }
}

/// A create request naming `name`, with the simplest configuration the model accepts.
fn create_body(name: &str) -> Value {
    json!({ "Name": name, "Configuration": minimal_configuration() })
}

/// Assert `answer` is the one-clause AWS envelope for `row`.
fn assert_refused(row: &Row, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {:?}: a value outside a modelled constraint is refused with HTTP 400, got {} \
         with {:?}",
        row.case, answer.status, answer.body
    );
    let code = text(&answer.body, "/__type");
    assert!(
        code.ends_with(VALIDATION_EXCEPTION),
        "case {:?}: expected the error code {VALIDATION_EXCEPTION}, got {code:?}",
        row.case
    );
    let message = text(&answer.body, "/message");
    let expected = format!(
        "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: {}",
        row.value, row.member, row.requirement
    );
    assert_eq!(
        message, expected,
        "case {:?}: expected the message {expected:?}, got {message:?}",
        row.case
    );
    if let Some(withheld) = row.withheld {
        assert!(
            !message.contains(withheld),
            "case {:?}: the model's anchored spelling {withheld:?} must never reach the wire, \
             got {message:?}",
            row.case
        );
    }
}

/// RULE: a refused `@pattern` is quoted as the expression a value has to satisfy, without
/// the anchors the model wraps it in — on every member that declares one, through every
/// operation that binds it.
#[tokio::test]
async fn a_refused_pattern_is_quoted_without_the_models_anchors() {
    let router = emulator();
    let host = create(&router, create_body(UPDATE_HOST)).await;
    assert_eq!(
        host.status, 202,
        "expected HTTP 202 from CreateNetworkConnector, got {} with {:?}",
        host.status, host.body
    );

    let rows = [
        // `Name`: the value the recording was taken from — a run of words ending in
        // characters the expression's name branch does not admit.
        Row {
            case: "a name with spaces and symbols",
            method: "POST",
            body: create_body("bad name has spaces & symbols!!"),
            member: "name",
            value: "bad name has spaces & symbols!!".to_string(),
            requirement: pattern_requirement(NAME_PATTERN_QUOTED),
            withheld: Some(NAME_PATTERN_MODELLED),
        },
        // `Name`: a much shorter value, missing the expression on a single character.
        Row {
            case: "a short name ending in a character the expression excludes",
            method: "POST",
            body: create_body("lnc="),
            member: "name",
            value: "lnc=".to_string(),
            requirement: pattern_requirement(NAME_PATTERN_QUOTED),
            withheld: Some(NAME_PATTERN_MODELLED),
        },
        // `Name`: a non-ASCII value, so the row does not depend on which ASCII
        // punctuation the expression leaves out.
        Row {
            case: "a non-ASCII name",
            method: "POST",
            body: create_body("ネットワーク"),
            member: "name",
            value: "ネットワーク".to_string(),
            requirement: pattern_requirement(NAME_PATTERN_QUOTED),
            withheld: Some(NAME_PATTERN_MODELLED),
        },
        // `OperatorRole` on CREATE: a second modelled expression, reported at a second
        // member path.
        Row {
            case: "an operator role that is not an ARN at all",
            method: "POST",
            body: json!({
                "Name": "lnc-role-rows",
                "Configuration": minimal_configuration(),
                "OperatorRole": "not-an-arn",
            }),
            member: "operatorRole",
            value: "not-an-arn".to_string(),
            requirement: pattern_requirement(ROLE_PATTERN_QUOTED),
            withheld: Some(ROLE_PATTERN_MODELLED),
        },
        // `OperatorRole` on CREATE: an ARN of the wrong SERVICE, so only one segment of
        // the expression is missed.
        Row {
            case: "an operator role naming another service",
            method: "POST",
            body: json!({
                "Name": "lnc-role-rows",
                "Configuration": minimal_configuration(),
                "OperatorRole": "arn:aws:s3:::my-bucket",
            }),
            member: "operatorRole",
            value: "arn:aws:s3:::my-bucket".to_string(),
            requirement: pattern_requirement(ROLE_PATTERN_QUOTED),
            withheld: Some(ROLE_PATTERN_MODELLED),
        },
        // The SAME expression through UPDATE: the rule is the member's, not one
        // operation's. Here the account id is 5 digits where the expression wants 12.
        Row {
            case: "an operator role with a short account id, on update",
            method: "PUT",
            body: json!({ "OperatorRole": "arn:aws:iam::12345:role/Operator" }),
            member: "operatorRole",
            value: "arn:aws:iam::12345:role/Operator".to_string(),
            requirement: pattern_requirement(ROLE_PATTERN_QUOTED),
            withheld: Some(ROLE_PATTERN_MODELLED),
        },
    ];

    for row in &rows {
        let answer = send(&router, row).await;
        assert_refused(row, &answer);
    }
}

/// RULE, the other half: only the `@pattern` QUOTING is this service's; every other
/// requirement, the member path and the envelope are the framework's and are unchanged.
#[tokio::test]
async fn a_constraint_that_is_not_a_pattern_keeps_the_frameworks_requirement() {
    let router = emulator();
    // 141 characters of the name branch's own alphabet: inside the expression (which is a
    // find, so its 1..64 run matches at the end) and one past `@length`'s maximum of 140
    // (`smithy_query(query="shape",shape="NetworkConnectorName")`).
    let too_long = "a".repeat(141);
    // One past `ClientTokenString`'s `@length` maximum of 64; the shape declares no
    // `@pattern`.
    let long_token = "t".repeat(65);
    let rows = [
        Row {
            case: "a name one character past the modelled maximum",
            method: "POST",
            body: create_body(&too_long),
            member: "name",
            value: too_long.clone(),
            requirement: "Member must have length less than or equal to 140".to_string(),
            withheld: None,
        },
        Row {
            case: "a client token one character past the modelled maximum",
            method: "POST",
            body: json!({
                "Name": "lnc-length-rows",
                "Configuration": minimal_configuration(),
                "ClientToken": long_token.clone(),
            }),
            member: "clientToken",
            value: long_token.clone(),
            requirement: "Member must have length less than or equal to 64".to_string(),
            withheld: None,
        },
    ];

    for row in &rows {
        let answer = send(&router, row).await;
        assert_refused(row, &answer);
        let message = text(&answer.body, "/message");
        assert!(
            !message.contains("regular expression pattern"),
            "case {:?}: a length refusal states no pattern, got {message:?}",
            row.case
        );
    }
}

/// RULE, the inverse: the quoting changes the MESSAGE only. Every value the modelled
/// expression admits is still accepted, so the rows below — which span the name branch's
/// alphabet, its length bound and the ARN branch the alternation adds — are created rather
/// than refused.
#[tokio::test]
async fn a_value_the_modelled_expression_admits_is_still_accepted() {
    let router = emulator();
    let accepted = [
        ("a plain name", "connector".to_string()),
        ("dashes and underscores", "lnc_egress-01".to_string()),
        ("mixed case and digits", "Lnc0Egress9".to_string()),
        ("the name branch's longest run", "n".repeat(64)),
        (
            "the ARN branch of the alternation",
            format!(
                "arn:aws:lambda:eu-west-1:000000000000:network-connector:{}",
                "arn-form-name"
            ),
        ),
    ];
    for (case, name) in &accepted {
        let answer = create(&router, create_body(name)).await;
        assert_eq!(
            answer.status, 202,
            "case {case:?}: {name:?} satisfies the modelled expression, so \
             CreateNetworkConnector must accept it, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            text(&answer.body, "/Name"),
            *name,
            "case {case:?}: the created connector must report the name it was given, got {:?}",
            answer.body["Name"]
        );
    }
    // An accepted `OperatorRole` travels the same path as the refused ones above.
    let with_role = create(
        &router,
        json!({
            "Name": "lnc-with-role",
            "Configuration": minimal_configuration(),
            "OperatorRole": VALID_ROLE,
        }),
    )
    .await;
    assert_eq!(
        with_role.status, 202,
        "an operator role the modelled expression admits must be accepted, got {} with {:?}",
        with_role.status, with_role.body
    );
    assert_eq!(
        text(&with_role.body, "/OperatorRole"),
        VALID_ROLE,
        "the created connector must report the operator role it was given, got {:?}",
        with_role.body["OperatorRole"]
    );
    // And the refusal path is reached through the same URI space the accepted calls use.
    let answer = call(&router, "POST", PREFIX, Some(create_body("nope!"))).await;
    assert_eq!(
        answer.status, 400,
        "expected the refusal on the same route, got {} with {:?}",
        answer.status, answer.body
    );
}
