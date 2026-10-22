// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `ListHubContents`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.ListHubContents`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, on the
//! shared harness in [`crate::inference_deployment_lifecycle`].
//!
//! The hub records the listing reports are seeded straight into the shared engine's store
//! — the same `state::ComAmazonawsSagemakerSagemakerState` the handler reads — because the
//! operations that WRITE them (`CreateHub`, `ImportHubContent`,
//! `CreateHubContentReference`) are still scaffold stubs with their own open issues. The
//! listing itself is exercised only through the wire.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! `HubContentType` filter for every member of the modelled enum, the latest-version row,
//! every `SortBy`/`SortOrder` combination, the name/schema/creation-time filters, paging
//! over every page size up to past the end of the result set, and the not-found refusal
//! for a hub that was never created.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    self, ComAmazonawsSagemakerSagemakerState, Hub, HubContent, HubContentKey,
};

use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator};

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the harness signs for, and so the region every minted ARN names.
const REGION: &str = "us-east-1";

/// The instant the seeded creation times are offset from, so `CreationTime` ordering and
/// the creation-time bounds are exact without sleeping.
const BASE_SECS: u64 = 1_750_000_000;

/// A `HubStatus` member (`smithy_query(query="shape",shape="HubStatus")`).
const HUB_IN_SERVICE: &str = "InService";

/// This service's server-side page ceiling.
const CEILING: usize = state::MAX_PAGE_SIZE as usize;

/// One hub content version to seed.
struct Seeded<'a> {
    name: &'a str,
    /// A `HubContentType` member.
    content_type: &'a str,
    version: &'a str,
    document_schema_version: &'a str,
    /// A `HubContentStatus` member.
    status: &'a str,
    /// Seconds after [`BASE_SECS`] this version was created.
    created_after_base: u64,
    /// Set only on a `ModelReference`, which points at public hub content.
    public_arn: Option<&'a str>,
}

/// The seeded contents of `demo-hub`: three `Model` contents (one of them with three
/// versions, one with two) plus one content of every other modelled `HubContentType`.
const DEMO_HUB_CONTENTS: &[Seeded<'static>] = &[
    Seeded {
        name: "alpha-model",
        content_type: "Model",
        version: "1.0.0",
        document_schema_version: "1.0.0",
        status: "Available",
        created_after_base: 10,
        public_arn: None,
    },
    Seeded {
        name: "alpha-model",
        content_type: "Model",
        version: "1.9.0",
        document_schema_version: "1.5.0",
        status: "Available",
        created_after_base: 20,
        public_arn: None,
    },
    // `1.10.0` is the LATEST version of `alpha-model`: ordering the dotted components
    // numerically puts it above `1.9.0`, which a string comparison would not.
    Seeded {
        name: "alpha-model",
        content_type: "Model",
        version: "1.10.0",
        document_schema_version: "2.0.0",
        status: "Available",
        created_after_base: 40,
        public_arn: None,
    },
    Seeded {
        name: "beta-model",
        content_type: "Model",
        version: "2.0.9",
        document_schema_version: "1.0.0",
        status: "Available",
        created_after_base: 25,
        public_arn: None,
    },
    Seeded {
        name: "beta-model",
        content_type: "Model",
        version: "2.0.10",
        document_schema_version: "1.10.0",
        status: "Importing",
        created_after_base: 30,
        public_arn: None,
    },
    Seeded {
        name: "gamma-model",
        content_type: "Model",
        version: "1.0.0",
        document_schema_version: "1.9.0",
        status: "Deleting",
        created_after_base: 35,
        public_arn: None,
    },
    Seeded {
        name: "demo-notebook",
        content_type: "Notebook",
        version: "1.0.0",
        document_schema_version: "1.0.0",
        status: "Available",
        created_after_base: 50,
        public_arn: None,
    },
    Seeded {
        name: "demo-dataset",
        content_type: "DataSet",
        version: "3.4.5",
        document_schema_version: "1.0.0",
        status: "PendingImport",
        created_after_base: 60,
        public_arn: None,
    },
    Seeded {
        name: "ref-model",
        content_type: "ModelReference",
        version: "1.0.0",
        document_schema_version: "1.0.0",
        status: "Available",
        created_after_base: 70,
        public_arn: Some(
            "arn:aws:sagemaker:us-east-1:aws:hub-content/SageMakerPublicHub/Model/public-model",
        ),
    },
    Seeded {
        name: "doc-json",
        content_type: "JsonDoc",
        version: "1.0.0",
        document_schema_version: "1.0.0",
        status: "Available",
        created_after_base: 80,
        public_arn: None,
    },
];

