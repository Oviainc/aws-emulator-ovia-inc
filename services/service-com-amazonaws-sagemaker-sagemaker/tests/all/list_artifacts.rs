// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `ListArtifacts`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.ListArtifacts`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is reused from
//! `inference_deployment_lifecycle`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! member of the modelled `SortArtifactsBy` × `SortOrder` grid, each filter member and
//! their conjunction, both creation-time bounds, every page size from one entry to past
//! the published `MaxResults` bound and past the service's own ceiling, the per-Region
//! scoping of the result set, and the refusal of inputs outside the documented value
//! sets. Each summary is checked against what `DescribeArtifact` reads back, so the
//! listing reports stored state rather than anything the request carried.

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    SORT_ARTIFACTS_BY_VALUES, SORT_ORDER_VALUES,
};
use service_com_amazonaws_sagemaker_sagemaker::state::MAX_PAGE_SIZE;

use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator, CREATE_SPACING};

/// The service's own server-side page ceiling, read from the service rather than
/// restated, so changing it does not fail this test for an unrelated reason.
const PAGE_CEILING: usize = MAX_PAGE_SIZE as usize;

/// `MaxResults`' documented default for `ListArtifacts` ("The maximum number of artifacts
/// to return in the response. The default value is 10.").
const DOCUMENTED_DEFAULT_MAX_RESULTS: usize = 10;

/// The source URI two of the seeded artifacts share, so a `SourceUri` filter that selects
/// more than one row is distinguishable from one that selects a single artifact.
const SHARED_URI: &str = "s3://bucket/shared.csv";

/// One seeded artifact: the body it was created from, the ARN the service minted for it
/// and the instants the service recorded, read back through `DescribeArtifact`.
struct Seeded {
    case: &'static str,
    arn: String,
    body: Value,
    created: f64,
    modified: f64,
}

/// The create bodies the sort and filter tables range over, created in this order.
///
/// Rows differ along what the rules generalise over: whether `ArtifactName` is sent, the
/// `ArtifactType` (two distinct types plus the empty one `String256`'s `len 0..256`
/// allows), whether two artifacts share one `SourceUri`, and the number of `SourceTypes`
/// (absent, empty, two). Their names are deliberately not in creation order.
fn seed_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "named-dataset",
            json!({
                "ArtifactName": "art-gamma",
                "Source": { "SourceUri": "s3://bucket/alpha.csv" },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "empty-source-types-model",
            json!({
                "ArtifactName": "art-beta",
                "Source": { "SourceUri": SHARED_URI, "SourceTypes": [] },
                "ArtifactType": "Model",
            }),
        ),
        (
            "unnamed-sharing-the-source-uri",
            json!({
                "Source": {
                    "SourceUri": SHARED_URI,
                    "SourceTypes": [
                        { "SourceIdType": "Custom", "Value": "sha256:abc" },
                        { "SourceIdType": "MD5Hash", "Value": "d41d8cd98f00b204e9800998ecf8427e" },
                    ],
                },
                "ArtifactType": "DataSet",
            }),
        ),
        (
            "empty-artifact-type",
            json!({
                "ArtifactName": "art-alpha",
                "Source": { "SourceUri": "s3://bucket/delta.csv" },
                "ArtifactType": "",
            }),
        ),
    ]
}

/// A numeric member of a response body.
fn number(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric {member} in {body}"))
}

/// `Source` as a reader reports it for an artifact created with `body`: the source the
/// create sent, with `SourceTypes` as the LIST the artifact holds — the empty list when the
/// create named none, because the member reports what the artifact holds rather than what
/// the request mentioned.
fn reported_source(body: &Value) -> Value {
    let mut source = body
        .get("Source")
        .cloned()
        .expect("every create in this table carries the required Source");
    if source.get("SourceTypes").is_none() {
        source["SourceTypes"] = json!([]);
    }
    source
}

/// The `ArtifactArn`s a `ListArtifacts` response lists, in response order.
fn listed_arns(body: &Value) -> Vec<String> {
    body.get("ArtifactSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an ArtifactSummaries list in {body}"))
        .iter()
        .map(|summary| str_field(summary, "ArtifactArn"))
        .collect()
}

