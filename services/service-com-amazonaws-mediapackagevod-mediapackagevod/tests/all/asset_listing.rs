// SPDX-License-Identifier: CC0-1.0

//! `ListAssets` as the FILTERED, PAGED report over stored assets that the model
//! describes: `GET /assets` with `packagingGroupId`, `maxResults` and `nextToken` all
//! bound to the query string (`@httpQuery`), answering `assets` (a list of
//! `AssetShallow`) plus `nextToken`.
//!
//! Every test drives real HTTP requests through the router the production binary
//! builds, so what it asserts is the wire behaviour a client sees. Each test owns its
//! own engine, so no test observes another's state; the request harness
//! ([`super::packaging_lifecycle::call`] and friends) is shared rather than rebuilt.
//!
//! The RULE these tests pin, over a SET of inputs rather than one recorded call: a
//! listing of a resource collection reports every stored resource whose filtered
//! attribute EQUALS the query parameter — exactly once, in one stable order, across
//! however many pages `maxResults` splits it into — and reports nothing else. Spelled
//! out for this operation:
//!
//! * `packagingGroupId` selects records by exact equality with the group the asset was
//!   created in: not a prefix, not a case-folded match, and not an address of a
//!   resource (a filter matching nothing is an empty page, never an error);
//! * paging a filtered listing reports that group's assets exactly once each, whatever
//!   the page size, and hands out a cursor only while something is left to read;
//! * each entry is the `AssetShallow` projection of the record `CreateAsset`
//!   persisted — the same ARN, `createdAt`, source members and tags the create
//!   answered with, and no member beyond the ones the model declares;
//! * the listing is reporting-only: reading it again, or paging it to the end, reports
//!   exactly the same thing.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::packaging_lifecycle::{account_id, call, create_asset, create_group, router, REGION};

/// The members an `AssetShallow` carries for an asset created without a `ResourceId`,
/// from the model's `AssetShallow` shape (`Arn`, `CreatedAt`, `Id`, `PackagingGroupId`,
/// `ResourceId`, `SourceArn`, `SourceRoleArn`, `Tags`) minus `ResourceId`, which the
/// service reports only where the caller stated it.
const ASSET_SHALLOW_MEMBERS: [&str; 7] = [
    "arn",
    "createdAt",
    "id",
    "packagingGroupId",
    "sourceArn",
    "sourceRoleArn",
    "tags",
];

/// A packaging-group id of the generated, UUID-suffixed form the recorded fixtures
/// use, so the filter is exercised on a long id and not only on a short literal.
const UUID_GROUP: &str = "sim2real-mpvod-groupa-bf36c11a-b9ff-4607-a34e-93fcde1af574";

/// One page of `ListAssets` for `query` (the raw query string, without `?`): the
/// entries it reported and the cursor it handed back.
async fn list_page(router: &MultiServiceRouter, query: &str) -> (Vec<Value>, Option<String>) {
    let path = if query.is_empty() {
        "/assets".to_string()
    } else {
        format!("/assets?{query}")
    };
    let (status, body) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {query:?}: a listing is a 200, got {status}: body {body}"
    );
    let entries = body["assets"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {query:?}: expected an assets list, got {body}"));
    let token = body["nextToken"].as_str().map(str::to_string);
    (entries, token)
}

/// Every entry the listing reports for `filter`, following `nextToken` to the end.
///
/// `page_size` is the `maxResults` each request asks for, so a page longer than the
/// caller allowed fails the test rather than being silently accepted.
async fn list_all(
    router: &MultiServiceRouter,
    filter: Option<&str>,
    page_size: Option<usize>,
) -> Vec<Value> {
    let case = format!("filter={filter:?}, maxResults={page_size:?}");
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than there can be pages of a single asset each, so a cursor
    // that fails to advance fails the test instead of looping forever.
    for _ in 0..256 {
        let mut query = String::new();
        if let Some(filter) = filter {
            query.push_str(&format!("packagingGroupId={filter}"));
        }
        if let Some(size) = page_size {
            query.push_str(&format!("&maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push_str(&format!("&nextToken={token}"));
        }
        let (page, next) = list_page(router, query.trim_start_matches('&')).await;
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {case:?}: a page must hold at most {size} entries, got {}",
                page.len()
            );
        }
        assert!(
            next.is_none() || !page.is_empty(),
            "case {case:?}: a cursor must mean something is left to read, got an empty \
             page with {next:?}"
        );
        entries.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => return entries,
        }
    }
    panic!("case {case:?}: the listing never stopped handing out a nextToken");
}