/// A content of the same name in a DIFFERENT hub, so the listing's hub scoping is
/// observable.
const OTHER_HUB_CONTENTS: &[Seeded<'static>] = &[Seeded {
    name: "alpha-model",
    content_type: "Model",
    version: "5.0.0",
    document_schema_version: "1.0.0",
    status: "Available",
    created_after_base: 90,
    public_arn: None,
}];

fn seeded_at(offset_secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(BASE_SECS + offset_secs)
}

/// Write `hub` and its `contents` into the shared store the handler reads.
fn seed(emulator: &Emulator, hub: &str, contents: &[Seeded<'_>]) {
    let store = emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>();
    let mut store = store.lock();
    store.hubs.insert(
        hub.to_string(),
        Hub {
            name: hub.to_string(),
            arn: state::resource_arn(REGION, "hub", hub),
            status: HUB_IN_SERVICE.to_string(),
            creation_time: seeded_at(0),
            last_modified_time: seeded_at(0),
        },
    );
    for content in contents {
        store.hub_contents.insert(
            HubContentKey {
                hub_name: hub.to_string(),
                hub_content_type: content.content_type.to_string(),
                hub_content_name: content.name.to_string(),
                version: state::version_key(content.version),
                version_text: content.version.to_string(),
            },
            HubContent {
                name: content.name.to_string(),
                arn: content_arn(hub, content.content_type, content.name),
                content_type: content.content_type.to_string(),
                version: content.version.to_string(),
                document_schema_version: content.document_schema_version.to_string(),
                display_name: Some(format!("{} display", content.name)),
                description: Some(format!("{} description", content.name)),
                support_status: Some("Supported".to_string()),
                search_keywords: Some(vec![content.content_type.to_string()]),
                status: content.status.to_string(),
                creation_time: seeded_at(content.created_after_base),
                original_creation_time: Some(seeded_at(0)),
                sage_maker_public_hub_content_arn: content.public_arn.map(str::to_string),
            },
        );
    }
}

fn content_arn(hub: &str, content_type: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:hub-content/{hub}/{content_type}/{name}")
}

/// An emulator holding `demo-hub` (every seeded content) and `other-hub`.
fn seeded_emulator() -> Emulator {
    let emulator = Emulator::new();
    seed(&emulator, "demo-hub", DEMO_HUB_CONTENTS);
    seed(&emulator, "other-hub", OTHER_HUB_CONTENTS);
    emulator
}

fn summaries(body: &Value) -> Vec<Value> {
    body.get("HubContentSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a HubContentSummaries list in {body}"))
        .clone()
}

fn listed_names(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| str_field(summary, "HubContentName"))
        .collect()
}

/// A `ListHubContents` body over `demo-hub`'s `Model` contents, ordered by name so the
/// expected rows are unambiguous.
fn models(extra: Value) -> Value {
    let mut body = json!({
        "HubName": "demo-hub",
        "HubContentType": "Model",
        "SortBy": "HubContentName",
        "SortOrder": "Ascending",
    });
    for (member, value) in extra.as_object().expect("an object of extra members") {
        body[member.clone()] = value.clone();
    }
    body
}

