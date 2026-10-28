// SPDX-License-Identifier: CC0-1.0

//! The `RedactedFields` of a web ACL's logging configuration, end to end over the
//! wire.
//!
//! The rule under test: `RedactedFields` is part of the mutable specification a
//! logging configuration carries — "the parts of the request that you want to keep
//! out of the logs"
//! (`smithy_query(query="doc",shape="LoggingConfiguration",member="RedactedFields")`)
//! — so a put stores the list the request gives, entry for entry and in the order
//! given, every reader answers back exactly that list, and a later put that leaves
//! the member out replaces it away ("this operation completely replaces any mutable
//! specifications that you already have for a logging configuration").
//!
//! The rows differ along what that rule generalises over: which request component is
//! redacted (the four `FieldToMatch` settings the member's own note names —
//! `UriPath`, `QueryString`, `SingleHeader` and `Method`), how many entries the list
//! carries (none, one, several), which header a `SingleHeader` entry names, the
//! ORDER of two otherwise equal entries, whether the member is absent or present but
//! empty, and the web ACL's `Scope`. The element-count bound
//! (`smithy_query(query="shape",shape="RedactedFields")`: `@length(0..100)`) is
//! asserted on each published bound and one past the ceiling.
//!
//! Every call goes through the same front door a real client uses (the `Waf` harness
//! in [`crate::web_acl_lifecycle`]), so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};

use crate::parameter_validation::{error_code, error_message};
use crate::web_acl_lifecycle::{create_web_acl, Waf};

/// A logging destination WAF delivers to: its name carries the `aws-waf-logs-`
/// prefix, so the destination rule (the sibling
/// [`crate::logging_destination_names`] suite's subject) is not what any row here
/// turns on.
const DESTINATION: &str = "arn:aws:s3:::aws-waf-logs-redaction";

/// The element-count bounds `RedactedFields` publishes
/// (`smithy_query(query="shape",shape="RedactedFields")`: `@length(0..100)`).
const MIN_REDACTED_FIELDS: usize = 0;
/// See [`MIN_REDACTED_FIELDS`].
const MAX_REDACTED_FIELDS: usize = 100;

/// One `RedactedFields` list to put in front of a web ACL and read back.
struct RedactionCase {
    case: &'static str,
    /// The web ACL's `Scope`.
    scope: &'static str,
    /// The `RedactedFields` list, or `None` to leave the member out of the request.
    redacted: Option<Vec<Value>>,
}

/// A `SingleHeader` redaction naming `header`.
fn single_header(header: &str) -> Value {
    json!({ "SingleHeader": { "Name": header } })
}

/// The rows: the member absent and present-but-empty, each redactable component on
/// its own, all four at once, two header entries in either order, and both scopes.
fn redaction_cases() -> Vec<RedactionCase> {
    vec![
        RedactionCase {
            case: "no redacted fields named at all",
            scope: "REGIONAL",
            redacted: None,
        },
        RedactionCase {
            case: "an empty list, the published element-count floor",
            scope: "REGIONAL",
            redacted: Some(vec![]),
        },
        RedactionCase {
            case: "the URI path alone",
            scope: "REGIONAL",
            redacted: Some(vec![json!({ "UriPath": {} })]),
        },
        RedactionCase {
            case: "the query string alone",
            scope: "CLOUDFRONT",
            redacted: Some(vec![json!({ "QueryString": {} })]),
        },
        RedactionCase {
            case: "the HTTP method alone",
            scope: "REGIONAL",
            redacted: Some(vec![json!({ "Method": {} })]),
        },
        RedactionCase {
            case: "one header, by name",
            scope: "REGIONAL",
            redacted: Some(vec![single_header("authorization")]),
        },
        RedactionCase {
            case: "every component the member admits for redaction",
            scope: "CLOUDFRONT",
            redacted: Some(vec![
                json!({ "UriPath": {} }),
                json!({ "QueryString": {} }),
                single_header("cookie"),
                json!({ "Method": {} }),
            ]),
        },
        RedactionCase {
            case: "two headers, in the order given",
            scope: "REGIONAL",
            redacted: Some(vec![single_header("x-api-key"), single_header("referer")]),
        },
        RedactionCase {
            case: "the same two headers in the opposite order",
            scope: "REGIONAL",
            redacted: Some(vec![single_header("referer"), single_header("x-api-key")]),
        },
    ]
}

/// The `LoggingConfiguration` a row puts in front of `resource_arn`.
fn configuration(resource_arn: &str, redacted: Option<&Vec<Value>>) -> Value {
    let mut built = json!({
        "ResourceArn": resource_arn,
        "LogDestinationConfigs": [DESTINATION],
    });
    if let Some(redacted) = redacted {
        built["RedactedFields"] = json!(redacted);
    }
    built
}

/// The `PutLoggingConfiguration` request body carrying `configuration`.
fn put_request(configuration: Value) -> Value {
    json!({ "LoggingConfiguration": configuration })
}

/// The configuration `ListLoggingConfigurations` reports for `resource_arn` in
/// `scope`, or `Value::Null` when it reports none.
async fn listed(waf: &Waf, scope: &str, resource_arn: &str) -> Value {
    let page = waf
        .ok("ListLoggingConfigurations", json!({ "Scope": scope }))
        .await;
    page["LoggingConfigurations"]
        .as_array()
        .expect("LoggingConfigurations")
        .iter()
        .find(|entry| entry["ResourceArn"] == json!(resource_arn))
        .cloned()
        .unwrap_or(Value::Null)
}