/// Create [`seed_bodies`] in `us-east-1`, spaced so the `CreationTime` order of the
/// result set is unambiguous, and read each one back so the assertions compare against
/// stored state rather than against the request.
async fn seed(emulator: &Emulator) -> Vec<Seeded> {
    let mut seeded = Vec::new();
    for (case, body) in seed_bodies() {
        let created = emulator.ok("CreateArtifact", &body).await;
        let arn = str_field(&created, "ArtifactArn");
        let described = emulator
            .ok("DescribeArtifact", &json!({ "ArtifactArn": arn }))
            .await;
        seeded.push(Seeded {
            case,
            arn,
            body,
            created: number(&described, "CreationTime"),
            modified: number(&described, "LastModifiedTime"),
        });
        tokio::time::sleep(CREATE_SPACING).await;
    }
    seeded
}

/// The ARNs of the seeded artifacts named by `cases`, in that order.
fn arns_of(seeded: &[Seeded], cases: &[&str]) -> Vec<String> {
    cases
        .iter()
        .map(|wanted| {
            seeded
                .iter()
                .find(|entry| entry.case == *wanted)
                .unwrap_or_else(|| panic!("no seeded artifact for case {wanted}"))
                .arn
                .clone()
        })
        .collect()
}

/// RULE: `ListArtifacts` returns the stored artifacts of the caller's Region, keeping only
/// those whose `SourceUri` and `ArtifactType` equal the filters the request carries, and
/// orders them by `SortBy`/`SortOrder` — defaulting to `CreationTime` / `Descending` as
/// both members' model docs state — with the `ArtifactArn` as the tie-break.
#[tokio::test]
async fn list_artifacts_honours_every_documented_sort_and_filter() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;

    let oldest_first = [
        "named-dataset",
        "empty-source-types-model",
        "unnamed-sharing-the-source-uri",
        "empty-artifact-type",
    ];
    let newest_first = {
        let mut reversed = oldest_first;
        reversed.reverse();
        reversed
    };

    // Rows differ along which sort members are sent, which filter member is used and how
    // selective it is, and whether the filters compose.
    let cases: Vec<(String, Value, Vec<&str>)> = vec![
        (
            "documented-defaults".to_string(),
            json!({}),
            newest_first.to_vec(),
        ),
        (
            "creation-time-ascending".to_string(),
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            oldest_first.to_vec(),
        ),
        (
            "creation-time-descending".to_string(),
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            newest_first.to_vec(),
        ),
        (
            "sort-order-alone-keeps-the-default-sort-by".to_string(),
            json!({ "SortOrder": "Ascending" }),
            oldest_first.to_vec(),
        ),
        (
            "sort-by-alone-keeps-the-default-sort-order".to_string(),
            json!({ "SortBy": "CreationTime" }),
            newest_first.to_vec(),
        ),
        (
            "source-uri-shared-by-two".to_string(),
            json!({ "SourceUri": SHARED_URI, "SortOrder": "Ascending" }),
            vec![
                "empty-source-types-model",
                "unnamed-sharing-the-source-uri",
            ],
        ),
        (
            "source-uri-of-one".to_string(),
            json!({ "SourceUri": "s3://bucket/alpha.csv" }),
            vec!["named-dataset"],
        ),
        (
            "source-uri-is-an-equality-filter-not-a-prefix".to_string(),
            json!({ "SourceUri": "s3://bucket/" }),
            vec![],
        ),
        (
            "source-uri-nothing-matches".to_string(),
            json!({ "SourceUri": "s3://bucket/absent.csv" }),
            vec![],
        ),
        (
            "artifact-type-of-two".to_string(),
            json!({ "ArtifactType": "DataSet", "SortOrder": "Ascending" }),
            vec!["named-dataset", "unnamed-sharing-the-source-uri"],
        ),
        (
            "artifact-type-of-one".to_string(),
            json!({ "ArtifactType": "Model" }),
            vec!["empty-source-types-model"],
        ),
        (
            // `String256` is `len 0..256`, so the empty string is a legal type and a
            // legal filter: it selects the artifacts stored with the empty type.
            "artifact-type-empty-string".to_string(),
            json!({ "ArtifactType": "" }),
            vec!["empty-artifact-type"],
        ),
        (
            "artifact-type-nothing-matches".to_string(),
            json!({ "ArtifactType": "Nothing" }),
            vec![],
        ),
        (
            "both-filters-compose".to_string(),
            json!({ "SourceUri": SHARED_URI, "ArtifactType": "DataSet" }),
            vec!["unnamed-sharing-the-source-uri"],
        ),
        (
            "both-filters-compose-to-nothing".to_string(),
            json!({ "SourceUri": "s3://bucket/alpha.csv", "ArtifactType": "Model" }),
            vec![],
        ),
    ];

    for (case, body, expected) in &cases {
        let listed = emulator.ok("ListArtifacts", body).await;
        assert_eq!(
            listed_arns(&listed),
            arns_of(&seeded, expected),
            "case {case}: ListArtifacts selection and order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
    }

    // The whole modelled grid of sort members is accepted, and each answers the full
    // result set in one of the two orders. The catalogs are the generated
    // `SORT_ARTIFACTS_BY_VALUES` / `SORT_ORDER_VALUES`, i.e. the modelled
    // `SortArtifactsBy` and `SortOrder` members.
    for sort_by in SORT_ARTIFACTS_BY_VALUES {
        for sort_order in SORT_ORDER_VALUES {
            let case = format!("SortBy={sort_by}/SortOrder={sort_order}");
            let listed = emulator
                .ok(
                    "ListArtifacts",
                    &json!({ "SortBy": sort_by, "SortOrder": sort_order }),
                )
                .await;
            let expected = if *sort_order == "Ascending" {
                oldest_first
            } else {
                newest_first
            };
            assert_eq!(
                listed_arns(&listed),
                arns_of(&seeded, &expected),
                "case {case}: ListArtifacts order, got {listed:?}"
            );
        }
    }
}

