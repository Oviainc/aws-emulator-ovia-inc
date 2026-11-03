// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the CLASS of refusal each kind of bad request earns from
//! this service, over every operation that can earn it.
//!
//! Four rules are under test, each asserted over a table of inputs rather than one
//! recorded case:
//!
//! 1. A not-found refusal carries ONE constant message, whatever identifier was asked
//!    about and whichever operation answered: the model states no text for
//!    `ResourceNotFoundException`, so the message is a service-wide constant and never
//!    embeds the identifier.
//! 2. A sink is one per account and Region: once a Region holds one, EVERY further
//!    `CreateSink` is the quota refusal, whatever `Name` it asks for, and deleting the
//!    stored sink frees the slot again.
//! 3. A value outside a member's modelled `@pattern` is refused by the REST front end
//!    that validates the request body — HTTP 400 `BadRequestException`, naming the
//!    regular expression and the value — while a value outside any other modelled
//!    bound, and an absent member, are the service's own modelled parameter errors.
//!    `ListAttachedLinks` is the exception: its authorization decision is taken on the
//!    sink the request names BEFORE the identifier is judged, so a value no resource
//!    identifier could be earns that decision instead (rule 4).
//! 4. An identifier that names the OTHER of this service's two resource kinds cannot
//!    be authorized for the action asked for, so it is refused by the authorization
//!    decision (HTTP 403 `AccessDeniedException`) before any lookup — identically
//!    whether or not anything is stored under it.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers), so what is asserted
//! is what a signed AWS SDK request observes.

use serde_json::{json, Value};
use service_com_amazonaws_oam_oamservice::state::{
    caller_principal, sink_quota_message, RESOURCE_NOT_FOUND_MESSAGE,
};

use super::sinks_and_links::{
    call, create_link_ok, create_sink_ok, emulator, error_code, field, ACCOUNT, AKID,
    RESOURCE_TYPES,
};

/// The region most cases sign for.
const REGION: &str = "us-west-2";

/// A second region, so a case can ask about a resource that exists but not at the
/// endpoint the request is signed for.
const OTHER_REGION: &str = "eu-west-1";

/// `SinkName`'s modelled `@pattern`
/// (`smithy_query(query="shape",shape="SinkName")`), which `CreateSink.Name` targets.
const SINK_NAME_PATTERN: &str = r"^[a-zA-Z0-9_\.\-]{1,255}$";

/// `ResourceIdentifier`'s modelled `@pattern`
/// (`smithy_query(query="shape",shape="ResourceIdentifier")`), which every
/// `Identifier` / `SinkIdentifier` member of this service targets.
const RESOURCE_IDENTIFIER_PATTERN: &str = r"^[a-zA-Z0-9][a-zA-Z0-9_:\.\-\/]{0,2047}$";

/// A sink policy this service accepts, so a `PutSinkPolicy` case is refused for its
/// identifier rather than for its document.
fn accepted_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": "oam:CreateLink",
        }],
    })
    .to_string()
}

/// The `message` member of a refusal, whichever casing the answering shape models it
/// with: the modelled error shapes of this service spell it `Message`, the framework's
/// own pre-handler envelope `message`.
fn refusal_message(body: &Value) -> String {
    for member in ["Message", "message"] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    panic!("expected a message member, got {body:?}")
}

/// Every call that can answer not-found for a sink, as `(operation, request)` with
/// `identifier` substituted into the member that operation names it by.
fn sink_calls(identifier: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("/GetSink", json!({ "Identifier": identifier })),
        ("/DeleteSink", json!({ "Identifier": identifier })),
        ("/GetSinkPolicy", json!({ "SinkIdentifier": identifier })),
        (
            "/PutSinkPolicy",
            json!({ "SinkIdentifier": identifier, "Policy": accepted_policy() }),
        ),
        (
            "/ListAttachedLinks",
            json!({ "SinkIdentifier": identifier }),
        ),
    ]
}

/// Every call that can answer not-found for a link.
fn link_calls(identifier: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("/GetLink", json!({ "Identifier": identifier })),
        ("/DeleteLink", json!({ "Identifier": identifier })),
    ]
}