/// RULE: the listing reports the contents of the named hub whose type is the requested
/// `HubContentType`, and nothing else.
#[tokio::test]
async fn hub_content_type_and_hub_name_scope_the_listing() {
    let emulator = seeded_emulator();

    // Rows: every member of `HubContentType`
    // (`smithy_query(query="shape",shape="HubContentType")`), plus a type the hub holds
    // nothing of.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("Model", vec!["alpha-model", "beta-model", "gamma-model"]),
        ("Notebook", vec!["demo-notebook"]),
        ("ModelReference", vec!["ref-model"]),
        ("DataSet", vec!["demo-dataset"]),
        ("JsonDoc", vec!["doc-json"]),
    ];
    for (content_type, expected) in cases {
        let listed = emulator
            .ok(
                "ListHubContents",
                &json!({
                    "HubName": "demo-hub",
                    "HubContentType": content_type,
                    "SortBy": "HubContentName",
                    "SortOrder": "Ascending",
                }),
            )
            .await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {content_type:?}: listed contents, got {listed:?}"
        );
        for summary in summaries(&listed) {
            assert_eq!(
                str_field(&summary, "HubContentType"),
                content_type,
                "case {content_type:?}: every row must carry the requested type, got {summary:?}"
            );
        }
    }

    // A second hub holding a content of the SAME name is a different record.
    let other = emulator
        .ok(
            "ListHubContents",
            &json!({ "HubName": "other-hub", "HubContentType": "Model" }),
        )
        .await;
    assert_eq!(
        listed_names(&other),
        vec!["alpha-model"],
        "case other-hub: the listing is scoped to the named hub, got {other:?}"
    );
    assert_eq!(
        str_field(&summaries(&other)[0], "HubContentArn"),
        content_arn("other-hub", "Model", "alpha-model"),
        "case other-hub: the row is that hub's own content, got {other:?}"
    );

    // A hub holding no content at all lists nothing, and does not fail.
    seed(&emulator, "empty-hub", &[]);
    let empty = emulator
        .ok(
            "ListHubContents",
            &json!({ "HubName": "empty-hub", "HubContentType": "Model" }),
        )
        .await;
    assert_eq!(
        listed_names(&empty),
        Vec::<String>::new(),
        "case empty-hub: an empty hub lists nothing, got {empty:?}"
    );
    assert!(
        empty.get("NextToken").is_none(),
        "case empty-hub: an empty page carries no NextToken, got {empty:?}"
    );
}

/// RULE: a content contributes ONE row, carrying its latest version — enumerating a
/// content's versions is `ListHubContentVersions`'s job — and versions order by their
/// dotted components numerically, so `1.10.0` is later than `1.9.0`.
#[tokio::test]
async fn each_row_carries_the_latest_version_of_its_content() {
    let emulator = seeded_emulator();
    let listed = emulator.ok("ListHubContents", &models(json!({}))).await;

    // Rows: a content with three versions whose latest is NOT the greatest string, a
    // content with two versions differing only in the patch component, and a content
    // with a single version.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("alpha-model", "1.10.0", "2.0.0"),
        ("beta-model", "2.0.10", "1.10.0"),
        ("gamma-model", "1.0.0", "1.9.0"),
    ];
    let rows = summaries(&listed);
    assert_eq!(
        rows.len(),
        cases.len(),
        "a content contributes exactly one row, got {listed:?}"
    );
    for (name, version, schema_version) in cases {
        let row = rows
            .iter()
            .find(|row| str_field(row, "HubContentName") == name)
            .unwrap_or_else(|| panic!("case {name}: expected a row, got {listed:?}"));
        assert_eq!(
            str_field(row, "HubContentVersion"),
            version,
            "case {name}: the row must carry the latest version, got {row:?}"
        );
        assert_eq!(
            str_field(row, "DocumentSchemaVersion"),
            schema_version,
            "case {name}: the row must carry THAT version's schema version, got {row:?}"
        );
    }
}