/// RULE: every `ArtifactSummary` replays the stored artifact — its minted ARN, its name
/// where it has one, the source it holds (`SourceTypes` included: the empty list where the
/// create named none), its type and the two instants the service recorded — and a listing
/// changes nothing a later caller sees.
#[tokio::test]
async fn list_artifacts_summaries_replay_the_stored_artifact() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;

    let listed = emulator
        .ok("ListArtifacts", &json!({ "SortOrder": "Ascending" }))
        .await;
    let summaries = listed
        .get("ArtifactSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an ArtifactSummaries list in {listed}"))
        .clone();
    assert_eq!(
        summaries.len(),
        seeded.len(),
        "every stored artifact must be listed, got {listed:?}"
    );

    for (index, entry) in seeded.iter().enumerate() {
        let case = entry.case;
        let summary = &summaries[index];
        assert_eq!(
            str_field(summary, "ArtifactArn"),
            entry.arn,
            "case {case}: ArtifactArn in the summary, got {summary:?}"
        );
        // `ArtifactName` is present only for an artifact created with one, and `Source` is
        // the source the artifact holds — the create's, with `SourceTypes` resolved to the
        // list it holds.
        for member in ["ArtifactName", "ArtifactType"] {
            assert_eq!(
                summary.get(member),
                entry.body.get(member),
                "case {case}: {member} must replay what was stored, got {summary:?}"
            );
        }
        assert_eq!(
            summary.get("Source"),
            Some(&reported_source(&entry.body)),
            "case {case}: Source must replay what the artifact holds, got {summary:?}"
        );
        assert_eq!(
            summary.get("CreationTime").and_then(Value::as_f64),
            Some(entry.created),
            "case {case}: CreationTime must be the stored one, got {summary:?}"
        );
        assert_eq!(
            summary.get("LastModifiedTime").and_then(Value::as_f64),
            Some(entry.modified),
            "case {case}: LastModifiedTime must be the stored one, got {summary:?}"
        );
        // The listing and the reader agree on the same stored artifact.
        let described = emulator
            .ok("DescribeArtifact", &json!({ "ArtifactArn": entry.arn }))
            .await;
        for member in [
            "ArtifactName",
            "Source",
            "ArtifactType",
            "CreationTime",
            "LastModifiedTime",
        ] {
            assert_eq!(
                summary.get(member),
                described.get(member),
                "case {case}: {member} must agree with DescribeArtifact, got {summary:?}"
            );
        }
    }

    // A read changes nothing a later caller sees: the second listing is identical.
    let again = emulator
        .ok("ListArtifacts", &json!({ "SortOrder": "Ascending" }))
        .await;
    assert_eq!(
        again, listed,
        "ListArtifacts must be stable across reads, got {again:?}"
    );
}

