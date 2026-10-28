// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the region segment of the `scanNameArn` this service reports.
//!
//! A SigV4 credential scope is request-controlled text, and its region is what the
//! emulator builds a region into a reported value from. The RULE this suite asserts is
//! that a reported value always satisfies its own member's shape: `CreateScan` names the
//! request's own region only when that value has a Region's shape AND is short enough for
//! the ARN to stay inside the `ScanNameArn` bound, and otherwise names the emulator's
//! default Region — so every `scanNameArn` `CreateScan`, `GetScan` and `ListScans` report
//! matches `^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$` within
//! `@length` 1..300 (`smithy_query(query="shape",shape="ScanNameArn")`), and is therefore
//! an ARN the tagging operations accept as their `resourceArn` label.
//!
//! The table's rows differ along what the rule generalises over: the FORM of the scope's
//! region (Region-shaped, whitespace-bearing, uppercase, dotted, missing its digit group,
//! absent) and its LENGTH (both sides of the longest region an ARN can carry), and every
//! row is driven as real HTTP through `MultiServiceRouter::dispatch`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    DEFAULT_REGION, SCAN_NAME_ARN_MAX_LEN, SCAN_NAME_MAX_LEN,
};

use crate::resource_tagging::{list_tags, tag};
use crate::scan_lifecycle::{artifact_for, call_signed_for, expected_scan_arn_in, harness, Answer};

/// The longest region segment a `ScanNameArn` can carry while the whole ARN still
/// satisfies the shape's `@length` maximum: that maximum, less the ARN's fixed text
/// (`arn:aws:codeguru-security:`, the `:` before the account id, and `:scans/`), its
/// 12-digit account id and the longest name a `ScanName` may be — every term read off
/// `smithy_query(query="shape",shape="ScanNameArn")` and `…shape="ScanName"`.
const LONGEST_ARN_REGION: usize = SCAN_NAME_ARN_MAX_LEN
    - "arn:aws:codeguru-security:".len()
    - ":".len()
    - 12
    - ":scans/".len()
    - SCAN_NAME_MAX_LEN;

/// Which region a row expects the reported ARN to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The region of the request's own credential scope.
    Scope,
    /// The emulator's documented default Region, because the scope's region is not one
    /// an ARN may carry.
    Default,
}

/// One row: what the credential scope's region is, and which region the ARN must name.
#[derive(Debug)]
struct Row {
    case: &'static str,
    scope_region: String,
    expect: Expect,
}

fn rows() -> Vec<Row> {
    // A Region-shaped value one character longer than an ARN can carry, and the longest
    // one it can: `<lowercase words>-<digits>`, padded in the first word.
    let longest = format!("{}-b-1", "a".repeat(LONGEST_ARN_REGION - "-b-1".len()));
    let one_too_long = format!("{}-b-1", "a".repeat(LONGEST_ARN_REGION - "-b-1".len() + 1));
    vec![
        Row {
            case: "the suite's own Region",
            scope_region: "us-west-2".to_string(),
            expect: Expect::Scope,
        },
        Row {
            case: "another Region",
            scope_region: "eu-central-1".to_string(),
            expect: Expect::Scope,
        },
        Row {
            case: "a Region of three words",
            scope_region: "us-gov-west-1".to_string(),
            expect: Expect::Scope,
        },
        Row {
            case: "the shortest Region-shaped value",
            scope_region: "a-b-1".to_string(),
            expect: Expect::Scope,
        },
        Row {
            case: "the longest region an ARN can carry",
            scope_region: longest,
            expect: Expect::Scope,
        },
        Row {
            case: "one character past that length",
            scope_region: one_too_long,
            expect: Expect::Default,
        },
        Row {
            case: "a space inside the region",
            scope_region: "us east-1".to_string(),
            expect: Expect::Default,
        },
        Row {
            case: "a space before the digit group",
            scope_region: "us-east- 1".to_string(),
            expect: Expect::Default,
        },
        Row {
            case: "uppercase and underscores",
            scope_region: "NOT_A_REGION".to_string(),
            expect: Expect::Default,
        },
        Row {
            case: "a hostname",
            scope_region: "evil.example.com".to_string(),
            expect: Expect::Default,
        },
        Row {
            case: "no digit group",
            scope_region: "us-east".to_string(),
            expect: Expect::Default,
        },
        Row {
            case: "digits alone",
            scope_region: "12345".to_string(),
            expect: Expect::Default,
        },
        Row {
            case: "an empty region segment, so the scope names none",
            scope_region: String::new(),
            expect: Expect::Default,
        },
    ]
}

/// True when `value` is a value the modelled `ScanNameArn` shape admits: its `@pattern`
/// `^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$` and its
/// `@length` 1..300 both hold.
fn is_modelled_scan_name_arn(value: &str) -> bool {
    let length = value.chars().count();
    if !(1..=SCAN_NAME_ARN_MAX_LEN).contains(&length) {
        return false;
    }
    let Some(rest) = value.strip_prefix("arn:aws:codeguru-security:") else {
        return false;
    };
    // `[\S]+` is greedy, so the LAST `:scans/` is where the pattern ends the region and
    // the account id.
    let Some((head, scan_name)) = rest.rsplit_once(":scans/") else {
        return false;
    };
    let Some((region, account_id)) = head.rsplit_once(':') else {
        return false;
    };
    !region.is_empty()
        && !region.chars().any(char::is_whitespace)
        && account_id.len() == 12
        && account_id.bytes().all(|byte| byte.is_ascii_digit())
        && scan_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '$' | ':' | '.'))
}

