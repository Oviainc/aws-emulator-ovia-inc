// SPDX-License-Identifier: CC0-1.0

//! What this service's FRONT DOOR says about a request it refused, end to end over the
//! wire.
//!
//! A value outside a modelled constraint is refused before any handler runs, and the
//! sentence depends on WHERE the value travelled and WHICH class of constraint it broke —
//! never on which operation carried it:
//!
//! * a member in the JSON request BODY is reported by the clause the body-schema validator
//!   states for that class, in brackets: the regular expression and the string that did not
//!   match it for a `@pattern`, the value and both lengths for a `@length` maximum on a
//!   string, the property counts for one on a map;
//! * a member carried OUTSIDE the body has no body-schema clause: a `@httpQuery` value that
//!   is EMPTY where the model requires at least one character never arrives as a parameter
//!   at all, and the refusal names the member alone;
//! * the pipe NAME is the one member the control plane validates itself, and it is reported
//!   as this service's own `ValidationErrorFields` document — in the exception's `message`
//!   AND in the modelled `fieldList` — naming the member as the control plane spells it.
//!
//! Each table's rows span what the rule generalises over (which member, which operation,
//! the form and the length of the value — including values far past the 256 bytes the
//! framework's own violation echo stops at), and the status and error code are pinned with
//! every row. The harness is the one `pipe_lifecycle`, `pipe_tagging` and `pipe_update`
//! already own.

use serde_json::{json, Value};

use crate::pipe_lifecycle::{call, create, describe, minimal_body, router, Reply, REGION};
use crate::pipe_tagging::{encode, pipe_arn, tag};
use crate::pipe_update::create_body;

const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-role";
const SOURCE_SQS: &str = "arn:aws:sqs:us-west-2:000000000000:orders";
const TARGET_LAMBDA: &str = "arn:aws:lambda:us-west-2:000000000000:function:handler";

/// The modelled `@pattern` of each member a row below gets wrong. Source:
/// `smithy_query(query="operation",operation="CreatePipe")` over
/// `services/service-com-amazonaws-pipes-pipes/smithy-model/com-amazonaws-pipes-pipes.json`
/// — `ArnOrUrl` (`Source`), `Arn` (`Target`), `RoleArn`, `KmsKeyIdentifier` and `PipeName`.
const ARN_OR_URL_PATTERN: &str = r"^smk://(([a-zA-Z0-9]|[a-zA-Z0-9][a-zA-Z0-9\-]*[a-zA-Z0-9])\.)*([A-Za-z0-9]|[A-Za-z0-9][A-Za-z0-9\-]*[A-Za-z0-9]):[0-9]{1,5}|arn:(aws[a-zA-Z0-9-]*):([a-zA-Z0-9\-]+):([a-z]{2,4}((-gov)|(-de)|(-iso([a-z]?)))?-[a-z]+(-\d{1})?)?:(\d{12})?:(.+)$";
const ARN_PATTERN: &str = r"^arn:(aws[a-zA-Z0-9-]*):([a-zA-Z0-9\-]+):([a-z]{2,4}((-gov)|(-de)|(-iso([a-z]?)))?-[a-z]+(-\d{1})?)?:(\d{12})?:(.+)$";
const ROLE_ARN_PATTERN: &str = r"^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z0-9+=,.@\-_/]+$";
const KMS_KEY_PATTERN: &str = r"^[a-zA-Z0-9_\-/:]*$";
const PIPE_NAME_PATTERN: &str = r"^[\.\-_A-Za-z0-9]+$";

/// The published maxima the length rows sit past. Source: the same model — `PipeDescription`
/// `@length(0, 512)`, `KmsKeyIdentifier` `@length(0, 2048)`, `TagMap` `@length(1, 50)`.
const MAX_DESCRIPTION: usize = 512;
const MAX_KMS_KEY: usize = 2048;
const MAX_TAGS: usize = 50;

/// Assert that `reply` is a 400 `ValidationException` carrying exactly `message`.
fn assert_refused(case: &str, reply: &Reply, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        message,
        "case {case:?}: expected the message {message:?}, got {reply:?}"
    );
}

/// `n` distinct tags, `key0..key{n-1}` -> `value0..value{n-1}`.
fn generated_tags(n: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..n {
        map.insert(format!("key{index}"), json!(format!("value{index}")));
    }
    Value::Object(map)
}

