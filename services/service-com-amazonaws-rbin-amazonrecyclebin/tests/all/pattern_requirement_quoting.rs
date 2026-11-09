// SPDX-License-Identifier: CC0-1.0

//! RULE: a validation clause quotes a `@pattern` requirement as the expression the
//! service publishes for the member that failed, which follows from WHERE the request
//! carried that member:
//!
//! * a member bound into the URI PATH (`@httpLabel`) is matched as ONE path segment of
//!   the route, so what its value is held to is the member's expression spliced into the
//!   route's — the modelled one without its whole-value `^`/`$` anchors, and that is the
//!   expression quoted back;
//! * a member the request carries in its BODY or its query string is matched against the
//!   member's own expression, so the modelled `@pattern` is quoted exactly as the model
//!   declares it, anchors included.
//!
//! The table spans every `@pattern` this service's input shapes declare, reached through
//! each of the three HTTP locations a value can arrive in and at every depth a member can
//! sit at (top level, inside a list, inside a structure inside a list) — not only the two
//! members whose refusals are recorded. Each row states the expression the clause must
//! quote, taken from the model.
//!
//! Every case drives the service through the real router, so the wire protocol, the route
//! table and the input-validation gate are exercised exactly as a signed SDK request
//! would exercise them.

use serde_json::{json, Value};

use crate::error_response_bodies::{error_code, message, one_validation_error};
use crate::resource_tags::percent_encode;
use crate::retention_rule_lifecycle::{call, create_rule, retention, router};

/// The expression each patterned member declares, as the MODEL writes it.
/// Source: smithy_query(query="shape",
/// shape="RuleIdentifier,RuleArn,NextToken,Description,TagKey,TagValue,ResourceTagKey,ResourceTagValue").
const RULE_IDENTIFIER: &str = "^[0-9a-zA-Z]{11}$";
const RULE_ARN: &str = r"^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}{0,1011}$";
const NEXT_TOKEN: &str = "^[A-Za-z0-9+/=]{1,2048}$";
const DESCRIPTION: &str = r"^[\S ]{0,255}$";
/// `TagKey` and `TagValue` declare the same expression.
const TAG_HALF: &str = r"^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$";
const RESOURCE_TAG_KEY: &str = r"^[\S\s]{1,128}$";
const RESOURCE_TAG_VALUE: &str = r"^[\S\s]{0,256}$";

/// Where the request carried the member whose `@pattern` was broken — the one fact the
/// quoted expression turns on.
/// Source: smithy_query(query="shape", shape="GetRuleRequest,UpdateRuleRequest,
/// TagResourceRequest,UntagResourceRequest,ListRulesRequest,CreateRuleRequest") —
/// `Identifier` and `ResourceArn` carry `@httpLabel`, `TagKeys` carries
/// `@httpQuery("tagKeys")`, and every other member carries no HTTP binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    /// `@httpLabel`: one segment of the route's path.
    UriPath,
    /// `@httpQuery`: a query-string pair.
    UriQuery,
    /// No HTTP binding: the JSON body.
    Body,
}

impl Location {
    /// The expression a clause quotes for a member carried here, given the one the model
    /// declares.
    fn quoted(self, modelled: &str) -> String {
        match self {
            // The route's own expression is what a path segment is matched against, so
            // the member's anchors are not part of it.
            Self::UriPath => modelled
                .strip_prefix('^')
                .and_then(|body| body.strip_suffix('$'))
                .unwrap_or(modelled)
                .to_string(),
            Self::UriQuery | Self::Body => modelled.to_string(),
        }
    }
}

/// One row: a request that breaks exactly one `@pattern`, and what the refusal must say.
struct PatternRow {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause names, when the broken member sits at the top level of
    /// the input shape. `None` where it sits inside a collection, whose path spelling is
    /// the framework's and not this service's.
    member_path: Option<&'static str>,
    location: Location,
    modelled: &'static str,
}

/// A rule ARN of this Region and account naming `identifier` — well formed, so every row
/// below is refused for the `@pattern` it breaks and for nothing about the rule.
fn rule_arn(identifier: &str) -> String {
    format!(
        "arn:aws:rbin:{}:{}:rule/{identifier}",
        crate::retention_rule_lifecycle::REGION,
        engine::EMULATOR_ACCOUNT_ID
    )
}