/// RULE: `CreatedAfter` and `CreatedBefore` keep exactly the artifacts on their side of
/// the instant, inclusive of it, and they compose with each other and with the value
/// filters. The bounds are placed between neighbouring instants, which is the finest
/// split the `awsJson1_1` epoch-seconds `Timestamp` represents without loss.
#[tokio::test]
async fn list_artifacts_filters_on_the_created_time_bounds() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;
    let split = (seeded[1].created + seeded[2].created) / 2.0;
    let upper_split = (seeded[2].created + seeded[3].created) / 2.0;

    // Rows differ along which bound is used, which side it selects, how many bounds
    // compose, and whether a bound falls outside the whole result set.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "created-before-the-split",
            json!({ "CreatedBefore": split, "SortOrder": "Ascending" }),
            vec!["named-dataset", "empty-source-types-model"],
        ),
        (
            "created-after-the-split",
            json!({ "CreatedAfter": split, "SortOrder": "Ascending" }),
            vec!["unnamed-sharing-the-source-uri", "empty-artifact-type"],
        ),
        (
            "both-bounds-around-one-artifact",
            json!({
                "CreatedAfter": split,
                "CreatedBefore": upper_split,
                "SortOrder": "Ascending",
            }),
            vec!["unnamed-sharing-the-source-uri"],
        ),
        (
            "bound-just-past-the-newest-instant-selects-every-artifact",
            json!({ "CreatedBefore": seeded[3].created + 0.001, "SortOrder": "Ascending" }),
            vec![
                "named-dataset",
                "empty-source-types-model",
                "unnamed-sharing-the-source-uri",
                "empty-artifact-type",
            ],
        ),
        (
            "bound-just-before-the-oldest-instant-selects-nothing",
            json!({ "CreatedBefore": seeded[0].created - 0.001 }),
            vec![],
        ),
        (
            "bound-an-hour-past-the-newest-instant-selects-nothing",
            json!({ "CreatedAfter": seeded[3].created + 3600.0 }),
            vec![],
        ),
        (
            "bound-composed-with-a-source-uri-filter",
            json!({
                "CreatedAfter": split,
                "SourceUri": SHARED_URI,
                "SortOrder": "Ascending",
            }),
            vec!["unnamed-sharing-the-source-uri"],
        ),
        (
            "bound-composed-with-an-artifact-type-filter",
            json!({
                "CreatedBefore": split,
                "ArtifactType": "DataSet",
                "SortOrder": "Ascending",
            }),
            vec!["named-dataset"],
        ),
    ];

    for (case, body, expected) in &cases {
        let listed = emulator.ok("ListArtifacts", body).await;
        assert_eq!(
            listed_arns(&listed),
            arns_of(&seeded, expected),
            "case {case}: ListArtifacts selection, got {listed:?}"
        );
    }
}

