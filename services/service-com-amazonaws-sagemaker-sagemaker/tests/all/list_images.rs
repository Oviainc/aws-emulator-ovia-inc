// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker AI's `ListImages`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.ListImages`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness is the shared
//! one from `inference_deployment_lifecycle`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! documented `SortBy`/`SortOrder` pair, every filter member and both sides of each time
//! bound, every page size from one entry to past the published `MaxResults` bound, the
//! members each listed entry replays, and the created-status transition the clock owes.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    IMAGE_SORT_BY_VALUES, IMAGE_SORT_ORDER_VALUES,
};
use service_com_amazonaws_sagemaker_sagemaker::state::{IMAGE_CREATED_AFTER, MAX_PAGE_SIZE};

use crate::inference_deployment_lifecycle::{
    error_code, f64_field, str_field, Emulator, ACCOUNT, CREATE_SPACING,
};

/// A `RoleArn` satisfying the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/SageMakerExecution";

/// The image names the sort and filter tables range over, created in this order. Their
/// names are deliberately NOT in creation order, so an `IMAGE_NAME` sort and a
/// `CREATION_TIME` sort cannot accidentally agree.
const SEEDED: [&str; 4] = ["alpha-one", "beta-two", "demo-three", "demo-four"];

/// `MaxResults`' documented default for `ListImages` ("The default value is 10.").
const DOCUMENTED_DEFAULT_MAX_RESULTS: usize = 10;

/// The service's own server-side page ceiling, read from the service rather than
/// restated, so changing it does not fail this test for an unrelated reason.
const PAGE_CEILING: usize = MAX_PAGE_SIZE as usize;

/// Long enough to pass the `CREATING` -> `CREATED` instant the clock owes, derived from
/// the service's own span rather than restating it.
fn past_created_due() -> Duration {
    IMAGE_CREATED_AFTER * 2
}

/// Create one image per name, spaced so `CreationTime` ordering is unambiguous.
async fn create_images(emulator: &Emulator, names: &[String]) {
    for name in names {
        emulator
            .ok(
                "CreateImage",
                &json!({ "ImageName": name, "RoleArn": ROLE_ARN }),
            )
            .await;
        tokio::time::sleep(CREATE_SPACING).await;
    }
}

/// One image's stored `CreationTime` and `LastModifiedTime`, read back through
/// `DescribeImage`.
async fn stored_times(emulator: &Emulator, name: &str) -> (f64, f64) {
    let described = emulator
        .ok("DescribeImage", &json!({ "ImageName": name }))
        .await;
    (
        f64_field(&described, "CreationTime"),
        f64_field(&described, "LastModifiedTime"),
    )
}

/// Seed [`SEEDED`] and wait past the `CREATED` instant the clock owes, so every image's
/// status and `LastModifiedTime` have settled and the assertions below are not racing the
/// clock. Answers each image's `(name, CreationTime, LastModifiedTime)` in creation order.
async fn seed_settled_images(emulator: &Emulator) -> Vec<(String, f64, f64)> {
    let names: Vec<String> = SEEDED.iter().map(|name| name.to_string()).collect();
    create_images(emulator, &names).await;
    tokio::time::sleep(past_created_due()).await;

    let mut seeded = Vec::new();
    for name in &names {
        let (created, modified) = stored_times(emulator, name).await;
        seeded.push((name.clone(), created, modified));
    }
    seeded
}

/// The `ImageName`s a `ListImages` response lists, in response order.
fn listed_names(body: &Value) -> Vec<String> {
    body.get("Images")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an Images list in {body}"))
        .iter()
        .map(|image| str_field(image, "ImageName"))
        .collect()
}