#[tokio::test]
async fn a_pattern_requirement_is_quoted_as_the_service_publishes_it() {
    let router = router();
    // A rule that exists, so no row's refusal can be mistaken for a missing resource.
    let (status, created) = create_rule(
        &router,
        json!({ "ResourceType": "EBS_SNAPSHOT", "RetentionPeriod": retention(7) }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    let held = created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateRule returned no Identifier: {created:?}"))
        .to_string();
    let held_arn = rule_arn(&held);

    // A value too long for the expression it must match: the two resource-tag halves
    // accept any CHARACTER, so length is the only way to break their pattern.
    let over_resource_tag_key = "k".repeat(129);
    let over_resource_tag_value = "v".repeat(257);

    let rows: Vec<PatternRow> = vec![
        PatternRow {
            case: "GetRule: an identifier outside the pattern's alphabet",
            method: "GET",
            path: "/rules/not-an-id!!!".to_string(),
            body: None,
            member_path: Some("identifier"),
            location: Location::UriPath,
            modelled: RULE_IDENTIFIER,
        },
        PatternRow {
            case: "UpdateRule: an identifier longer than the pattern allows",
            method: "PATCH",
            path: "/rules/000000000000".to_string(),
            body: Some(json!({ "Description": "fine" })),
            member_path: Some("identifier"),
            location: Location::UriPath,
            modelled: RULE_IDENTIFIER,
        },
        PatternRow {
            case: "ListTagsForResource: a resource ARN of another service",
            method: "GET",
            path: format!(
                "/tags/{}",
                percent_encode("arn:aws:ec2:eu-west-2:111111111111:volume/vol-01234567")
            ),
            body: None,
            member_path: Some("resourceArn"),
            location: Location::UriPath,
            modelled: RULE_ARN,
        },
        PatternRow {
            case: "TagResource: a resource ARN that is not an ARN at all",
            method: "POST",
            path: format!("/tags/{}", percent_encode("not-an-arn")),
            body: Some(json!({ "Tags": [{ "Key": "env", "Value": "dev" }] })),
            member_path: Some("resourceArn"),
            location: Location::UriPath,
            modelled: RULE_ARN,
        },
        PatternRow {
            case: "ListRules: a page token outside its pattern's alphabet",
            method: "POST",
            path: "/list-rules".to_string(),
            body: Some(json!({ "ResourceType": "EBS_SNAPSHOT", "NextToken": "not a token!" })),
            member_path: Some("nextToken"),
            location: Location::Body,
            modelled: NEXT_TOKEN,
        },
        PatternRow {
            case: "CreateRule: a description holding a character its pattern forbids",
            method: "POST",
            path: "/rules".to_string(),
            body: Some(json!({
                "ResourceType": "EBS_VOLUME",
                "RetentionPeriod": retention(7),
                "Description": "two\tcolumns",
            })),
            member_path: Some("description"),
            location: Location::Body,
            modelled: DESCRIPTION,
        },
        PatternRow {
            case: "UpdateRule: a description holding a character its pattern forbids",
            method: "PATCH",
            path: format!("/rules/{held}"),
            body: Some(json!({ "Description": "two\tcolumns" })),
            member_path: Some("description"),
            location: Location::Body,
            modelled: DESCRIPTION,
        },
        PatternRow {
            case: "CreateRule: a tag key outside its pattern, inside the tag list",
            method: "POST",
            path: "/rules".to_string(),
            body: Some(json!({
                "ResourceType": "EC2_IMAGE",
                "RetentionPeriod": retention(7),
                "Tags": [{ "Key": "bad?key", "Value": "v" }],
            })),
            member_path: None,
            location: Location::Body,
            modelled: TAG_HALF,
        },
        PatternRow {
            case: "TagResource: a tag value outside its pattern, inside the tag list",
            method: "POST",
            path: format!("/tags/{}", percent_encode(&held_arn)),
            body: Some(json!({ "Tags": [{ "Key": "env", "Value": "bad?value" }] })),
            member_path: None,
            location: Location::Body,
            modelled: TAG_HALF,
        },
        PatternRow {
            case: "CreateRule: a resource tag key longer than its pattern allows",
            method: "POST",
            path: "/rules".to_string(),
            body: Some(json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(7),
                "ResourceTags": [{ "ResourceTagKey": over_resource_tag_key }],
            })),
            member_path: None,
            location: Location::Body,
            modelled: RESOURCE_TAG_KEY,
        },
        PatternRow {
            case: "CreateRule: a resource tag value longer than its pattern allows",
            method: "POST",
            path: "/rules".to_string(),
            body: Some(json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(7),
                "ResourceTags": [{
                    "ResourceTagKey": "retain",
                    "ResourceTagValue": over_resource_tag_value,
                }],
            })),
            member_path: None,
            location: Location::Body,
            modelled: RESOURCE_TAG_VALUE,
        },
        PatternRow {
            case: "UntagResource: a tag key outside its pattern, in the query string",
            method: "DELETE",
            path: format!(
                "/tags/{}?tagKeys={}",
                percent_encode(&held_arn),
                percent_encode("bad?key")
            ),
            body: None,
            member_path: None,
            location: Location::UriQuery,
            modelled: TAG_HALF,
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, body) = call(&router, row.method, &row.path, row.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {body:?}"
        );
        let requirement = format!(
            "Member must satisfy regular expression pattern: {}",
            row.location.quoted(row.modelled)
        );
        let stated = message(&body);
        assert!(
            stated.ends_with(&requirement),
            "case {case:?}: expected the clause to state {requirement:?}, got {stated:?}"
        );
        if let Some(member_path) = row.member_path {
            let expected = one_validation_error(member_path, &requirement);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected Message {expected:?}, got {stated:?}"
            );
        }
    }
}

/// The rule's own bound: the expression a clause quotes is the MODEL's, so a caller that
/// takes it from one refusal and sends a value matching it is refused no longer.
///
/// Asserted over the two members whose patterns a caller can satisfy with a value of its
/// own choosing, in the two locations — so the anchored and the unanchored rendering are
/// each shown to describe the values the service accepts.
#[tokio::test]
async fn a_value_matching_the_quoted_expression_is_accepted() {
    let router = router();
    let (status, created) = create_rule(
        &router,
        json!({
            "ResourceType": "EBS_SNAPSHOT",
            "RetentionPeriod": retention(7),
            // Every character here is `\S` or a space, as `Description`'s quoted
            // expression requires.
            "Description": "retains snapshots (prod)",
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"a description its quoted expression admits\": expected HTTP 201, got {status} / \
         {created:?}"
    );
    let held = created["Identifier"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The identifier the create handed back is 11 alphanumerics — exactly what the
    // expression quoted for the URI-path member describes — so addressing the rule with
    // it is accepted.
    let (status, fetched) = call(&router, "GET", &format!("/rules/{held}"), None).await;
    assert_eq!(
        status, 200,
        "case \"the minted identifier in the URI path\": expected HTTP 200, got {status} / \
         {fetched:?}"
    );
    assert_eq!(
        fetched["Description"].as_str(),
        Some("retains snapshots (prod)"),
        "case \"the minted identifier in the URI path\": expected the stored description, got {:?}",
        fetched["Description"]
    );
}
