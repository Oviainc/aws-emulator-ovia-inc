// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Region segment of the ARNs this service mints.
//!
//! The rule asserted here: the Region a request is served in is the Region its SigV4
//! credential scope names only when that is shaped like an Amazon Web Services Region
//! code — lowercase letters, digits and hyphens, within the length the modelled
//! `RuleArn` Region segment admits (`^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}$`,
//! from smithy_query(query="shape", shape="RuleArn")) — and otherwise the emulator's
//! default Region, so every ARN this service reports satisfies the pattern of the member
//! it is reported through and is a value the ARN-addressed operations can be asked for.
//!
//! The table below spans the dimension the rule generalises over: the credential scope's
//! region field — its character class (upper case, underscore, space, dot, the ARN field
//! separator), its length at and one past the published bound, and its absence
//! altogether. Every row drives the service through the real router
//! (`MultiServiceRouter::dispatch`), exactly as a signed SDK request would, and states
//! its own case so a failure names the input.

use serde_json::{json, Value};

use crate::resource_tags::percent_encode;
use crate::retention_rule_lifecycle::{retention, router, RESOURCE_TYPES};
use service_com_amazonaws_rbin_amazonrecyclebin::state::{DEFAULT_REGION, REGION_CODE_MAX_LEN};
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint every row addresses. The Region under test travels in the credential
/// scope alone — the one place this service reads a request's Region from — so the host
/// is held fixed, which is also what lets a row carry a scope region no host could spell
/// (a space, a colon).
const ENDPOINT: &str = "https://rbin.us-east-1.amazonaws.com";

/// The index of the Region segment of an ARN: `arn:<partition>:<service>:<region>:…`.
const ARN_REGION_SEGMENT: usize = 3;

/// The number of colon-separated sections of an ARN, the last of which is the resource.
const ARN_SECTIONS: usize = 6;

/// The modelled `RuleIdentifier` length (`^[0-9a-zA-Z]{11}$`), which closes a `RuleArn`.
const IDENTIFIER_LEN: usize = 11;

/// The length the modelled `RuleArn` bounds the whole ARN by (len 0..1011, from
/// smithy_query(query="shape", shape="RuleArn")).
const RULE_ARN_MAX_LEN: usize = 1011;

/// A Region-code-shaped value of exactly `len` characters: lowercase letters closed by a
/// Region number, so only the LENGTH distinguishes it from a published Region code.
fn region_of_length(len: usize) -> String {
    format!("{}-1", "a".repeat(len - 2))
}

/// Whether `rule_arn` satisfies the modelled `RuleArn` pattern
/// `^arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}$` and its length bound
/// (both from smithy_query(query="shape", shape="RuleArn")), checked section by section
/// rather than with a regex engine this crate does not depend on.
fn satisfies_rule_arn_pattern(rule_arn: &str) -> bool {
    if rule_arn.len() > RULE_ARN_MAX_LEN {
        return false;
    }
    let sections: Vec<&str> = rule_arn.splitn(ARN_SECTIONS, ':').collect();
    if sections.len() != ARN_SECTIONS {
        return false;
    }
    let [arn, partition, service, region, account, resource] = sections[..] else {
        return false;
    };
    let identifier = match resource.strip_prefix("rule/") {
        Some(identifier) => identifier,
        None => return false,
    };
    arn == "arn"
        // `\S+`: one or more non-whitespace characters.
        && !partition.is_empty()
        && !partition.contains(char::is_whitespace)
        && service == "rbin"
        // `[a-z\-0-9]{0,63}`.
        && region.len() <= REGION_CODE_MAX_LEN
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        // `[0-9]{12}`.
        && account.len() == 12
        && account.bytes().all(|byte| byte.is_ascii_digit())
        // `[0-9a-zA-Z]{11}`.
        && identifier.len() == IDENTIFIER_LEN
        && identifier.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// One row of the table: the region field of the request's credential scope (`None` for
/// a request that carries no `Authorization` header at all), and the Region the service
/// must serve that request in.
struct Row {
    case: &'static str,
    scope_region: Option<String>,
    served_in: String,
}

fn rows() -> Vec<Row> {
    let row = |case: &'static str, scope_region: Option<&str>, served_in: &str| Row {
        case,
        scope_region: scope_region.map(str::to_string),
        served_in: served_in.to_string(),
    };
    let at_bound = region_of_length(REGION_CODE_MAX_LEN);
    let past_bound = region_of_length(REGION_CODE_MAX_LEN + 1);
    vec![
        // Region codes the segment's character class admits: served where they name.
        row("a published Region code", Some("us-east-1"), "us-east-1"),
        row(
            "a Region code in another geography",
            Some("ap-southeast-4"),
            "ap-southeast-4",
        ),
        row(
            "a Region code of a non-commercial partition",
            Some("us-gov-east-1"),
            "us-gov-east-1",
        ),
        row(
            "a one-character region, the shortest the segment admits",
            Some("a"),
            "a",
        ),
        Row {
            case: "a region of the longest length the segment admits",
            scope_region: Some(at_bound.clone()),
            served_in: at_bound,
        },
        // Values outside that class or that bound: no Region of this service, so the
        // request is served in the default Region.
        Row {
            case: "a region one character past the length the segment admits",
            scope_region: Some(past_bound),
            served_in: DEFAULT_REGION.to_string(),
        },
        row(
            "an upper-case region",
            Some("US-EAST-1"),
            DEFAULT_REGION,
        ),
        row(
            "a region written with underscores",
            Some("us_east_1"),
            DEFAULT_REGION,
        ),
        row(
            "a region carrying a space",
            Some("eu west 1"),
            DEFAULT_REGION,
        ),
        row(
            "a region carrying an ARN section separator",
            Some("us-east-1:rule"),
            DEFAULT_REGION,
        ),
        row("a region carrying dots", Some("us.east.1"), DEFAULT_REGION),
        row(
            "a region that is a word rather than a code",
            Some("Local"),
            DEFAULT_REGION,
        ),
        row("no credential scope at all", None, DEFAULT_REGION),
    ]
}