/// RULE: `ListImages` returns the stored images filtered by `NameContains` and ordered by
/// `SortBy`/`SortOrder`, defaulting to `CREATION_TIME` / `DESCENDING` as both members'
/// model docs state, with the image name as the tie-break so equal sort keys keep one
/// order whichever way the page is sorted.
#[tokio::test]
async fn list_images_honours_every_documented_sort_and_filter() {
    let emulator = Emulator::new();
    seed_settled_images(&emulator).await;

    // Rows differ along sort field (each member of `ImageSortBy`), sort order (each
    // member of `ImageSortOrder`), and the name filter's selectivity. Every image is
    // settled, so `LAST_MODIFIED_TIME` orders them exactly as `CREATION_TIME` does: the
    // settled instant is the creation instant plus the one owed span.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "documented-defaults",
            json!({}),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "image-name-ascending",
            json!({ "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "image-name-descending",
            json!({ "SortBy": "IMAGE_NAME", "SortOrder": "DESCENDING" }),
            vec!["demo-three", "demo-four", "beta-two", "alpha-one"],
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CREATION_TIME", "SortOrder": "ASCENDING" }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CREATION_TIME", "SortOrder": "DESCENDING" }),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "last-modified-time-ascending",
            json!({ "SortBy": "LAST_MODIFIED_TIME", "SortOrder": "ASCENDING" }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "last-modified-time-descending",
            json!({ "SortBy": "LAST_MODIFIED_TIME", "SortOrder": "DESCENDING" }),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "sort-order-alone-defaults-the-sort-field",
            json!({ "SortOrder": "ASCENDING" }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "sort-field-alone-defaults-the-order",
            json!({ "SortBy": "IMAGE_NAME" }),
            vec!["demo-three", "demo-four", "beta-two", "alpha-one"],
        ),
        (
            "name-contains-two-matches",
            json!({ "NameContains": "demo", "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
            vec!["demo-four", "demo-three"],
        ),
        (
            "name-contains-one-match",
            json!({ "NameContains": "alpha", "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
            vec!["alpha-one"],
        ),
        (
            "name-contains-an-infix-rather-than-a-prefix",
            json!({ "NameContains": "-t", "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
            vec!["beta-two", "demo-three"],
        ),
        (
            "name-contains-no-match",
            json!({ "NameContains": "nothing-here" }),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListImages", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListImages order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
    }

    // Every documented pair of the two enums is accepted, whatever the rows above cover.
    for sort_by in IMAGE_SORT_BY_VALUES {
        for sort_order in IMAGE_SORT_ORDER_VALUES {
            let listed = emulator
                .ok(
                    "ListImages",
                    &json!({ "SortBy": sort_by, "SortOrder": sort_order }),
                )
                .await;
            assert_eq!(
                listed_names(&listed).len(),
                SEEDED.len(),
                "case {sort_by}/{sort_order}: every stored image must be listed, got {listed:?}"
            );
        }
    }
}

/// RULE: every listed entry replays the stored image — its name, its minted ARN, the
/// creation and last-modified instants the service recorded, its current status and the
/// optional members the create stored — and it reports exactly what the reader does. An
/// image removed from state stops being listed.
#[tokio::test]
async fn list_images_entries_replay_the_stored_image() {
    let emulator = Emulator::new();

    // Rows differ along which optional members the create carried, so an absent member is
    // omitted from the listing rather than reported as an empty string.
    let described_name = "described-image";
    let bare_name = "bare-image";
    emulator
        .ok(
            "CreateImage",
            &json!({
                "ImageName": described_name,
                "RoleArn": ROLE_ARN,
                "Description": "an image with every optional member",
                "DisplayName": "Described Image",
            }),
        )
        .await;
    tokio::time::sleep(CREATE_SPACING).await;
    emulator
        .ok(
            "CreateImage",
            &json!({ "ImageName": bare_name, "RoleArn": ROLE_ARN }),
        )
        .await;
    tokio::time::sleep(past_created_due()).await;

    let listed = emulator
        .ok(
            "ListImages",
            &json!({ "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
        )
        .await;
    let entries = listed
        .get("Images")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an Images list in {listed}"))
        .clone();
    assert_eq!(
        listed_names(&listed),
        vec![bare_name.to_string(), described_name.to_string()],
        "every stored image must be listed, got {listed:?}"
    );

    for entry in &entries {
        let name = str_field(entry, "ImageName");
        let described = emulator
            .ok("DescribeImage", &json!({ "ImageName": name }))
            .await;
        assert_eq!(
            str_field(entry, "ImageArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:image/{name}"),
            "case {name}: ImageArn in the listed entry, got {entry:?}"
        );
        for member in ["ImageArn", "ImageStatus"] {
            assert_eq!(
                str_field(entry, member),
                str_field(&described, member),
                "case {name}: {member} must be what DescribeImage reports, got {entry:?}"
            );
        }
        for member in ["CreationTime", "LastModifiedTime"] {
            assert_eq!(
                f64_field(entry, member),
                f64_field(&described, member),
                "case {name}: {member} must be the stored instant, got {entry:?}"
            );
        }
        for member in ["Description", "DisplayName"] {
            assert_eq!(
                entry.get(member),
                described.get(member),
                "case {name}: {member} must be replayed exactly as stored, got {entry:?}"
            );
        }
        assert_eq!(
            str_field(entry, "ImageStatus"),
            "CREATED",
            "case {name}: a settled image reports CREATED, got {entry:?}"
        );
        assert!(
            entry.get("FailureReason").is_none(),
            "case {name}: an image that did not fail carries no FailureReason, got {entry:?}"
        );
    }

    // A removed image stops being listed, and only that one.
    emulator
        .ok("DeleteImage", &json!({ "ImageName": bare_name }))
        .await;
    let listed = emulator
        .ok("ListImages", &json!({ "SortBy": "IMAGE_NAME" }))
        .await;
    assert_eq!(
        listed_names(&listed),
        vec![described_name.to_string()],
        "case delete-one: a deleted image must not be listed, got {listed:?}"
    );
}

/// RULE: each of the four time bounds selects exactly the images on its side of the
/// instant, and the bounds compose with each other and with `NameContains`. The bounds are
/// placed between neighbouring instants, which is the finest split the `awsJson1_1`
/// epoch-seconds `Timestamp` represents without loss.
#[tokio::test]
async fn list_images_filters_on_the_creation_and_last_modified_time_bounds() {
    let emulator = Emulator::new();
    let seeded = seed_settled_images(&emulator).await;
    // Bounds strictly between two neighbours, so each split is unambiguous.
    let created_split = (seeded[1].1 + seeded[2].1) / 2.0;
    let created_split_upper = (seeded[2].1 + seeded[3].1) / 2.0;
    let modified_split = (seeded[1].2 + seeded[2].2) / 2.0;

    // Rows differ along which bound is used, which side it selects, how many bounds
    // compose, and whether a bound sits exactly on a stored instant.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "creation-time-before-the-split",
            json!({
                "CreationTimeBefore": created_split,
                "SortBy": "CREATION_TIME",
                "SortOrder": "ASCENDING",
            }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "creation-time-after-the-split",
            json!({
                "CreationTimeAfter": created_split,
                "SortBy": "CREATION_TIME",
                "SortOrder": "ASCENDING",
            }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "creation-time-both-bounds-around-one-image",
            json!({
                "CreationTimeAfter": created_split,
                "CreationTimeBefore": created_split_upper,
                "SortBy": "CREATION_TIME",
                "SortOrder": "ASCENDING",
            }),
            vec!["demo-three"],
        ),
        (
            "creation-time-bound-just-past-the-last-instant-selects-every-image",
            json!({
                "CreationTimeBefore": seeded[3].1 + 0.001,
                "SortBy": "CREATION_TIME",
                "SortOrder": "ASCENDING",
            }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-time-bound-that-excludes-everything",
            json!({ "CreationTimeAfter": seeded[3].1 + 3600.0 }),
            vec![],
        ),
        (
            "last-modified-time-before-the-split",
            json!({
                "LastModifiedTimeBefore": modified_split,
                "SortBy": "CREATION_TIME",
                "SortOrder": "ASCENDING",
            }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "last-modified-time-after-the-split",
            json!({
                "LastModifiedTimeAfter": modified_split,
                "SortBy": "CREATION_TIME",
                "SortOrder": "ASCENDING",
            }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "last-modified-time-bound-just-before-the-first-instant-selects-nothing",
            json!({
                "LastModifiedTimeBefore": seeded[0].2 - 0.001,
                "SortBy": "CREATION_TIME",
                "SortOrder": "ASCENDING",
            }),
            vec![],
        ),
        (
            "creation-and-last-modified-bounds-compose",
            json!({
                "CreationTimeAfter": created_split,
                "LastModifiedTimeBefore": (seeded[2].2 + seeded[3].2) / 2.0,
                "SortBy": "CREATION_TIME",
                "SortOrder": "ASCENDING",
            }),
            vec!["demo-three"],
        ),
        (
            "bound-combined-with-a-name-filter",
            json!({
                "CreationTimeAfter": created_split,
                "NameContains": "demo",
                "SortBy": "IMAGE_NAME",
                "SortOrder": "ASCENDING",
            }),
            vec!["demo-four", "demo-three"],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListImages", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListImages selection, got {listed:?}"
        );
    }
}

/// RULE: paging walks the whole ordered result exactly once — each page holds at most the
/// effective page size, a `NextToken` is present only while more remain, and feeding it
/// back resumes immediately after the previous page. The effective size is `MaxResults`
/// where the caller sends one (capped by the service's own ceiling) and this operation's
/// documented default of 10 where it does not.
#[tokio::test]
async fn list_images_paging_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    // More images than the documented default page, so a defaulted page and an explicitly
    // sized one are distinguishable.
    let names: Vec<String> = (0..12).map(|index| format!("img-{index:02}")).collect();
    create_images(&emulator, &names).await;
    tokio::time::sleep(past_created_due()).await;

    // Rows: every page size from one entry up to past the end of the result set, plus both
    // published bounds of `MaxResults` (`range 1..100`, from
    // smithy_query(query="operation",operation="ListImages")).
    for page_size in [1_usize, 2, 5, 11, 12, 13, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({
                "MaxResults": page_size,
                "SortBy": "IMAGE_NAME",
                "SortOrder": "ASCENDING",
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListImages", &body).await;
            let page = listed_names(&listed);
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
                pages <= names.len() + 1,
                "case {case}: paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case {case}: paging must walk the whole ordered result exactly once"
        );
    }

    // No `MaxResults`: the page is this operation's documented default of 10, and the
    // token still walks the rest.
    let first = emulator
        .ok(
            "ListImages",
            &json!({ "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
        )
        .await;
    assert_eq!(
        listed_names(&first).len(),
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
            "ListImages",
            &json!({ "NextToken": token, "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
        )
        .await;
    assert_eq!(
        listed_names(&second),
        names[DOCUMENTED_DEFAULT_MAX_RESULTS..].to_vec(),
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
async fn the_image_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    let emulator = Emulator::new();
    // Seeded past the ceiling so a capped page and an uncapped one are distinguishable.
    let names: Vec<String> = (0..PAGE_CEILING + 3)
        .map(|index| format!("img-{index:03}"))
        .collect();
    for name in &names {
        emulator
            .ok(
                "CreateImage",
                &json!({ "ImageName": name, "RoleArn": ROLE_ARN }),
            )
            .await;
    }

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({
            "MaxResults": 100,
            "SortBy": "IMAGE_NAME",
            "SortOrder": "ASCENDING",
        });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("ListImages", &body).await;
        let page = listed_names(&listed);
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
    assert_eq!(
        seen, names,
        "case ceiling: the resume token must still walk the whole result"
    );
}

/// RULE: the listing reports the status the clock owes — `CREATING` before the owed
/// instant and `CREATED` from it on — it agrees with the reader, and the settled value is
/// stable across later reads because the transition is materialised rather than recomputed
/// per call.
#[tokio::test]
async fn list_images_reports_the_created_status_the_clock_owes() {
    let emulator = Emulator::new();
    let names: Vec<String> = SEEDED.iter().map(|name| name.to_string()).collect();
    let sorted: Vec<String> = {
        let mut sorted = names.clone();
        sorted.sort();
        sorted
    };

    // Before the owed transition is due: the image is still `CREATING`. ONE create and
    // ONE listing carry the pre-due assertion, so it cannot be lost to a host slow enough
    // to spend the transition's span on the wire calls themselves.
    emulator
        .ok(
            "CreateImage",
            &json!({ "ImageName": &names[0], "RoleArn": ROLE_ARN }),
        )
        .await;
    let listed = emulator
        .ok(
            "ListImages",
            &json!({ "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
        )
        .await;
    let statuses: Vec<String> = listed
        .get("Images")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an Images list in {listed}"))
        .iter()
        .map(|image| str_field(image, "ImageStatus"))
        .collect();
    assert_eq!(
        statuses,
        vec!["CREATING".to_string()],
        "case before-due: a freshly created image is reported CREATING, got {listed:?}"
    );
    assert_eq!(
        listed_names(&listed),
        vec![names[0].clone()],
        "case before-due: the created image is listed, got {listed:?}"
    );

    // Past the owed transition: every image is `CREATED`, the reader agrees, and the
    // settled instants do not move on a later listing.
    for name in &names[1..] {
        emulator
            .ok(
                "CreateImage",
                &json!({ "ImageName": name, "RoleArn": ROLE_ARN }),
            )
            .await;
    }
    tokio::time::sleep(past_created_due()).await;
    let settled = emulator
        .ok(
            "ListImages",
            &json!({ "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
        )
        .await;
    let entries = settled
        .get("Images")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an Images list in {settled}"))
        .clone();
    assert_eq!(
        listed_names(&settled),
        sorted,
        "case past-due: every created image is listed, got {settled:?}"
    );
    for entry in &entries {
        let name = str_field(entry, "ImageName");
        assert_eq!(
            str_field(entry, "ImageStatus"),
            "CREATED",
            "case {name}: past the owed instant an image is CREATED, got {entry:?}"
        );
        let described = emulator
            .ok("DescribeImage", &json!({ "ImageName": &name }))
            .await;
        assert_eq!(
            str_field(&described, "ImageStatus"),
            "CREATED",
            "case {name}: DescribeImage must agree with the listing, got {described:?}"
        );
    }

    let again = emulator
        .ok(
            "ListImages",
            &json!({ "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
        )
        .await;
    assert_eq!(
        again.get("Images"),
        settled.get("Images"),
        "case stability: a settled listing must not change on a later read, got {again:?}"
    );
}

/// RULE: an input outside a documented value set is refused with HTTP 400 rather than
/// silently ignored, and the refusal writes nothing.
#[tokio::test]
async fn list_images_refuses_inputs_outside_the_documented_value_sets() {
    let emulator = Emulator::new();
    seed_settled_images(&emulator).await;

    // Rows: one outside each documented value set — `ImageSortBy`, `ImageSortOrder`, an
    // unminted `NextToken`, `MaxResults` one past each published bound (`range 1..100`),
    // and `NameContains` outside `^[a-zA-Z0-9\-.]+$` / len 0..63.
    let too_long = "a".repeat(64);
    let cases: Vec<(&str, Value)> = vec![
        ("sort-by-outside-the-enum", json!({ "SortBy": "Nope" })),
        (
            "sort-by-in-the-wrong-case",
            json!({ "SortBy": "CreationTime" }),
        ),
        (
            "sort-order-outside-the-enum",
            json!({ "SortOrder": "Sideways" }),
        ),
        (
            "sort-order-in-the-wrong-case",
            json!({ "SortOrder": "Descending" }),
        ),
        (
            "next-token-that-names-no-boundary",
            json!({ "NextToken": "not-a-token" }),
        ),
        ("max-results-below-1", json!({ "MaxResults": 0 })),
        ("max-results-above-100", json!({ "MaxResults": 101 })),
        (
            "name-contains-with-an-illegal-character",
            json!({ "NameContains": "bad name" }),
        ),
        (
            "name-contains-past-63",
            json!({ "NameContains": too_long.clone() }),
        ),
    ];

    for (case, body) in cases {
        let (status, parsed) = emulator.call("ListImages", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: ListImages must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: refusal code, got {parsed}"
        );
    }

    // Nothing above reached state: the seeded images are all still listed.
    let listed = emulator
        .ok(
            "ListImages",
            &json!({ "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING" }),
        )
        .await;
    assert_eq!(
        listed_names(&listed).len(),
        SEEDED.len(),
        "a refused request must write nothing, got {listed:?}"
    );
}

/// RULE: a listing over a store holding no image answers an empty result rather than
/// refusing, and carries no resume token.
#[tokio::test]
async fn list_images_over_an_empty_store_answers_an_empty_page() {
    let emulator = Emulator::new();

    for (case, body) in [
        ("no-members", json!({})),
        ("with-a-name-filter", json!({ "NameContains": "anything" })),
        (
            "with-a-sort-and-a-page-size",
            json!({ "SortBy": "IMAGE_NAME", "SortOrder": "ASCENDING", "MaxResults": 1 }),
        ),
    ] {
        let listed = emulator.ok("ListImages", &body).await;
        assert_eq!(
            listed_names(&listed),
            Vec::<String>::new(),
            "case {case}: an empty store lists nothing, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: an empty page must carry no NextToken, got {listed:?}"
        );
    }
}