/// A put stores `RedactedFields` as given, every reader answers it back, and a put
/// that leaves the member out replaces it away.
#[tokio::test]
async fn redacted_fields_round_trip_through_a_put_and_its_readers() {
    for row in redaction_cases() {
        let waf = Waf::new();
        let (_acl_id, acl_arn, _acl_lock) =
            create_web_acl(&waf, row.scope, "redacted-acl", json!([])).await;

        let put = waf
            .ok(
                "PutLoggingConfiguration",
                put_request(configuration(&acl_arn, row.redacted.as_ref())),
            )
            .await;
        let answered = put["LoggingConfiguration"].clone();
        let expected = row
            .redacted
            .as_ref()
            .map_or(Value::Null, |redacted| json!(redacted));
        assert_eq!(
            answered["RedactedFields"], expected,
            "case {:?}: the put must answer the redacted fields as given, got {put}",
            row.case
        );

        // Every reader answers exactly the stored configuration, redaction list
        // included — the list is state, not something the put composed for its own
        // answer.
        let got = waf
            .ok(
                "GetLoggingConfiguration",
                json!({ "ResourceArn": &acl_arn }),
            )
            .await;
        assert_eq!(
            got["LoggingConfiguration"], answered,
            "case {:?}: the read must answer the stored configuration, got {got}",
            row.case
        );
        let enumerated = listed(&waf, row.scope, &acl_arn).await;
        assert_eq!(
            enumerated, answered,
            "case {:?}: the listing must report the stored configuration, got \
             {enumerated}",
            row.case
        );

        // Reading changed nothing a later caller sees.
        let again = waf
            .ok(
                "GetLoggingConfiguration",
                json!({ "ResourceArn": &acl_arn }),
            )
            .await;
        assert_eq!(
            again, got,
            "case {:?}: reading a configuration must not change its redacted fields, \
             got {again}",
            row.case
        );

        // A put is a complete replacement: one that names no redacted fields leaves
        // the web ACL redacting nothing, whatever the previous put stored.
        waf.ok(
            "PutLoggingConfiguration",
            put_request(configuration(&acl_arn, None)),
        )
        .await;
        let replaced = waf
            .ok(
                "GetLoggingConfiguration",
                json!({ "ResourceArn": &acl_arn }),
            )
            .await;
        assert!(
            replaced["LoggingConfiguration"]["RedactedFields"].is_null(),
            "case {:?}: a put naming no redacted fields must replace the list away, \
             got {replaced}",
            row.case
        );

        // And a put can put a list back in front of the same web ACL.
        let restored = waf
            .ok(
                "PutLoggingConfiguration",
                put_request(configuration(&acl_arn, Some(&vec![single_header("cookie")]))),
            )
            .await;
        assert_eq!(
            restored["LoggingConfiguration"]["RedactedFields"],
            json!([single_header("cookie")]),
            "case {:?}: a later put must store the list it names, got {restored}",
            row.case
        );
    }
}

/// The element-count bound `RedactedFields` publishes holds on each bound and one
/// past the ceiling.
#[tokio::test]
async fn the_redacted_field_count_bound_holds_on_its_published_bounds() {
    let waf = Waf::new();
    let (_acl_id, acl_arn, _acl_lock) =
        create_web_acl(&waf, "REGIONAL", "redaction-bounds-acl", json!([])).await;

    // A distinct entry per element, so a list of `count` of them is a list of
    // `count` different redactions rather than one repeated.
    let fields = |count: usize| -> Vec<Value> {
        (0..count)
            .map(|index| single_header(&format!("x-probe-header-{index}")))
            .collect()
    };

    for count in [MIN_REDACTED_FIELDS, 1, MAX_REDACTED_FIELDS] {
        let redacted = fields(count);
        let put = waf
            .ok(
                "PutLoggingConfiguration",
                put_request(configuration(&acl_arn, Some(&redacted))),
            )
            .await;
        let answered = put["LoggingConfiguration"]["RedactedFields"].clone();
        if count == MIN_REDACTED_FIELDS {
            assert_eq!(
                answered,
                json!([]),
                "case {count} redacted fields: the floor must be stored as the empty \
                 list, got {put}"
            );
        } else {
            assert_eq!(
                answered,
                json!(redacted),
                "case {count} redacted fields: the list must be stored as given, got \
                 {put}"
            );
        }
        let got = waf
            .ok(
                "GetLoggingConfiguration",
                json!({ "ResourceArn": &acl_arn }),
            )
            .await;
        assert_eq!(
            got["LoggingConfiguration"]["RedactedFields"], answered,
            "case {count} redacted fields: the read must answer the stored list, got \
             {got}"
        );
    }

    // One past the ceiling is outside the modelled constraint, so the request is
    // refused and the configuration the web ACL carries is untouched.
    let stored = fields(MAX_REDACTED_FIELDS);
    let too_many = fields(MAX_REDACTED_FIELDS + 1);
    let (status, body) = waf
        .call(
            "PutLoggingConfiguration",
            put_request(configuration(&acl_arn, Some(&too_many))),
        )
        .await;
    assert_eq!(
        status,
        400,
        "case {} redacted fields: expected 400, got status {status} and body {body}",
        too_many.len()
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {} redacted fields: a member-constraint failure is the request \
         validator's own refusal, got {body}",
        too_many.len()
    );
    assert!(
        error_message(&body).contains("redactedFields"),
        "case {} redacted fields: the clause must name the member, got {:?}",
        too_many.len(),
        error_message(&body)
    );
    let got = waf
        .ok(
            "GetLoggingConfiguration",
            json!({ "ResourceArn": &acl_arn }),
        )
        .await;
    assert_eq!(
        got["LoggingConfiguration"]["RedactedFields"],
        json!(stored),
        "a refused put must leave the stored redaction list in place, got {got}"
    );
}