#[tokio::test]
async fn every_not_found_refusal_carries_the_one_constant_message() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", REGION).await;
    let sink_id = sink_arn
        .rsplit('/')
        .next()
        .expect("a sink ARN ends in its id")
        .to_string();
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;
    let link_id = link_arn
        .rsplit('/')
        .next()
        .expect("a link ARN ends in its id")
        .to_string();
    let unknown = "11111111-2222-3333-4444-555555555555";

    // Rows differ along WHY the identifier resolves to nothing — its form (a bare id,
    // a name, a full ARN), and which ARN field misses (account, region, partition) —
    // and along which operation answers. None of that may change the message.
    let mut cases: Vec<(String, &'static str, Value, &'static str)> = Vec::new();
    for (why, identifier) in [
        (
            "a bare id nothing was stored under".to_string(),
            unknown.to_string(),
        ),
        (
            "a name nothing was stored under".to_string(),
            "other-sink".to_string(),
        ),
        (
            "the stored id in another account".to_string(),
            format!("arn:aws:oam:{REGION}:123456789012:sink/{sink_id}"),
        ),
        (
            "the stored id in another partition".to_string(),
            format!("arn:aws-cn:oam:{REGION}:{ACCOUNT}:sink/{sink_id}"),
        ),
        (
            "an ARN nothing was stored under".to_string(),
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:sink/{unknown}"),
        ),
    ] {
        for (uri, body) in sink_calls(&identifier) {
            cases.push((format!("{uri} with {why}"), uri, body, REGION));
        }
    }
    for (why, identifier) in [
        (
            "a bare id nothing was stored under".to_string(),
            unknown.to_string(),
        ),
        (
            "the stored link id in another account".to_string(),
            format!("arn:aws:oam:{REGION}:123456789012:link/{link_id}"),
        ),
        (
            "a link ARN nothing was stored under".to_string(),
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:link/{unknown}"),
        ),
    ] {
        for (uri, body) in link_calls(&identifier) {
            cases.push((format!("{uri} with {why}"), uri, body, REGION));
        }
    }
    // A sink and a link that DO exist, asked about at another Region's endpoint: a
    // regional endpoint sees only its own Region's resources.
    for (uri, body) in sink_calls(&sink_arn) {
        cases.push((
            format!("{uri} for a stored sink at another Region's endpoint"),
            uri,
            body,
            OTHER_REGION,
        ));
    }
    for (uri, body) in link_calls(&link_arn) {
        cases.push((
            format!("{uri} for a stored link at another Region's endpoint"),
            uri,
            body,
            OTHER_REGION,
        ));
    }

    for (case, uri, body, region) in &cases {
        let (status, refused) = call(&router, uri, body, region).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {refused:?}"
        );
        assert_eq!(
            refusal_message(&refused),
            RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the one constant not-found message, got {refused:?}"
        );
    }

    // The policy of a sink that has none is the same refusal, with the same message,
    // as a sink that is not there at all: the policy is the resource being asked for.
    let (status, refused) = call(
        &router,
        "/GetSinkPolicy",
        &json!({ "SinkIdentifier": sink_arn.clone() }),
        REGION,
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (404, "ResourceNotFoundException"),
        "a sink with no policy: expected 404, got {status} {refused:?}"
    );
    assert_eq!(
        refusal_message(&refused),
        RESOURCE_NOT_FOUND_MESSAGE,
        "a sink with no policy: expected the constant message, got {refused:?}"
    );

    // And a deleted resource: the message does not become identifier-bearing once
    // something WAS stored under it.
    for (uri, body) in [
        ("/DeleteLink", json!({ "Identifier": link_arn.clone() })),
        ("/DeleteSink", json!({ "Identifier": sink_arn.clone() })),
    ] {
        let (status, deleted) = call(&router, uri, &body, REGION).await;
        assert_eq!(status, 200, "{uri}: expected 200, got {status} {deleted:?}");
        let (status, refused) = call(&router, uri, &body, REGION).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "{uri} twice: expected 404 ResourceNotFoundException, got {status} {refused:?}"
        );
        assert_eq!(
            refusal_message(&refused),
            RESOURCE_NOT_FOUND_MESSAGE,
            "{uri} twice: expected the constant message, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn an_account_region_holds_one_sink_whatever_name_a_create_asks_for() {
    let router = emulator();
    let first = create_sink_ok(&router, "sink-a", REGION).await;

    // Rows differ along the `Name` the second create asks for — the member the
    // defective rule keyed on — including the name already in use, names nothing
    // holds, and both ends of the pattern's length (one character, and the 255 the
    // modelled pattern admits).
    let longest = "s".repeat(255);
    let names: &[(&str, &str)] = &[
        ("the name already in use", "sink-a"),
        ("a name nothing holds", "sink-b"),
        ("a single-character name", "s"),
        ("the longest name the pattern admits", longest.as_str()),
        ("a name of digits only", "20260101"),
        ("dots, dashes and underscores", "Team.Sink_01-prod"),
    ];

    for (case, name) in names {
        let (status, refused) = call(&router, "/CreateSink", &json!({ "Name": name }), REGION).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (429, "ServiceQuotaExceededException"),
            "case {case:?}: expected 429 ServiceQuotaExceededException, got {status} {refused:?}"
        );
        assert_eq!(
            refusal_message(&refused),
            sink_quota_message(),
            "case {case:?}: expected the account-scoped quota message, got {refused:?}"
        );
        // And nothing was stored for the refused name.
        let (status, got) = call(&router, "/GetSink", &json!({ "Identifier": name }), REGION).await;
        let expected = if *name == "sink-a" { 200 } else { 404 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected} from GetSink after the refusal, got {status} \
             {got:?}"
        );
    }

    // The stored sink is untouched, another Region holds its own, and deleting the
    // stored one frees the slot for a differently named create.
    let (status, got) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": "sink-a" }),
        REGION,
    )
    .await;
    assert_eq!(
        (status, field("the first sink", &got, "Arn")),
        (200, first.clone()),
        "the first sink must still be there, got {status} {got:?}"
    );
    let other = create_sink_ok(&router, "sink-b", OTHER_REGION).await;
    assert_ne!(
        other, first,
        "another Region's sink must be its own resource"
    );

    let (status, deleted) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": first.clone() }),
        REGION,
    )
    .await;
    assert_eq!(status, 200, "DeleteSink: got {status} {deleted:?}");
    let replacement = create_sink_ok(&router, "sink-c", REGION).await;
    assert_ne!(
        replacement, first,
        "the replacement sink must be its own resource"
    );
}