/// The `id` of every entry, in the order the listing reported them.
fn ids_of(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The asset ids `count` assets of `group` are created under: deliberately NOT in
/// creation order, so a listing that merely echoes insertion order fails the test.
fn asset_ids(group: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("{group}-asset-{:02}", (count - index) * 7 % 97))
        .collect()
}

/// One row of [`the_filtered_listing_pages_exactly_that_groups_assets`]: the case name,
/// the groups to create with how many assets each, the `packagingGroupId` to list with
/// (`None` for an unfiltered listing) and the `maxResults` each page asks for.
type PagingCase<'a> = (&'a str, Vec<(&'a str, usize)>, Option<&'a str>, Option<usize>);

/// Paging a listing reports exactly the assets the filter selects — every one of them,
/// exactly once, in one stable ascending `Id` order — whatever the page size, and an
/// unmatched filter is an empty page with no cursor rather than an error.
#[tokio::test]
async fn the_filtered_listing_pages_exactly_that_groups_assets() {
    let cases: Vec<PagingCase<'_>> = vec![
        (
            "two groups, one asset each, filtered by the long generated id",
            vec![(UUID_GROUP, 1), ("g1", 1)],
            Some(UUID_GROUP),
            None,
        ),
        (
            "one group, unfiltered, one asset per page",
            vec![("g1", 2)],
            None,
            Some(1),
        ),
        (
            "three groups, filtered by the middle one, two per page",
            vec![("g1", 3), ("vod-group-2", 4), ("group.with_mixed-chars", 2)],
            Some("vod-group-2"),
            Some(2),
        ),
        (
            "filtered, one asset per page, so every page but the last hands out a cursor",
            vec![("g1", 5), ("vod-group-2", 2)],
            Some("g1"),
            Some(1),
        ),
        (
            "filtered, a page larger than the whole match set",
            vec![("g1", 3), ("vod-group-2", 1)],
            Some("g1"),
            Some(1000),
        ),
        (
            "filtered by a stored group holding no asset",
            vec![("g1", 2), ("vod-group-2", 0)],
            Some("vod-group-2"),
            None,
        ),
        (
            "filtered by a group nothing is stored under",
            vec![("g1", 2)],
            Some("no-such-group"),
            Some(3),
        ),
        (
            "no assets at all, unfiltered",
            vec![("g1", 0)],
            None,
            Some(1),
        ),
        (
            "unfiltered, spanning every group",
            vec![("g1", 2), ("vod-group-2", 3), (UUID_GROUP, 1)],
            None,
            Some(2),
        ),
    ];

    for (case, groups, filter, page_size) in cases {
        let router = router();
        let mut created: Vec<(String, String)> = Vec::new();
        for (group, count) in &groups {
            create_group(&router, group, None).await;
            for id in asset_ids(group, *count) {
                create_asset(&router, &id, group).await;
                created.push(((*group).to_string(), id));
            }
        }

        let mut expected: Vec<String> = created
            .iter()
            .filter(|(group, _)| filter.is_none_or(|filter| group == filter))
            .map(|(_, id)| id.clone())
            .collect();
        expected.sort();

        let reported = list_all(&router, filter, page_size).await;
        let reported_ids = ids_of(&reported);
        assert_eq!(
            reported_ids, expected,
            "case {case:?}: the listing must report exactly the filtered assets in \
             ascending id order, got {reported_ids:?}"
        );
        let unique: BTreeSet<&String> = reported_ids.iter().collect();
        assert_eq!(
            unique.len(),
            reported_ids.len(),
            "case {case:?}: no asset may be reported twice, got {reported_ids:?}"
        );
        for entry in &reported {
            assert_eq!(
                entry["packagingGroupId"].as_str(),
                filter.or_else(|| {
                    created
                        .iter()
                        .find(|(_, id)| Some(id.as_str()) == entry["id"].as_str())
                        .map(|(group, _)| group.as_str())
                }),
                "case {case:?}: every entry must report the group it was created in, \
                 got {entry}"
            );
        }
    }
}