/// RULE: a BODY member whose value is outside its modelled `@pattern` is refused with the
/// bracketed clause naming the expression the model declares and the string the caller
/// submitted.
///
/// The rows differ along which member and so which expression (a source that may be an ARN
/// or a Kafka URL, a target ARN, a role ARN, a key identifier), and along the form of the
/// rejected value (a URL where an ARN was wanted, plain text, another service's identifier,
/// a value carrying characters the expression excludes).
#[tokio::test]
async fn a_body_member_outside_its_pattern_names_the_expression_and_the_value() {
    // (the case and pipe name, the member, the value sent, the modelled expression).
    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            "sourceasqueueurl",
            "Source",
            "https://127.0.0.1:36667/000000000000/orders".to_string(),
            ARN_OR_URL_PATTERN,
        ),
        (
            "sourceasplaintext",
            "Source",
            "orders".to_string(),
            ARN_OR_URL_PATTERN,
        ),
        (
            "targetnotanarn",
            "Target",
            "results".to_string(),
            ARN_PATTERN,
        ),
        (
            "rolenotarolearn",
            "RoleArn",
            "arn:aws:iam::000000000000:user/not-a-role".to_string(),
            ROLE_ARN_PATTERN,
        ),
        (
            "kmskeywithaspace",
            "KmsKeyIdentifier",
            "alias/pipes key".to_string(),
            KMS_KEY_PATTERN,
        ),
    ];

    for (case, member, value, pattern) in rows {
        let router = router();
        let mut body = minimal_body(SOURCE_SQS, TARGET_LAMBDA);
        body[member] = json!(value);
        let refused = create(&router, case, body).await;
        assert_refused(
            case,
            &refused,
            &format!(
                "[ECMA 262 regex \"{pattern}\" does not match input string \"{value}\"]"
            ),
        );
        let described = describe(&router, case).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused CreatePipe must have stored no pipe, got {described:?}"
        );
    }
}

/// RULE: a BODY member longer than its modelled `@length` maximum is refused with the
/// bracketed clause quoting the value the caller submitted, its length and that maximum —
/// whatever its length, including far past the 256 bytes the framework's own violation echo
/// stops at.
///
/// The rows differ along the member and so the maximum, and along how far past it the value
/// is (one character, hundreds of characters) and whether its characters are single-byte.
#[tokio::test]
async fn a_body_member_past_its_length_maximum_quotes_the_value_and_both_lengths() {
    // (the case and pipe name, the member, the value, the member's maximum).
    let rows: Vec<(&str, &str, String, usize)> = vec![
        (
            "descriptiononepast",
            "Description",
            "x".repeat(MAX_DESCRIPTION + 1),
            MAX_DESCRIPTION,
        ),
        (
            "descriptionfarpast",
            "Description",
            "d".repeat(MAX_DESCRIPTION + 88),
            MAX_DESCRIPTION,
        ),
        (
            // Unicode: `@length` counts CHARACTERS, so the length the clause states is the
            // character count and not the byte count.
            "descriptionunicode",
            "Description",
            "é".repeat(MAX_DESCRIPTION + 1),
            MAX_DESCRIPTION,
        ),
        (
            "kmskeyonepast",
            "KmsKeyIdentifier",
            "k".repeat(MAX_KMS_KEY + 1),
            MAX_KMS_KEY,
        ),
    ];

    for (case, member, value, maximum) in rows {
        let router = router();
        let mut body = minimal_body(SOURCE_SQS, TARGET_LAMBDA);
        body[member] = json!(value);
        let refused = create(&router, case, body).await;
        assert_refused(
            case,
            &refused,
            &format!(
                "[string \"{value}\" is too long (length: {}, maximum allowed: {maximum})]",
                value.chars().count()
            ),
        );
        let described = describe(&router, case).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused CreatePipe must have stored no pipe, got {described:?}"
        );
    }
}