/// `CreateScan` for `scan_name`, signed for `scope_region`.
async fn create_scan_signed_for(
    router: &MultiServiceRouter,
    scope_region: &str,
    scan_name: &str,
) -> Answer {
    let artifact = artifact_for(router, scan_name).await;
    call_signed_for(
        router,
        scope_region,
        "POST",
        "/scans",
        Some(json!({
            "scanName": scan_name,
            "resourceId": { "codeArtifactId": artifact },
        })),
    )
    .await
}

/// The `scanNameArn` `GetScan` reports for `scan_name`.
async fn get_scan_arn(router: &MultiServiceRouter, scan_name: &str) -> Value {
    let answer = call_signed_for(
        router,
        "us-west-2",
        "GET",
        &format!("/scans/{scan_name}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "GetScan({scan_name}): expected 200, got {} {}",
        answer.status, answer.json
    );
    answer.get("scanNameArn").clone()
}

/// The `scanNameArn` `ListScans` reports in `scan_name`'s summary.
async fn list_scans_arn(router: &MultiServiceRouter, scan_name: &str) -> Value {
    let answer = call_signed_for(router, "us-west-2", "GET", "/scans", None).await;
    assert_eq!(
        answer.status, 200,
        "ListScans: expected 200, got {} {}",
        answer.status, answer.json
    );
    answer
        .get("summaries")
        .as_array()
        .unwrap_or_else(|| panic!("ListScans reports no summaries: {}", answer.json))
        .iter()
        .find(|summary| summary["scanName"].as_str() == Some(scan_name))
        .unwrap_or_else(|| panic!("ListScans reports no summary for {scan_name}: {}", answer.json))
        .get("scanNameArn")
        .cloned()
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// The reported ARN is one the ScanNameArn shape admits, whatever the scope says
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_scan_reports_an_arn_its_own_shape_admits_for_any_credential_scope() {
    let (_engine, router) = harness();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let scan_name = format!("scope-arn-{index}");
        let created = create_scan_signed_for(&router, &row.scope_region, &scan_name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateScan expected 200, got {} {}",
            created.status, created.json
        );
        let arn = created.str_of("scanNameArn");
        let expected_region = match row.expect {
            Expect::Scope => row.scope_region.as_str(),
            Expect::Default => DEFAULT_REGION,
        };
        assert_eq!(
            arn,
            expected_scan_arn_in(expected_region, &scan_name),
            "case {case:?}: expected the ARN of region {expected_region:?}, got {arn:?}"
        );
        assert!(
            is_modelled_scan_name_arn(&arn),
            "case {case:?}: the reported scanNameArn must satisfy the ScanNameArn shape, \
             got {arn:?}"
        );

        // The readers report the ARN the create reported: one record, read back.
        let read = get_scan_arn(&router, &scan_name).await;
        assert_eq!(
            read,
            Value::String(arn.clone()),
            "case {case:?}: GetScan must report the ARN CreateScan reported, got {read:?}"
        );
        let listed = list_scans_arn(&router, &scan_name).await;
        assert_eq!(
            listed,
            Value::String(arn.clone()),
            "case {case:?}: ListScans must report the ARN CreateScan reported, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The scan is addressable by the ARN it was handed back under
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_reported_arn_is_accepted_as_a_resource_arn_by_the_tagging_operations() {
    let (_engine, router) = harness();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let scan_name = format!("taggable-{index}");
        let created = create_scan_signed_for(&router, &row.scope_region, &scan_name).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateScan expected 200, got {} {}",
            created.status, created.json
        );
        let arn = created.str_of("scanNameArn");

        let tagged = tag(&router, &arn, json!({ "team": "appsec" })).await;
        assert_eq!(
            tagged.status, 204,
            "case {case:?}: TagResource on the reported ARN {arn:?} expected 204, got {} {}",
            tagged.status, tagged.json
        );
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListTagsForResource on {arn:?} expected 200, got {} {}",
            listed.status, listed.json
        );
        assert_eq!(
            listed.get("tags"),
            &json!({ "team": "appsec" }),
            "case {case:?}: the tags written through the reported ARN, got {}",
            listed.json
        );
    }
}

// ---------------------------------------------------------------------------
// The shape's @length bound holds at its own extreme
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_longest_region_and_the_longest_scan_name_still_fit_the_length_bound() {
    let (_engine, router) = harness();
    // The longest value of each segment the ARN carries: a Region-shaped region of
    // `LONGEST_ARN_REGION` characters and a `ScanName` of its own maximum length, so the
    // reported ARN is exactly as long as the shape allows.
    let region = format!("{}-b-1", "a".repeat(LONGEST_ARN_REGION - "-b-1".len()));
    let scan_name = "s".repeat(SCAN_NAME_MAX_LEN);

    let created = create_scan_signed_for(&router, &region, &scan_name).await;
    assert_eq!(
        created.status, 200,
        "CreateScan at the shape's extreme: expected 200, got {} {}",
        created.status, created.json
    );
    let arn = created.str_of("scanNameArn");
    assert_eq!(
        arn.chars().count(),
        SCAN_NAME_ARN_MAX_LEN,
        "the longest region and the longest scan name make an ARN of exactly the shape's \
         maximum, got {arn:?}"
    );
    assert!(
        is_modelled_scan_name_arn(&arn),
        "the ARN at the shape's extreme must satisfy the ScanNameArn shape, got {arn:?}"
    );

    // Still addressable: the longest ARN is a `resourceArn` the tagging surface accepts.
    let tagged = tag(&router, &arn, json!({ "team": "appsec" })).await;
    assert_eq!(
        tagged.status, 204,
        "TagResource on the longest reported ARN expected 204, got {} {}",
        tagged.status, tagged.json
    );
}