/// The filter selects by EQUALITY with the stored group id: an id that merely starts
/// with the filter, or differs only in case, is a different packaging group and its
/// assets must not be reported.
#[tokio::test]
async fn the_filter_matches_the_stored_group_id_exactly() {
    // (group id, how many assets it holds) — ids chosen so one is a prefix of another
    // and two differ only in case, which `CreatePackagingGroup` accepts as distinct
    // resources.
    let groups: [(&str, usize); 3] = [("alpha", 2), ("alpha-2", 1), ("ALPHA", 1)];
    let router = router();
    let mut created: Vec<(&str, String)> = Vec::new();
    for (group, count) in groups {
        create_group(&router, group, None).await;
        for id in asset_ids(group, count) {
            create_asset(&router, &id, group).await;
            created.push((group, id));
        }
    }

    // (case, the filter, the group whose assets must be reported — `None` for none)
    let cases: [(&str, &str, Option<&str>); 5] = [
        ("the exact id", "alpha", Some("alpha")),
        ("the longer id that starts with it", "alpha-2", Some("alpha-2")),
        ("the same id in another case", "ALPHA", Some("ALPHA")),
        ("a prefix of a stored id", "alph", None),
        ("a stored id with a suffix", "alpha-", None),
    ];
    for (case, filter, selected) in cases {
        let mut expected: Vec<String> = created
            .iter()
            .filter(|(group, _)| Some(*group) == selected)
            .map(|(_, id)| id.clone())
            .collect();
        expected.sort();
        let reported = ids_of(&list_all(&router, Some(filter), Some(2)).await);
        assert_eq!(
            reported, expected,
            "case {case:?}: filtering by {filter:?} must report exactly the assets of \
             {selected:?}, got {reported:?}"
        );
    }
}