/// RULE: every member of the stored content the summary shape declares is replayed as
/// stored.
#[tokio::test]
async fn every_summary_member_replays_the_stored_record() {
    let emulator = seeded_emulator();

    // Rows: the type whose records carry a public-hub-content ARN, and one whose records
    // do not.
    let cases: Vec<(&str, &str, u64, Option<&str>)> = vec![
        (
            "ModelReference",
            "ref-model",
            70,
            Some("arn:aws:sagemaker:us-east-1:aws:hub-content/SageMakerPublicHub/Model/public-model"),
        ),
        ("Notebook", "demo-notebook", 50, None),
    ];

    for (content_type, name, created_after_base, public_arn) in cases {
        let listed = emulator
            .ok(
                "ListHubContents",
                &json!({ "HubName": "demo-hub", "HubContentType": content_type }),
            )
            .await;
        let row = &summaries(&listed)[0];
        assert_eq!(
            str_field(row, "HubContentArn"),
            content_arn("demo-hub", content_type, name),
            "case {name}: HubContentArn, got {row:?}"
        );
        assert_eq!(
            str_field(row, "HubContentDisplayName"),
            format!("{name} display"),
            "case {name}: HubContentDisplayName, got {row:?}"
        );
        assert_eq!(
            str_field(row, "HubContentDescription"),
            format!("{name} description"),
            "case {name}: HubContentDescription, got {row:?}"
        );
        assert_eq!(
            str_field(row, "SupportStatus"),
            "Supported",
            "case {name}: SupportStatus, got {row:?}"
        );
        assert_eq!(
            row.get("HubContentSearchKeywords"),
            Some(&json!([content_type])),
            "case {name}: HubContentSearchKeywords, got {row:?}"
        );
        assert_eq!(
            str_field(row, "HubContentStatus"),
            "Available",
            "case {name}: HubContentStatus, got {row:?}"
        );
        assert_eq!(
            row.get("CreationTime").and_then(Value::as_f64),
            Some((BASE_SECS + created_after_base) as f64),
            "case {name}: CreationTime must be the stored one, got {row:?}"
        );
        assert_eq!(
            row.get("OriginalCreationTime").and_then(Value::as_f64),
            Some(BASE_SECS as f64),
            "case {name}: OriginalCreationTime must be the stored one, got {row:?}"
        );
        assert_eq!(
            row.get("SageMakerPublicHubContentArn")
                .and_then(Value::as_str),
            public_arn,
            "case {name}: SageMakerPublicHubContentArn, got {row:?}"
        );
    }
}

/// RULE: `SortBy` orders the rows by the named key and `SortOrder` reverses that key,
/// with the content name as the tie-break in both directions.
#[tokio::test]
async fn sort_by_and_sort_order_order_the_listing() {
    let emulator = seeded_emulator();

    // Rows: every member of `HubContentSortBy`
    // (`smithy_query(query="shape",shape="HubContentSortBy")`) in both `SortOrder`
    // directions, plus the call that names neither. Seeded so name order, creation-time
    // order and status order all differ.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "name-ascending",
            json!({ "SortBy": "HubContentName", "SortOrder": "Ascending" }),
            vec!["alpha-model", "beta-model", "gamma-model"],
        ),
        (
            "name-descending",
            json!({ "SortBy": "HubContentName", "SortOrder": "Descending" }),
            vec!["gamma-model", "beta-model", "alpha-model"],
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["beta-model", "gamma-model", "alpha-model"],
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            vec!["alpha-model", "gamma-model", "beta-model"],
        ),
        (
            "status-ascending",
            json!({ "SortBy": "HubContentStatus", "SortOrder": "Ascending" }),
            vec!["alpha-model", "gamma-model", "beta-model"],
        ),
        (
            "status-descending",
            json!({ "SortBy": "HubContentStatus", "SortOrder": "Descending" }),
            vec!["beta-model", "gamma-model", "alpha-model"],
        ),
        (
            "no-sort-named",
            json!({}),
            vec!["alpha-model", "gamma-model", "beta-model"],
        ),
    ];

    for (case, extra, expected) in cases {
        let mut body = json!({ "HubName": "demo-hub", "HubContentType": "Model" });
        for (member, value) in extra.as_object().expect("an object of extra members") {
            body[member.clone()] = value.clone();
        }
        let listed = emulator.ok("ListHubContents", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: listing order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page carries no NextToken, got {listed:?}"
        );
    }
}

