// SPDX-License-Identifier: CC0-1.0

//! WHICH regular expression a body refusal of this service quotes.
//!
//! The rule under test: the regex clause names the expression the request-body validator
//! MEASURES the member against, which for this service's general-purpose string type —
//! the type every name, client token and tag value a request body carries targets — is a
//! non-empty run of printable ASCII with a few characters excluded, and not the
//! "any non-whitespace run" expression the model publishes for that type, which only
//! generalises it. A member whose declared expression is already a concrete character
//! class (the ARN members) is quoted as the model writes it, because that is the class
//! the validator applies to it.
//!
//! Why it matters: the quoted expression is what a client repairs a rejected value
//! against, so an expression wider than the one the service applies sends it back with
//! another value the service refuses.
//!
//! The table spans what the rule generalises over rather than the one request that
//! exposed it: nine operations, three HTTP methods, every member kind a body carries (a
//! plain member, a map value, a member two structures deep, a member on a create and on
//! an update), both string types, and several forms of value that miss the class — four
//! kinds of whitespace, whitespace at either end, and the empty string.
//!
//! Its mirrors are `tests/all/rejection_error_classes.rs` (the whole sentence, and the
//! `@length` classes) and `tests/all/body_enum_refusals.rs` (the enumerated class).

use serde_json::{json, Value};

use super::common::{call, create_cluster, enc, error_code, router};

/// The expression the request-body validator measures a string member of this service's
/// general-purpose string type against, and therefore quotes: printable ASCII
/// (U+0021..U+007E), one character or more, less the characters it excludes. Restated here
/// independently of the service's own code.
const VALIDATED_STRING_CLASS: &str = r#"^((?![;'\s<>&"])[\u0021-\u007E])+$"#;

/// The two expressions the MODEL publishes for that type — `__stringMin1Max64PatternS`
/// and `__stringMin0Max256PatternS` carry the first, `__stringMin1Max8096PatternS` the
/// second (`smithy_query(query="shape",shape="__stringMin1Max64PatternS")`). Both spell
/// "a run of non-whitespace characters", which is wider than the class above, so neither
/// may appear in a refusal.
const PUBLISHED_STRING_SPELLINGS: [&str; 2] = [r"^\S+$", r"^[\S]*$"];

/// The expression the model declares for the ARN members a request body carries
/// (`__stringMin1Max256PatternAZaZ09`,
/// `smithy_query(query="shape",shape="__stringMin1Max256PatternAZaZ09")`) — already a
/// concrete character class, which the validator applies and quotes as published.
const ARN_CLASS: &str = r"^[A-Za-z0-9:\/_-]*$";

/// A syntactically well-formed ARN of each kind, for the members an update requires
/// alongside the one under test. The door refuses the body before anything is resolved,
/// so these need name no live resource.
const SOME_PANEL_ARN: &str =
    "arn:aws:route53-recovery-control::000000000000:controlpanel/0123456789abcdef";
const SOME_CONTROL_ARN: &str = "arn:aws:route53-recovery-control::000000000000:controlpanel/\
                                0123456789abcdef/routingcontrol/0123456789abcdef";
const SOME_RULE_ARN: &str = "arn:aws:route53-recovery-control::000000000000:controlpanel/\
                             0123456789abcdef/safetyrule/0123456789abcdef";