/// RULE: paging walks the whole ordered result exactly once — each page holds at most the
/// effective page size, a `NextToken` is present only while more remain, and feeding it
/// back resumes immediately after the previous page. The effective size is `MaxResults`
/// where the caller sends one (capped by the service's own ceiling) and this operation's
/// documented default of 10 where it does not.
#[tokio::test]
async fn list_artifacts_paging_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    // More artifacts than the documented default page, so a defaulted page and an
    // explicitly sized one are distinguishable.
    let mut arns: Vec<String> = Vec::new();
    for index in 0..12 {
        let created = emulator
            .ok(
                "CreateArtifact",
                &json!({
                    "ArtifactName": format!("page-{index:02}"),
                    "Source": { "SourceUri": format!("s3://bucket/page-{index:02}.csv") },
                    "ArtifactType": "DataSet",
                }),
            )
            .await;
        arns.push(str_field(&created, "ArtifactArn"));
        tokio::time::sleep(CREATE_SPACING).await;
    }

    // Rows: every page size from one entry up to past the end of the result set, plus
    // both published bounds of `MaxResults` (`range 1..100`, from
    // smithy_query(query="operation",operation="ListArtifacts")).
    for page_size in [1_usize, 2, 5, 11, 12, 13, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "MaxResults": page_size, "SortOrder": "Ascending" });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListArtifacts", &body).await;
            let page = listed_arns(&listed);
            let cap = page_size.min(PAGE_CEILING);
            assert!(
                page.len() <= cap,
                "case {case}: a page must hold at most {cap} entries, got {page:?}"
            );
            seen.extend(page);
            token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= arns.len() + 1,
                "case {case}: paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, arns,
            "case {case}: paging must walk the whole ordered result exactly once"
        );
    }

    // No `MaxResults`: the page is this operation's documented default of 10, and the
    // token still walks the rest.
    let first = emulator
        .ok("ListArtifacts", &json!({ "SortOrder": "Ascending" }))
        .await;
    assert_eq!(
        listed_arns(&first).len(),
        DOCUMENTED_DEFAULT_MAX_RESULTS,
        "case documented-default: the page size must default to 10, got {first:?}"
    );
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case documented-default: expected a NextToken in {first}"))
        .to_string();
    let second = emulator
        .ok(
            "ListArtifacts",
            &json!({ "NextToken": token, "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        listed_arns(&second),
        arns[DOCUMENTED_DEFAULT_MAX_RESULTS..].to_vec(),
        "case documented-default: the token must resume after the first page, got {second:?}"
    );
    assert!(
        second.get("NextToken").is_none(),
        "case documented-default: the last page must carry no NextToken, got {second:?}"
    );
}

/// RULE: the page is capped server-side, so no single call serialises the whole store
/// however large `MaxResults` is — and the resume token still walks the full set.
#[tokio::test]
async fn the_artifact_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    let emulator = Emulator::new();
    // Seeded past the ceiling so a capped page and an uncapped one are distinguishable.
    // The sort is `ArtifactArn`-tie-broken within one creation instant, so the walk below
    // compares against the ARNs sorted, not against creation order.
    let mut arns: Vec<String> = Vec::new();
    for index in 0..PAGE_CEILING + 3 {
        let created = emulator
            .ok(
                "CreateArtifact",
                &json!({
                    "ArtifactName": format!("cap-{index:03}"),
                    "Source": { "SourceUri": format!("s3://bucket/cap-{index:03}.csv") },
                    "ArtifactType": "DataSet",
                }),
            )
            .await;
        arns.push(str_field(&created, "ArtifactArn"));
    }

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "MaxResults": 100, "SortOrder": "Ascending" });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("ListArtifacts", &body).await;
        let page = listed_arns(&listed);
        assert!(
            page.len() <= PAGE_CEILING,
            "case ceiling: a page must hold at most {PAGE_CEILING} entries, got {}",
            page.len()
        );
        seen.extend(page);
        token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    let mut sorted_seen = seen.clone();
    sorted_seen.sort();
    let mut expected = arns.clone();
    expected.sort();
    assert_eq!(
        sorted_seen, expected,
        "case ceiling: the resume token must still walk the whole result exactly once"
    );
    assert_eq!(
        seen.len(),
        arns.len(),
        "case ceiling: no artifact may be listed twice, got {} entries",
        seen.len()
    );
}