/// RULE: a BODY member that is a MAP carrying more entries than its modelled `@length`
/// maximum is refused with the bracketed clause stating the property counts — the same
/// clause whichever operation carried the map.
///
/// The rows differ along the operation (the create's `Tags` and the tagging layer's `tags`)
/// and along how many entries past the maximum the request carries.
#[tokio::test]
async fn a_body_map_past_its_entry_maximum_states_the_property_counts() {
    let live = "tagcountlive";
    for entries in [MAX_TAGS + 1, MAX_TAGS + 10] {
        let router = router();
        let created = create(&router, live, minimal_body(SOURCE_SQS, TARGET_LAMBDA)).await;
        assert_eq!(
            created.status, 200,
            "the fixture pipe must be created, got {created:?}"
        );

        let expected = format!(
            "[object has too many properties (found {entries} but schema requires at most \
             {MAX_TAGS})]"
        );
        let case = format!("createpipe-{entries}-tags");
        let refused = create(
            &router,
            "tagcountcreate",
            create_body(
                SOURCE_SQS,
                TARGET_LAMBDA,
                json!({"Tags": generated_tags(entries)}),
            ),
        )
        .await;
        assert_refused(&case, &refused, &expected);
        let described = describe(&router, "tagcountcreate").await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused CreatePipe must have stored no pipe, got {described:?}"
        );

        let case = format!("tagresource-{entries}-tags");
        let refused = tag(&router, &pipe_arn(live), &generated_tags(entries)).await;
        assert_refused(&case, &refused, &expected);
        let described = describe(&router, live).await;
        assert_eq!(
            described.member("Tags"),
            &json!({}),
            "case {case:?}: a refused TagResource must have stored no tag, got {described:?}"
        );
    }
}

/// RULE: a `@httpQuery` value that is EMPTY where the model requires at least one
/// character never arrives as a parameter, so the refusal names that member alone — with
/// the modelled `ValidationException`, not the code the tagging layer answers a broken tag
/// rule with.
///
/// The rows differ along how many keys the call names and where the empty one sits among
/// them, so what is reported is the member rather than the position.
#[tokio::test]
async fn an_empty_query_value_names_the_member_alone() {
    let router = router();
    let name = "emptytagkey";
    let created = create(
        &router,
        name,
        create_body(
            SOURCE_SQS,
            TARGET_LAMBDA,
            json!({"Tags": {"env": "prod", "team": "core"}}),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the fixture pipe must be created, got {created:?}"
    );
    let arn = pipe_arn(name);

    // (the case, the keys the call names).
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("the-only-key", vec![""]),
        ("the-first-of-two", vec!["", "env"]),
        ("the-last-of-two", vec!["env", ""]),
        ("between-two-stored-keys", vec!["env", "", "team"]),
    ];

    for (case, keys) in rows {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={}", encode(key)))
            .collect::<Vec<_>>()
            .join("&");
        let refused = call(
            &router,
            "DELETE",
            &format!("/tags/{}?{query}", encode(&arn)),
            None,
            REGION,
        )
        .await;
        assert_refused(case, &refused, "[tagKeys]");

        // The refused call removed nothing: both tags are still there.
        let described = describe(&router, name).await;
        assert_eq!(
            described.member("Tags"),
            &json!({"env": "prod", "team": "core"}),
            "case {case:?}: a refused UntagResource must remove nothing, got {described:?}"
        );
    }

    // The same rule over the OTHER operation that carries query members with a minimum
    // length — so what decides the wording is where the value travelled and not which
    // operation carried it. Source: the `ListPipes` input in `OPERATIONS.md`
    // (`NamePrefix`, `SourcePrefix`, `TargetPrefix` and `NextToken` are each
    // `@length(min: 1)` and `@httpQuery`).
    for (parameter, member) in [
        ("NamePrefix", "namePrefix"),
        ("SourcePrefix", "sourcePrefix"),
        ("TargetPrefix", "targetPrefix"),
        ("NextToken", "nextToken"),
    ] {
        let refused = call(
            &router,
            "GET",
            &format!("/v1/pipes?{parameter}="),
            None,
            REGION,
        )
        .await;
        assert_refused(parameter, &refused, &format!("[{member}]"));
    }

    // A query value that IS there is not refused, so the rule is about the empty value.
    let listed = call(&router, "GET", "/v1/pipes?NamePrefix=empty", None, REGION).await;
    assert_eq!(
        listed.status, 200,
        "a non-empty query value must be accepted, got {listed:?}"
    );
}