/// The whole message a refusal quoting `expression` about `value` carries, restated here
/// independently of the service's own code.
fn regex_clause(expression: &str, value: &str) -> String {
    format!(
        "Invalid request body : [ECMA 262 regex \"{expression}\" does not match input string \
         \"{value}\"]"
    )
}

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes (`smithy_query(query="shape",shape="ValidationException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One request to make, with the value its refusal must quote and the expression it must
/// be measured against.
struct Case {
    label: &'static str,
    method: &'static str,
    path: String,
    body: Value,
    value: &'static str,
    expression: &'static str,
}

/// RULE: a body refusal quotes the expression the validator measures the member against —
/// the printable-ASCII class for this service's general-purpose string type, the declared
/// character class for an ARN member — whichever operation, method, depth and member kind
/// the value sits at.
#[tokio::test]
async fn a_body_refusal_quotes_the_expression_the_validator_applies() {
    let router = router();

    let cases: Vec<Case> = vec![
        Case {
            label: "ClusterName with an inner space, on a create",
            method: "POST",
            path: "/cluster".to_string(),
            body: json!({ "ClusterName": "has a space" }),
            value: "has a space",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "ClusterName with a leading space, on a create",
            method: "POST",
            path: "/cluster".to_string(),
            body: json!({ "ClusterName": " leading" }),
            value: " leading",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "an empty ClusterName, on a create",
            method: "POST",
            path: "/cluster".to_string(),
            body: json!({ "ClusterName": "" }),
            value: "",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "ClientToken with a tab, on a create",
            method: "POST",
            path: "/cluster".to_string(),
            body: json!({ "ClusterName": "tokened", "ClientToken": "two\twords" }),
            value: "two\twords",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            // A MAP value, reached through the map member rather than declared on it.
            label: "a Tags value with a trailing newline, on a create",
            method: "POST",
            path: "/cluster".to_string(),
            body: json!({ "ClusterName": "tagged", "Tags": { "team": "trailing\n" } }),
            value: "trailing\n",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "a Tags value with a space, on TagResource",
            method: "POST",
            path: format!("/tags/{}", enc(SOME_PANEL_ARN)),
            body: json!({ "Tags": { "team": "two words" } }),
            value: "two words",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "ControlPanelName with a space, on a create",
            method: "POST",
            path: "/controlpanel".to_string(),
            body: json!({ "ClusterArn": SOME_PANEL_ARN, "ControlPanelName": "has a space" }),
            value: "has a space",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "ControlPanelName with a carriage return, on an update",
            method: "PUT",
            path: "/controlpanel".to_string(),
            body: json!({
                "ControlPanelArn": SOME_PANEL_ARN,
                "ControlPanelName": "two\rlines"
            }),
            value: "two\rlines",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "RoutingControlName with a space, on a create",
            method: "POST",
            path: "/routingcontrol".to_string(),
            body: json!({ "ClusterArn": SOME_PANEL_ARN, "RoutingControlName": "has a space" }),
            value: "has a space",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "RoutingControlName with a form feed, on an update",
            method: "PUT",
            path: "/routingcontrol".to_string(),
            body: json!({
                "RoutingControlArn": SOME_CONTROL_ARN,
                "RoutingControlName": "two\u{000C}parts"
            }),
            value: "two\u{000C}parts",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            // Two structures deep in the body: the expression comes from the member's own
            // type, so depth changes nothing.
            label: "an assertion rule's Name with a space, on a create",
            method: "POST",
            path: "/safetyrule".to_string(),
            body: json!({
                "AssertionRule": {
                    "AssertedControls": [SOME_CONTROL_ARN],
                    "ControlPanelArn": SOME_PANEL_ARN,
                    "Name": "has a space",
                    "RuleConfig": { "Inverted": false, "Threshold": 1, "Type": "ATLEAST" },
                    "WaitPeriodMs": 5000
                }
            }),
            value: "has a space",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "a gating rule's Name with a space, on a create",
            method: "POST",
            path: "/safetyrule".to_string(),
            body: json!({
                "GatingRule": {
                    "ControlPanelArn": SOME_PANEL_ARN,
                    "GatingControls": [SOME_CONTROL_ARN],
                    "Name": "has a space",
                    "RuleConfig": { "Inverted": false, "Threshold": 1, "Type": "ATLEAST" },
                    "TargetControls": [SOME_CONTROL_ARN],
                    "WaitPeriodMs": 5000
                }
            }),
            value: "has a space",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            label: "a safety rule update's Name with a space, on an update",
            method: "PUT",
            path: "/safetyrule".to_string(),
            body: json!({
                "AssertionRuleUpdate": {
                    "Name": "has a space",
                    "SafetyRuleArn": SOME_RULE_ARN,
                    "WaitPeriodMs": 5000
                }
            }),
            value: "has a space",
            expression: VALIDATED_STRING_CLASS,
        },
        Case {
            // The OTHER string type: its declared expression is the one the validator
            // applies, so it is quoted as the model writes it.
            label: "a ClusterArn outside the ARN class, on an update",
            method: "PUT",
            path: "/cluster".to_string(),
            body: json!({ "ClusterArn": "not an arn!", "NetworkType": "IPV4" }),
            value: "not an arn!",
            expression: ARN_CLASS,
        },
        Case {
            label: "a ClusterArn outside the ARN class, on a create",
            method: "POST",
            path: "/controlpanel".to_string(),
            body: json!({ "ClusterArn": "arn:aws:route53?", "ControlPanelName": "panel" }),
            value: "arn:aws:route53?",
            expression: ARN_CLASS,
        },
        Case {
            label: "an assertion rule's ControlPanelArn outside the ARN class, nested",
            method: "POST",
            path: "/safetyrule".to_string(),
            body: json!({
                "AssertionRule": {
                    "AssertedControls": [SOME_CONTROL_ARN],
                    "ControlPanelArn": "arn:aws:route53-recovery-control!",
                    "Name": "assertion",
                    "RuleConfig": { "Inverted": false, "Threshold": 1, "Type": "ATLEAST" },
                    "WaitPeriodMs": 5000
                }
            }),
            value: "arn:aws:route53-recovery-control!",
            expression: ARN_CLASS,
        },
    ];

    for case in &cases {
        let (status, answer) = call(&router, case.method, &case.path, Some(case.body.clone())).await;
        assert_eq!(
            status, 400,
            "case {}: expected 400, got {status} {answer:?}",
            case.label
        );
        assert_eq!(
            error_code(&answer),
            "BadRequestException",
            "case {}: expected BadRequestException, got {answer:?}",
            case.label
        );
        let actual = message(&answer);
        let expected = regex_clause(case.expression, case.value);
        assert_eq!(
            actual, expected,
            "case {}: expected {expected:?}, got {actual:?}",
            case.label
        );
        for published in PUBLISHED_STRING_SPELLINGS {
            assert!(
                !actual.contains(published),
                "case {}: a refusal must quote the expression the validator applies, not the \
                 published {published:?} it generalises; got {actual:?}",
                case.label
            );
        }
    }
}

/// RULE, the other way round: the class a refusal advertises is one this service accepts,
/// so a value built from it passes the door — which is what makes the quoted expression
/// usable for repairing a rejected one.
///
/// Rows span the printable-ASCII range the class admits in four chunks, each read back
/// through `DescribeCluster` so the accepted value is the stored one. The excluded
/// characters (`;`, `'`, `<`, `>`, `&`) appear in none of them.
#[tokio::test]
async fn a_name_inside_the_advertised_class_is_accepted_and_stored() {
    let router = router();

    // (case, name) — printable ASCII U+0021..U+007E in order, less the excluded five and
    // less `"`, which the class excludes too.
    let names: [(&str, &str); 4] = [
        ("punctuation", "!#$%()*+,-./"),
        ("digits and the ARN separators", "0123456789:/"),
        ("upper case with brackets", "=?@ABCYZ[]^_`"),
        ("lower case with braces", "abcyz{|}~"),
    ];

    for (case, name) in names {
        let created = create_cluster(&router, json!({ "ClusterName": name })).await;
        assert_eq!(
            created["Name"],
            json!(name),
            "case a ClusterName of {case}: expected it stored, got {:?}",
            created["Name"]
        );
        let arn = created["ClusterArn"].as_str().unwrap_or_default().to_string();
        let (status, described) = call(&router, "GET", &format!("/cluster/{}", enc(&arn)), None).await;
        assert_eq!(
            status, 200,
            "case a ClusterName of {case}: expected DescribeCluster 200, got {status} \
             {described:?}"
        );
        assert_eq!(
            described["Cluster"]["Name"],
            json!(name),
            "case a ClusterName of {case}: expected DescribeCluster to report the stored name, \
             got {:?}",
            described["Cluster"]["Name"]
        );
    }
}