#[tokio::test]
async fn a_value_outside_a_modelled_pattern_is_refused_by_the_body_validator() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", REGION).await;

    // Rows differ along the member carrying the pattern, the operation that binds it,
    // and WHICH part of the pattern the value misses (a character the class excludes,
    // a leading character the first class excludes, an empty value, a value one
    // character past the length the pattern itself bounds).
    let too_long_name = "s".repeat(256);
    let cases: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            "CreateSink Name with a space and a bang",
            "/CreateSink",
            json!({ "Name": "bad name!" }),
            SINK_NAME_PATTERN,
            "bad name!".to_string(),
        ),
        (
            "CreateSink Name one character past the pattern's length",
            "/CreateSink",
            json!({ "Name": too_long_name.clone() }),
            SINK_NAME_PATTERN,
            too_long_name.clone(),
        ),
        (
            "CreateSink Name with a slash",
            "/CreateSink",
            json!({ "Name": "team/sink" }),
            SINK_NAME_PATTERN,
            "team/sink".to_string(),
        ),
        (
            "CreateSink Name that is empty",
            "/CreateSink",
            json!({ "Name": "" }),
            SINK_NAME_PATTERN,
            String::new(),
        ),
        (
            "GetSink Identifier starting with an excluded character",
            "/GetSink",
            json!({ "Identifier": "_leading-underscore" }),
            RESOURCE_IDENTIFIER_PATTERN,
            "_leading-underscore".to_string(),
        ),
        (
            "DeleteSink Identifier with a space",
            "/DeleteSink",
            json!({ "Identifier": "sink name" }),
            RESOURCE_IDENTIFIER_PATTERN,
            "sink name".to_string(),
        ),
        (
            "GetLink Identifier that is empty",
            "/GetLink",
            json!({ "Identifier": "" }),
            RESOURCE_IDENTIFIER_PATTERN,
            String::new(),
        ),
        (
            "DeleteLink Identifier with a comma",
            "/DeleteLink",
            json!({ "Identifier": "link,one" }),
            RESOURCE_IDENTIFIER_PATTERN,
            "link,one".to_string(),
        ),
        (
            "GetSinkPolicy SinkIdentifier with a space",
            "/GetSinkPolicy",
            json!({ "SinkIdentifier": "sink name" }),
            RESOURCE_IDENTIFIER_PATTERN,
            "sink name".to_string(),
        ),
        (
            "PutSinkPolicy SinkIdentifier with a space",
            "/PutSinkPolicy",
            json!({ "SinkIdentifier": "sink name", "Policy": accepted_policy() }),
            RESOURCE_IDENTIFIER_PATTERN,
            "sink name".to_string(),
        ),
        (
            "CreateLink SinkIdentifier with a space",
            "/CreateLink",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": [RESOURCE_TYPES[0]],
                "SinkIdentifier": "sink name",
            }),
            RESOURCE_IDENTIFIER_PATTERN,
            "sink name".to_string(),
        ),
        (
            "UpdateLink Identifier with a space",
            "/UpdateLink",
            json!({ "Identifier": "link one", "ResourceTypes": [RESOURCE_TYPES[0]] }),
            RESOURCE_IDENTIFIER_PATTERN,
            "link one".to_string(),
        ),
    ];

    for (case, uri, body, pattern, value) in &cases {
        let (status, refused) = call(&router, uri, body, REGION).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {status} {refused:?}"
        );
        assert_eq!(
            refusal_message(&refused),
            format!(
                "Invalid request body: [ECMA 262 regex \"{pattern}\" does not match input \
                 string \"{value}\"]"
            ),
            "case {case:?}: expected the body validator's regex clause, got {refused:?}"
        );
    }

    // A value past what a refusal will echo is still refused by the same rule, and the
    // message does not grow with the request.
    let far_too_long = "x".repeat(4_096);
    let (status, refused) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": format!("{far_too_long}!") }),
        REGION,
    )
    .await;
    let message = refusal_message(&refused);
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "BadRequestException"),
        "an over-long identifier: expected 400 BadRequestException, got {status} {refused:?}"
    );
    assert!(
        message.starts_with(&format!(
            "Invalid request body: [ECMA 262 regex \"{RESOURCE_IDENTIFIER_PATTERN}\" does not \
             match input string \""
        )) && message.len() < far_too_long.len(),
        "an over-long identifier: expected a bounded regex clause, got {message:?}"
    );

    // The boundary of the rule: every OTHER refusal class keeps the code the answering
    // layer models. A value outside a `@length` or `@range`, or an element outside an
    // enum, is `InvalidParameterException`; an absent member is
    // `MissingRequiredParameterException`; and a violation of the `@pattern` on a
    // member bound to the URI rather than the body (`ResourceArn`, an `@httpLabel` of
    // the three tagging operations) keeps the `ValidationException` those model.
    let contrasts: &[(&str, &str, Value, u16, &str)] = &[
        (
            "ListAttachedLinks MaxResults one below the modelled minimum",
            "/ListAttachedLinks",
            json!({ "SinkIdentifier": "demo-sink", "MaxResults": 0 }),
            400,
            "InvalidParameterException",
        ),
        (
            "ListAttachedLinks MaxResults one past the modelled maximum",
            "/ListAttachedLinks",
            json!({ "SinkIdentifier": "demo-sink", "MaxResults": 1001 }),
            400,
            "InvalidParameterException",
        ),
        (
            "CreateLink with an empty ResourceTypes",
            "/CreateLink",
            json!({
                "LabelTemplate": "$AccountName",
                "ResourceTypes": [],
                "SinkIdentifier": "demo-sink",
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "CreateSink with no Name at all",
            "/CreateSink",
            json!({}),
            400,
            "MissingRequiredParameterException",
        ),
        (
            "DeleteSink with no Identifier at all",
            "/DeleteSink",
            json!({}),
            400,
            "MissingRequiredParameterException",
        ),
    ];
    for (case, uri, body, expected_status, expected_code) in contrasts {
        let (status, refused) = call(&router, uri, body, REGION).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (*expected_status, *expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {refused:?}"
        );
    }

    // The stored sink is untouched by any of it.
    let (status, got) = call(
        &router,
        "/GetSink",
        &json!({ "Identifier": sink_arn.clone() }),
        REGION,
    )
    .await;
    assert_eq!(
        status, 200,
        "the stored sink must still answer, got {status} {got:?}"
    );
}

/// The explicit-deny refusal `operation` answers for `identifier`.
fn expected_deny(operation: &str, identifier: &str) -> String {
    format!(
        "User: {} is not authorized to perform: oam:{operation} on resource: {identifier} with \
         an explicit deny in a resource-based policy",
        caller_principal()
    )
}

#[tokio::test]
async fn an_identifier_of_the_other_resource_kind_is_refused_before_any_lookup() {
    let router = emulator();
    let sink_arn = create_sink_ok(&router, "demo-sink", REGION).await;
    let link_arn = create_link_ok(&router, &sink_arn, AKID, REGION).await;
    let unknown = "11111111-2222-3333-4444-555555555555";

    // Rows differ along the operation (and the member it names its identifier by),
    // along the resource kind that operation acts on, and along whether the wrongly
    // typed ARN names something STORED or nothing at all — which the refusal may not
    // disclose.
    let link_shaped: Vec<(&str, String)> = vec![
        ("a stored link's ARN", link_arn.clone()),
        (
            "a link ARN nothing was stored under",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:link/{unknown}"),
        ),
        (
            "a link ARN of another account",
            format!("arn:aws:oam:{REGION}:123456789012:link/{unknown}"),
        ),
    ];
    let sink_shaped: Vec<(&str, String)> = vec![
        ("a stored sink's ARN", sink_arn.clone()),
        (
            "a sink ARN nothing was stored under",
            format!("arn:aws:oam:{REGION}:{ACCOUNT}:sink/{unknown}"),
        ),
    ];

    for (why, identifier) in &link_shaped {
        let calls: Vec<(&str, &str, Value)> = vec![
            ("GetSink", "/GetSink", json!({ "Identifier": identifier })),
            (
                "DeleteSink",
                "/DeleteSink",
                json!({ "Identifier": identifier }),
            ),
            (
                "GetSinkPolicy",
                "/GetSinkPolicy",
                json!({ "SinkIdentifier": identifier }),
            ),
            (
                "PutSinkPolicy",
                "/PutSinkPolicy",
                json!({ "SinkIdentifier": identifier, "Policy": accepted_policy() }),
            ),
            (
                "ListAttachedLinks",
                "/ListAttachedLinks",
                json!({ "SinkIdentifier": identifier }),
            ),
            (
                "CreateLink",
                "/CreateLink",
                json!({
                    "LabelTemplate": "$AccountName",
                    "ResourceTypes": [RESOURCE_TYPES[1]],
                    "SinkIdentifier": identifier,
                }),
            ),
        ];
        for (operation, uri, body) in calls {
            let case = format!("{operation} given {why}");
            let (status, refused) = call(&router, uri, &body, REGION).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected 403 AccessDeniedException, got {status} {refused:?}"
            );
            assert_eq!(
                refusal_message(&refused),
                expected_deny(operation, identifier),
                "case {case:?}: expected the explicit-deny message, got {refused:?}"
            );
        }
    }

    for (why, identifier) in &sink_shaped {
        // `UpdateLink` is the one operation whose decision is NOT taken on the
        // identifier (see `update_link.rs`), so it is asserted there.
        let calls: Vec<(&str, &str, Value)> = vec![
            ("GetLink", "/GetLink", json!({ "Identifier": identifier })),
            (
                "DeleteLink",
                "/DeleteLink",
                json!({ "Identifier": identifier }),
            ),
        ];
        for (operation, uri, body) in calls {
            let case = format!("{operation} given {why}");
            let (status, refused) = call(&router, uri, &body, REGION).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected 403 AccessDeniedException, got {status} {refused:?}"
            );
            assert_eq!(
                refusal_message(&refused),
                expected_deny(operation, identifier),
                "case {case:?}: expected the explicit-deny message, got {refused:?}"
            );
        }
    }

    // The refusal discloses nothing: the stored link's ARN and an ARN nothing was
    // stored under earn messages that differ only in the identifier they were asked
    // about.
    let (_, stored) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": link_arn.clone() }),
        REGION,
    )
    .await;
    let absent_link = format!("arn:aws:oam:{REGION}:{ACCOUNT}:link/{unknown}");
    let (_, absent) = call(
        &router,
        "/DeleteSink",
        &json!({ "Identifier": absent_link.clone() }),
        REGION,
    )
    .await;
    assert_eq!(
        (
            refusal_message(&stored).replace(&link_arn, "<identifier>"),
            refusal_message(&absent).replace(&absent_link, "<identifier>")
        ),
        (
            expected_deny("DeleteSink", "<identifier>"),
            expected_deny("DeleteSink", "<identifier>")
        ),
        "a stored and an absent link must earn the same refusal: {stored:?} / {absent:?}"
    );

    // And nothing it refused was done: both resources are still there, and every
    // identifier form of the RIGHT kind still resolves as it did.
    for (case, uri, body, expected) in [
        (
            "the sink by its own ARN",
            "/GetSink",
            json!({ "Identifier": sink_arn.clone() }),
            200,
        ),
        (
            "the link by its own ARN",
            "/GetLink",
            json!({ "Identifier": link_arn.clone() }),
            200,
        ),
        (
            "the sink by its name",
            "/GetSink",
            json!({ "Identifier": "demo-sink" }),
            200,
        ),
        (
            "a sink id nothing was stored under",
            "/GetSink",
            json!({ "Identifier": unknown }),
            404,
        ),
        (
            "a link id nothing was stored under",
            "/GetLink",
            json!({ "Identifier": unknown }),
            404,
        ),
    ] {
        let (status, got) = call(&router, uri, &body, REGION).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {got:?}"
        );
    }

    // The tagging operations take an `Arn` that names EITHER kind, so neither kind is
    // the wrong one for them and neither is denied; `resource_tags.rs` asserts what
    // they answer for each.
}