/// RULE: a pipe NAME outside `PipeName`'s modelled pattern is the control plane's OWN
/// refusal: the `ValidationErrorFields` document naming the member as the control plane
/// spells it and the expression the model declares, carried both in the exception's
/// `message` and in the modelled `fieldList` — the same document whichever operation
/// addressed the pipe by that name, and whatever else the request got wrong.
///
/// The rows differ along the operation (every one that addresses one pipe by name, read and
/// write) and along which character the name carries that the pattern excludes.
#[tokio::test]
async fn a_pipe_name_outside_its_pattern_is_reported_as_the_services_own_document() {
    let expected_message = format!(
        r#"{{"ValidationErrorFields":[{{"Name":"PipeName","Message":"Invalid pattern for pipe name. Pipe name should match pattern {}."}}]}}"#,
        PIPE_NAME_PATTERN.replace('\\', r"\\")
    );

    for name in ["bad@name", "bad~name", "bad!name", "bad$name", "bad,name"] {
        let router = router();
        // (the case, method, uri, body) of every operation that addresses one pipe by name.
        let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "CreatePipe",
                "POST",
                format!("/v1/pipes/{name}"),
                Some(minimal_body(SOURCE_SQS, TARGET_LAMBDA)),
            ),
            (
                // The name is read from the URI before the body is validated at all, so it
                // is what the refusal is about even when the body is wrong too.
                "CreatePipe with a bad body as well",
                "POST",
                format!("/v1/pipes/{name}"),
                Some(create_body(
                    "not-an-arn",
                    TARGET_LAMBDA,
                    json!({"Description": "x".repeat(MAX_DESCRIPTION + 1)}),
                )),
            ),
            ("DescribePipe", "GET", format!("/v1/pipes/{name}"), None),
            (
                "UpdatePipe",
                "PUT",
                format!("/v1/pipes/{name}"),
                Some(json!({"RoleArn": ROLE})),
            ),
            (
                "StartPipe",
                "POST",
                format!("/v1/pipes/{name}/start"),
                None,
            ),
            ("StopPipe", "POST", format!("/v1/pipes/{name}/stop"), None),
            ("DeletePipe", "DELETE", format!("/v1/pipes/{name}"), None),
        ];

        for (operation, method, uri, body) in rows {
            let case = format!("{operation} {name:?}");
            let refused = call(&router, method, &uri, body, REGION).await;
            assert_refused(&case, &refused, &expected_message);
            assert_eq!(
                refused.member("fieldList"),
                &json!([{
                    "name": "PipeName",
                    "message": format!(
                        "Invalid pattern for pipe name. Pipe name should match pattern \
                         {PIPE_NAME_PATTERN}."
                    ),
                }]),
                "case {case:?}: the fieldList must carry the same entry as the message, \
                 got {refused:?}"
            );
        }
    }
}

/// RULE: a request that satisfies every modelled constraint is NOT refused — the front door
/// above is what refuses one that does not, and it refuses nothing else. Asserted over the
/// members the rows above get wrong, each at the edge of what the model allows.
#[tokio::test]
async fn a_request_at_the_edge_of_every_constraint_is_accepted() {
    let router = router();
    let name = format!("edge.{}_-0", "n".repeat(10));
    let body = create_body(
        // The `smk://` branch of `ArnOrUrl`, which is the one an ARN-shaped pattern check
        // would wrongly refuse.
        "smk://b-1.broker.kafka.us-west-2.amazonaws.com:9092",
        TARGET_LAMBDA,
        json!({
            "Description": "d".repeat(MAX_DESCRIPTION),
            "KmsKeyIdentifier": "k".repeat(MAX_KMS_KEY),
            "Tags": generated_tags(MAX_TAGS),
            // A self-managed Kafka source reads one topic, which is the member its
            // parameters shape requires, so the request that must be accepted carries it.
            "SourceParameters": {"SelfManagedKafkaParameters": {"TopicName": "orders"}},
        }),
    );
    let created = create(&router, &name, body).await;
    assert_eq!(
        created.status, 200,
        "every value is inside its modelled bound, so the request must be accepted, \
         got {created:?}"
    );
    let described = describe(&router, &name).await;
    assert_eq!(
        described.string("Description"),
        "d".repeat(MAX_DESCRIPTION),
        "the accepted pipe must read back, got {described:?}"
    );
}