/// RULE: `NameContains` keeps the contents whose name contains the string, and
/// `MaxSchemaVersion` keeps those whose document schema version is at most the bound,
/// compared component by component.
#[tokio::test]
async fn name_contains_and_max_schema_version_select_the_matching_contents() {
    let emulator = seeded_emulator();

    // Rows differ along the filter's selectivity and, for the schema bound, along which
    // dotted component decides. The seeded latest rows carry schema versions 2.0.0
    // (alpha), 1.10.0 (beta) and 1.9.0 (gamma); `1.10.0` as a BOUND must therefore keep
    // 1.9.0, which a string comparison would drop. `DocumentSchemaVersion` is
    // `len 5..14` (smithy_query operation=ListHubContents), so `1234.1234.1234` is the
    // published upper bound's longest value.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "name-contains-one-match",
            json!({ "NameContains": "alpha" }),
            vec!["alpha-model"],
        ),
        (
            "name-contains-an-inner-substring",
            json!({ "NameContains": "mma" }),
            vec!["gamma-model"],
        ),
        (
            "name-contains-every-content",
            json!({ "NameContains": "model" }),
            vec!["alpha-model", "beta-model", "gamma-model"],
        ),
        (
            "name-contains-no-match",
            json!({ "NameContains": "zzz" }),
            vec![],
        ),
        (
            "schema-bound-below-every-content",
            json!({ "MaxSchemaVersion": "1.0.0" }),
            vec![],
        ),
        (
            "schema-bound-is-inclusive",
            json!({ "MaxSchemaVersion": "1.9.0" }),
            vec!["gamma-model"],
        ),
        (
            "schema-bound-orders-components-numerically",
            json!({ "MaxSchemaVersion": "1.10.0" }),
            vec!["beta-model", "gamma-model"],
        ),
        (
            "schema-bound-above-every-content",
            json!({ "MaxSchemaVersion": "2.0.0" }),
            vec!["alpha-model", "beta-model", "gamma-model"],
        ),
        (
            "schema-bound-at-the-longest-published-value",
            json!({ "MaxSchemaVersion": "1234.1234.1234" }),
            vec!["alpha-model", "beta-model", "gamma-model"],
        ),
        (
            "both-filters-together",
            json!({ "NameContains": "model", "MaxSchemaVersion": "1.10.0" }),
            vec!["beta-model", "gamma-model"],
        ),
    ];

    for (case, extra, expected) in cases {
        let listed = emulator.ok("ListHubContents", &models(extra)).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: selected contents, got {listed:?}"
        );
    }
}

/// RULE: `CreationTimeAfter` / `CreationTimeBefore` select exactly the contents on that
/// side of the bound, inclusively.
#[tokio::test]
async fn the_creation_time_bounds_select_contents_on_that_side() {
    let emulator = seeded_emulator();
    let beta = (BASE_SECS + 30) as f64;
    let gamma = (BASE_SECS + 35) as f64;
    let alpha = (BASE_SECS + 40) as f64;
    let between_beta_and_gamma = (beta + gamma) / 2.0;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "after-a-bound-between-two-contents",
            json!({ "CreationTimeAfter": between_beta_and_gamma }),
            vec!["gamma-model", "alpha-model"],
        ),
        (
            "before-a-bound-between-two-contents",
            json!({ "CreationTimeBefore": between_beta_and_gamma }),
            vec!["beta-model"],
        ),
        (
            "after-is-inclusive-of-the-stored-time",
            json!({ "CreationTimeAfter": alpha }),
            vec!["alpha-model"],
        ),
        (
            "before-is-inclusive-of-the-stored-time",
            json!({ "CreationTimeBefore": beta }),
            vec!["beta-model"],
        ),
        (
            "both-bounds-around-one-content",
            json!({ "CreationTimeAfter": between_beta_and_gamma, "CreationTimeBefore": (gamma + alpha) / 2.0 }),
            vec!["gamma-model"],
        ),
        (
            "a-bound-that-excludes-everything",
            json!({ "CreationTimeAfter": alpha + 3600.0 }),
            vec![],
        ),
        (
            "a-bound-combined-with-a-name-filter",
            json!({ "CreationTimeAfter": between_beta_and_gamma, "NameContains": "gamma" }),
            vec!["gamma-model"],
        ),
    ];

    for (case, extra, expected) in cases {
        let mut body = models(extra);
        // Ordered by creation time, so the expected rows read in the order the bound
        // selects them.
        body["SortBy"] = json!("CreationTime");
        body["SortOrder"] = json!("Ascending");
        let listed = emulator.ok("ListHubContents", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: selected contents, got {listed:?}"
        );
    }
}