/// Each listed entry is the `AssetShallow` projection of the record the create
/// persisted — same ARN, `createdAt`, source members, `resourceId` and tags — and
/// carries no member the model does not declare for it.
#[tokio::test]
async fn each_listed_asset_is_the_record_the_create_persisted() {
    // (case, the create request's optional members beyond the required four)
    let cases: Vec<(&str, Value)> = vec![
        ("no optional members", json!({})),
        ("two tags", json!({ "tags": { "env": "test", "team": "video" } })),
        ("an empty tag map", json!({ "tags": {} })),
        ("a resourceId", json!({ "resourceId": "speke-resource-7" })),
        (
            "a resourceId and tags",
            json!({ "resourceId": "speke-resource-8", "tags": { "env": "prod" } }),
        ),
    ];

    let router = router();
    create_group(&router, UUID_GROUP, None).await;
    let mut created: Vec<(&str, String, Value)> = Vec::new();
    for (index, (case, optional)) in cases.into_iter().enumerate() {
        let id = format!("asset-{index}");
        let mut body = json!({
            "id": id,
            "packagingGroupId": UUID_GROUP,
            "sourceArn": format!("arn:aws:s3:::source-bucket-{index}/source/asset.m3u8"),
            "sourceRoleArn": format!("arn:aws:iam::{}:role/MediaPackageVodIngest", account_id()),
        });
        let Value::Object(members) = optional else {
            panic!("case {case:?}: the optional members must be a JSON object");
        };
        for (key, value) in members {
            body[key] = value;
        }
        let (status, asset) = call(&router, "POST", "/assets", Some(body)).await;
        assert_eq!(status, 200, "case {case:?}: CreateAsset body {asset}");
        created.push((case, id, asset));
    }

    let listed = list_all(&router, Some(UUID_GROUP), Some(2)).await;
    assert_eq!(
        listed.len(),
        created.len(),
        "the listing must report every created asset, got {:?}",
        ids_of(&listed)
    );

    for (case, id, create_answer) in &created {
        let entry = listed
            .iter()
            .find(|entry| entry["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: {id} is missing from the listing: {listed:?}")
            });
        for member in ASSET_SHALLOW_MEMBERS {
            assert_eq!(
                entry[member], create_answer[member],
                "case {case:?}: the listed {member} must be what the create persisted, \
                 got {}",
                entry[member]
            );
        }
        assert_eq!(
            entry["resourceId"], create_answer["resourceId"],
            "case {case:?}: the listed resourceId must be what the create persisted, got {}",
            entry["resourceId"]
        );
        assert_eq!(
            entry["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:mediapackage-vod:{REGION}:{}:assets/{id}",
                    account_id()
                )
                .as_str()
            ),
            "case {case:?}: the listed ARN must name this request's region and account, got {}",
            entry["arn"]
        );

        // `AssetShallow` declares eight members and no more: the shallow projection
        // carries `resourceId` exactly where the caller stated it, and nothing else
        // beyond the seven the service always materialises.
        let mut expected_members: BTreeSet<&str> = ASSET_SHALLOW_MEMBERS.into_iter().collect();
        if !create_answer["resourceId"].is_null() {
            expected_members.insert("resourceId");
        }
        let reported_members: BTreeSet<&str> = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a listed asset is an object, got {entry}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            reported_members, expected_members,
            "case {case:?}: the listed asset must carry exactly the modelled members, \
             got {entry}"
        );

        // The shallow projection is the same resource the single read reports.
        let (status, described) = call(&router, "GET", &format!("/assets/{id}"), None).await;
        assert_eq!(status, 200, "case {case:?}: DescribeAsset body {described}");
        for member in ASSET_SHALLOW_MEMBERS {
            assert_eq!(
                entry[member], described[member],
                "case {case:?}: the listing and the read must agree on {member}, the \
                 listing said {} and the read said {}",
                entry[member], described[member]
            );
        }
    }
}

/// The listing is reporting-only: reading it again — unfiltered, filtered, or paged to
/// the end one asset at a time — reports exactly the same thing, so a read changes
/// nothing a later caller sees.
#[tokio::test]
async fn reading_the_listing_changes_nothing_a_later_caller_sees() {
    let router = router();
    for (group, count) in [("g1", 2usize), (UUID_GROUP, 3)] {
        create_group(&router, group, None).await;
        for id in asset_ids(group, count) {
            create_asset(&router, &id, group).await;
        }
    }

    // (case, the query string to read twice)
    let cases: [(&str, String); 3] = [
        ("unfiltered", String::new()),
        ("filtered", format!("packagingGroupId={UUID_GROUP}")),
        ("filtered, one per page", format!("packagingGroupId={UUID_GROUP}&maxResults=1")),
    ];
    for (case, query) in cases {
        let (first, first_token) = list_page(&router, &query).await;
        // Page the whole listing to the end in between, so even a read that only a
        // paged walk would disturb is caught.
        let walked = ids_of(&list_all(&router, Some(UUID_GROUP), Some(1)).await);
        let (again, again_token) = list_page(&router, &query).await;
        assert_eq!(
            (first, first_token),
            (again, again_token),
            "case {case:?}: a second read must report the same page, after walking {walked:?}"
        );
    }

    // ... and every asset is still readable afterwards, in both listings.
    let after_filtered = ids_of(&list_all(&router, Some(UUID_GROUP), None).await);
    let after_unfiltered = ids_of(&list_all(&router, None, None).await);
    assert_eq!(
        after_filtered.len(),
        3,
        "the filtered listing must still report the group's three assets, got {after_filtered:?}"
    );
    assert_eq!(
        after_unfiltered.len(),
        5,
        "the unfiltered listing must still report all five assets, got {after_unfiltered:?}"
    );
}