/// Dispatch one request whose credential scope names `scope_region` (or that carries no
/// `Authorization` header, for `None`), and return its status and parsed JSON body.
async fn call_with_scope(
    router: &MultiServiceRouter,
    scope_region: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("{ENDPOINT}{path}"))
        .header("content-type", "application/json");
    if let Some(region) = scope_region {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDRBINTEST/20260101/{region}/rbin/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=deadbeef"
            ),
        );
    }
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path} answered {status} with a non-JSON body ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The Region segment of an ARN.
fn arn_region(rule_arn: &str) -> &str {
    rule_arn
        .split(':')
        .nth(ARN_REGION_SEGMENT)
        .unwrap_or_else(|| panic!("an rbin rule ARN carries a Region segment, got {rule_arn:?}"))
}

/// Create one retention rule carrying one tag through `scope_region`'s request, and hand
/// back its `(Identifier, RuleArn)`.
async fn created_rule(
    router: &MultiServiceRouter,
    scope_region: Option<&str>,
    case: &str,
) -> (String, String) {
    let (status, created) = call_with_scope(
        router,
        scope_region,
        "POST",
        "/rules",
        Some(json!({
            "ResourceType": RESOURCE_TYPES[0],
            "RetentionPeriod": retention(7),
            "Tags": [{ "Key": "env", "Value": "dev" }],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    let identifier = created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier: {created:?}"))
        .to_string();
    let rule_arn = created["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no RuleArn: {created:?}"))
        .to_string();
    (identifier, rule_arn)
}

#[tokio::test]
async fn a_minted_rule_arn_names_a_region_the_modelled_pattern_admits() {
    for Row {
        case,
        scope_region,
        served_in,
    } in rows()
    {
        let router = router();
        let (identifier, rule_arn) = created_rule(&router, scope_region.as_deref(), case).await;
        assert!(
            satisfies_rule_arn_pattern(&rule_arn),
            "case {case:?}: the reported RuleArn must satisfy the modelled pattern, \
             got {rule_arn:?}"
        );
        assert_eq!(
            arn_region(&rule_arn),
            served_in,
            "case {case:?}: the ARN must name the Region the request is served in, \
             got {rule_arn:?}"
        );

        // The rule really belongs to that Region: the endpoint signed for it reports the
        // rule, so the Region in the ARN is the Region every operation resolves it
        // through — not a string only the ARN carries.
        let (status, listed) = call_with_scope(
            &router,
            Some(&served_in),
            "POST",
            "/list-rules",
            Some(json!({ "ResourceType": RESOURCE_TYPES[0] })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListRules expected HTTP 200, got {status} / {listed:?}"
        );
        let reported: Vec<&str> = listed["Rules"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: ListRules reported no Rules: {listed:?}"))
            .iter()
            .map(|summary| summary["Identifier"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported,
            vec![identifier.as_str()],
            "case {case:?}: the Region the ARN names must report the rule, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_arn_a_create_reports_is_one_every_tag_operation_serves() {
    for Row {
        case,
        scope_region,
        served_in,
    } in rows()
    {
        let router = router();
        let scope = scope_region.as_deref();
        let (identifier, rule_arn) = created_rule(&router, scope, case).await;
        let arn_path = format!("/tags/{}", percent_encode(&rule_arn));

        // The rule is reachable by its identifier, through the Region it was served in.
        let (status, fetched) = call_with_scope(
            &router,
            scope,
            "GET",
            &format!("/rules/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["RuleArn"].as_str(),
            Some(rule_arn.as_str()),
            "case {case:?}: GetRule must report the ARN the create reported, got {:?}",
            fetched["RuleArn"]
        );

        // ListTagsForResource: the ARN the create handed out resolves the rule.
        let (status, tags) = call_with_scope(&router, scope, "GET", &arn_path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected HTTP 200, got {status} / {tags:?}"
        );
        assert_eq!(
            tags["Tags"],
            json!([{ "Key": "env", "Value": "dev" }]),
            "case {case:?}: expected the rule's own tags, got {:?}",
            tags["Tags"]
        );

        // TagResource and UntagResource: the same ARN, assigning and unassigning a tag.
        let (status, assigned) = call_with_scope(
            &router,
            scope,
            "POST",
            &arn_path,
            Some(json!({ "Tags": [{ "Key": "owner", "Value": served_in }] })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: TagResource expected HTTP 201, got {status} / {assigned:?}"
        );
        let (status, listed) = call_with_scope(&router, scope, "GET", &arn_path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected HTTP 200, got {status} / {listed:?}"
        );
        assert_eq!(
            listed["Tags"],
            json!([
                { "Key": "env", "Value": "dev" },
                { "Key": "owner", "Value": served_in },
            ]),
            "case {case:?}: expected the assigned tag beside the created one, got {:?}",
            listed["Tags"]
        );
        let (status, removed) = call_with_scope(
            &router,
            scope,
            "DELETE",
            &format!("{arn_path}?tagKeys={}", percent_encode("owner")),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: UntagResource expected HTTP 204, got {status} / {removed:?}"
        );
        let (status, left) = call_with_scope(&router, scope, "GET", &arn_path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected HTTP 200, got {status} / {left:?}"
        );
        assert_eq!(
            left["Tags"],
            json!([{ "Key": "env", "Value": "dev" }]),
            "case {case:?}: expected the untagged rule to keep its created tag, got {:?}",
            left["Tags"]
        );
    }
}