/// RULE: paging with `MaxResults` walks the whole ordered listing exactly once — each
/// page holds at most `MaxResults` rows, a `NextToken` is present only while more remain,
/// and feeding it back resumes immediately after the previous page.
#[tokio::test]
async fn paging_walks_the_whole_listing_exactly_once() {
    let emulator = seeded_emulator();
    let ordered = vec!["alpha-model", "beta-model", "gamma-model"];

    // Rows: every page size from one row up to past the end of the result set, and the
    // published upper bound. `MaxResults` is `range 1..100`
    // (smithy_query operation=ListHubContents), so 1 and 100 are its bounds.
    for page_size in [1_usize, 2, 3, 4, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            let mut body = models(json!({ "MaxResults": page_size }));
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListHubContents", &body).await;
            let names = listed_names(&listed);
            assert!(
                names.len() <= page_size,
                "case {case}: a page holds at most {page_size} rows, got {names:?}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= ordered.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, ordered,
            "case {case}: paging must yield the whole ordered listing exactly once"
        );
        assert_eq!(
            pages,
            ordered.len().div_ceil(page_size),
            "case {case}: page count, saw {seen:?}"
        );
    }

    // A token that names no boundary of this result set is refused rather than silently
    // restarting the listing.
    let (status, parsed) = emulator
        .call("ListHubContents", &models(json!({ "NextToken": "nope" })))
        .await;
    assert_eq!(
        status, 400,
        "case bogus-NextToken: expected HTTP 400, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case bogus-NextToken: error code, got {parsed:?}"
    );
}

/// RULE: the page is capped server-side, so no single call serialises a whole hub however
/// large `MaxResults` is or whether the caller sends it at all — and the resume token
/// still walks the full listing.
#[tokio::test]
async fn the_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    let stored = CEILING + 7;
    let emulator = Emulator::new();
    // Names are zero-padded so the name order and the seeding order agree.
    let names: Vec<String> = (0..stored).map(|index| format!("content-{index:03}")).collect();
    let seeds: Vec<Seeded<'_>> = names
        .iter()
        .enumerate()
        .map(|(index, name)| Seeded {
            name,
            content_type: "Model",
            version: "1.0.0",
            document_schema_version: "1.0.0",
            status: "Available",
            created_after_base: index as u64,
            public_arn: None,
        })
        .collect();
    seed(&emulator, "big-hub", &seeds);

    // Rows: no `MaxResults` at all, and the published maximum, which is above the
    // service's own ceiling.
    for extra in [json!({}), json!({ "MaxResults": 100 })] {
        let case = format!("extra={extra}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({
                "HubName": "big-hub",
                "HubContentType": "Model",
                "SortBy": "HubContentName",
                "SortOrder": "Ascending",
            });
            for (member, value) in extra.as_object().expect("an object of extra members") {
                body[member.clone()] = value.clone();
            }
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListHubContents", &body).await;
            let names = listed_names(&listed);
            assert!(
                names.len() <= CEILING,
                "case {case}: a page must be capped at {CEILING} rows, got {}",
                names.len()
            );
            seen.extend(names);
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen.len(),
            stored,
            "case {case}: the token must walk every stored content"
        );
    }
}