/// RULE: an artifact belongs to the Region it was created in, so a listing reports the
/// artifacts of the Region the request signed for and never one whose ARN names another;
/// an unsigned request is served by the emulator's default Region.
#[tokio::test]
async fn artifacts_are_listed_only_in_the_region_they_were_created_in() {
    let emulator = Emulator::new();
    // Rows differ along the Region, including the emulator default an unsigned request
    // falls back to.
    let regions = ["us-east-1", "us-west-2", "eu-central-1"];
    let mut per_region: Vec<(&str, String)> = Vec::new();
    for region in regions {
        let (status, created) = emulator
            .call_in(
                region,
                "CreateArtifact",
                // One name per Region: `ArtifactName` is unique per account per Region,
                // so the same name in each Region is legal and keeps the rows identical
                // apart from the Region under test.
                &json!({
                    "ArtifactName": "regional",
                    "Source": { "SourceUri": "s3://bucket/regional.csv" },
                    "ArtifactType": "DataSet",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region}: CreateArtifact must succeed, got {status} with {created}"
        );
        per_region.push((region, str_field(&created, "ArtifactArn")));
    }

    for (region, arn) in &per_region {
        let (status, listed) = emulator.call_in(region, "ListArtifacts", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {region}: ListArtifacts must succeed, got {status} with {listed}"
        );
        assert_eq!(
            listed_arns(&listed),
            vec![arn.clone()],
            "case {region}: only that Region's artifact is listed, got {listed:?}"
        );
    }

    // An unsigned request is served by the emulator's default Region (`us-east-1`), so it
    // sees that Region's artifact and no other.
    let (status, listed) = emulator.call_unsigned("ListArtifacts", &json!({})).await;
    assert_eq!(
        status, 200,
        "case unsigned: ListArtifacts must succeed, got {status} with {listed}"
    );
    assert_eq!(
        listed_arns(&listed),
        vec![per_region[0].1.clone()],
        "case unsigned: the default Region's artifact is listed, got {listed:?}"
    );
}

/// RULE: an input outside a documented value set is refused with HTTP 400
/// `ValidationException` rather than silently ignored, and the refusal changes nothing.
#[tokio::test]
async fn list_artifacts_refuses_inputs_outside_the_documented_value_sets() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;

    // Rows: one outside each documented value set — `SortArtifactsBy`, `SortOrder`,
    // `MaxResults` one past each published bound (`range 1..100`), `SourceUri` outside
    // `len 1..2048`, `ArtifactType` past `String256`'s 256, `NextToken` past 8192, and a
    // token that names no boundary of the result set.
    let token_of_a_shorter_result_set = {
        let page = emulator
            .ok(
                "ListArtifacts",
                &json!({ "MaxResults": 1, "SortOrder": "Ascending" }),
            )
            .await;
        page.get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected a NextToken in {page}"))
            .to_string()
    };
    let cases: Vec<(&str, Value)> = vec![
        ("sort-by-outside-the-enum", json!({ "SortBy": "Name" })),
        (
            "sort-order-outside-the-enum",
            json!({ "SortOrder": "Sideways" }),
        ),
        ("max-results-below-1", json!({ "MaxResults": 0 })),
        ("max-results-above-100", json!({ "MaxResults": 101 })),
        ("source-uri-empty", json!({ "SourceUri": "" })),
        (
            "source-uri-one-past-the-maximum",
            json!({ "SourceUri": format!("s3://bucket/{}", "u".repeat(2049 - 12)) }),
        ),
        (
            "artifact-type-one-past-the-maximum",
            json!({ "ArtifactType": "T".repeat(257) }),
        ),
        (
            "next-token-one-past-the-maximum",
            json!({ "NextToken": "a".repeat(8193) }),
        ),
        (
            "next-token-that-names-no-boundary",
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            // A token minted against one result set names an artifact the filtered one
            // does not hold, so it is refused rather than silently restarting the walk.
            "next-token-of-another-result-set",
            json!({
                "NextToken": token_of_a_shorter_result_set,
                "ArtifactType": "Model",
                "SortOrder": "Ascending",
            }),
        ),
    ];

    for (case, body) in &cases {
        let (status, parsed) = emulator.call("ListArtifacts", body).await;
        assert_eq!(
            status, 400,
            "case {case}: ListArtifacts must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: refusal code, got {parsed}"
        );
    }

    // Nothing above reached state: every seeded artifact is still listed, in order.
    let listed = emulator
        .ok("ListArtifacts", &json!({ "SortOrder": "Ascending" }))
        .await;
    assert_eq!(
        listed_arns(&listed),
        seeded
            .iter()
            .map(|entry| entry.arn.clone())
            .collect::<Vec<String>>(),
        "a refused request must change nothing, got {listed:?}"
    );
}