/// RULE: a hub that was never created has no contents to list, so the call is refused
/// with the operation's one modelled error, `ResourceNotFound` (HTTP 400); a hub that
/// exists is addressable by name or by ARN, both naming the same record.
#[tokio::test]
async fn a_hub_that_was_never_created_is_refused_with_resource_not_found() {
    let emulator = seeded_emulator();
    let hub_arn = |name: &str| format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:hub/{name}");

    // Rows: a name nothing created, the ARN form of such a name, a name that differs from
    // a stored hub only in case, and an ARN whose hub segment names no hub.
    let cases: Vec<(&str, String)> = vec![
        ("unknown-name", "no-such-hub".to_string()),
        ("unknown-arn", hub_arn("no-such-hub")),
        ("case-differs-from-a-stored-hub", "Demo-Hub".to_string()),
        ("unknown-arn-in-another-account", {
            format!("arn:aws:sagemaker:{REGION}:aws:hub/no-such-hub")
        }),
    ];
    for (case, hub_name) in cases {
        let (status, parsed) = emulator
            .call(
                "ListHubContents",
                &json!({ "HubName": hub_name, "HubContentType": "Model" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFound",
            "case {case}: error code, got {parsed:?}"
        );
    }

    // Both forms of an EXISTING hub's name resolve to the same stored hub.
    for (case, hub_name) in [
        ("plain-name", "demo-hub".to_string()),
        ("arn", hub_arn("demo-hub")),
    ] {
        let listed = emulator
            .ok(
                "ListHubContents",
                &json!({
                    "HubName": hub_name,
                    "HubContentType": "Model",
                    "SortBy": "HubContentName",
                    "SortOrder": "Ascending",
                }),
            )
            .await;
        assert_eq!(
            listed_names(&listed),
            vec!["alpha-model", "beta-model", "gamma-model"],
            "case {case}: a hub is addressable by name and by ARN, got {listed:?}"
        );
    }
}

/// RULE: a request outside a member's modelled constraint, or naming a value the modelled
/// enum does not hold, is refused with HTTP 400 rather than listed.
#[tokio::test]
async fn requests_outside_the_modelled_contract_are_refused() {
    let emulator = seeded_emulator();

    // Rows: one past each published bound this operation states — `MaxResults`
    // `range 1..100`, `NameContains` `len 0..63`, `MaxSchemaVersion` `len 5..14` — and a
    // value outside each of the three enums the input names.
    let too_long_name = "a".repeat(64);
    let cases: Vec<(&str, Value)> = vec![
        ("max-results-below-1", json!({ "MaxResults": 0 })),
        ("max-results-above-100", json!({ "MaxResults": 101 })),
        ("name-contains-past-63", json!({ "NameContains": too_long_name })),
        (
            "name-contains-with-an-illegal-character",
            json!({ "NameContains": "not a name" }),
        ),
        (
            "schema-version-below-the-minimum-length",
            json!({ "MaxSchemaVersion": "1.0" }),
        ),
        (
            "schema-version-past-the-maximum-length",
            json!({ "MaxSchemaVersion": "12345.1234.1234" }),
        ),
        ("sort-by-outside-the-enum", json!({ "SortBy": "HubName" })),
        ("sort-order-outside-the-enum", json!({ "SortOrder": "Random" })),
    ];
    for (case, extra) in cases {
        let (status, parsed) = emulator
            .call("ListHubContents", &models(extra))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: must be refused, got {status} {parsed}"
        );
    }

    // `HubContentType` is required and enum-valued: a value outside the enum is refused
    // even though the hub exists.
    let (status, parsed) = emulator
        .call(
            "ListHubContents",
            &json!({ "HubName": "demo-hub", "HubContentType": "Spreadsheet" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case hub-content-type-outside-the-enum: must be refused, got {status} {parsed}"
    );
}
