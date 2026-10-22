// SPDX-License-Identifier: CC0-1.0

//! `DescribeWorkspaceBundles`, end to end over the awsJson1_1 wire.
//!
//! Every case drives a real `MultiServiceRouter` with this service mounted on one shared
//! engine, so each assertion is about what a client would see.
//!
//! The rules under test, stated over the SET of bundles the store holds rather than over
//! any one recorded request:
//!
//! * A bundle is reported from persisted state, with every member the model's
//!   `WorkspaceBundle` declares that this emulator has state for, and the same values on
//!   every read (describing is a report, never a write).
//! * `Owner` selects by owner: `AMAZON` reports the Amazon-provided catalog, an absent
//!   `Owner` reports the bundles this account owns ("To describe the bundles that belong
//!   to your account, don't specify a value"), and any other owner reports that owner's.
//! * `BundleIds` names bundles outright, whatever their owner, and reports only the ones
//!   the store holds.
//! * A result longer than one page is paginated: each page but the last is full and
//!   carries a `NextToken` that resumes strictly after it, so a walk reports every
//!   matching bundle exactly once.
//! * Combining `BundleIds` with `Owner`, or sending a `NextToken` this service never
//!   minted, is refused with `InvalidParameterValuesException` (HTTP 400).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::ops::types::{
    BUNDLE_TYPE_VALUES, COMPUTE_VALUES, WORKSPACE_BUNDLE_STATE_VALUES,
};
use service_com_amazonaws_workspaces_workspacesservice::state::{
    service_state, NewBundle, AMAZON_BUNDLE_OWNER, DESCRIBE_BUNDLE_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_lifecycle::{call, emulator_with_engine};

/// Persist `count` bundles owned by this account and return their minted ids, in
/// creation order.
///
/// The store's writer is `CreateWorkspaceBundle`, which is still a scaffold stub, so this
/// suite reaches `create_bundle` through the same state the handlers see — exactly as the
/// lifecycle suite settles the clock through it.
fn seed_account_bundles(engine: &Arc<engine::InMemoryEngine>, count: usize) -> Vec<String> {
    let state = service_state(&**engine).expect("the WorkSpaces state store");
    let mut state = state.lock();
    (0..count)
        .map(|index| {
            // The compute type walks the model's `Compute` enum, so the seeded
            // bundles differ in more than their name.
            let compute = COMPUTE_VALUES[index % COMPUTE_VALUES.len()];
            let request = NewBundle {
                name: format!("account-bundle-{index}"),
                description: format!("bundle {index} owned by this account"),
                image_id: format!("wsi-account{index:04}"),
                compute_name: compute.to_string(),
                root_capacity_gib: 80,
                user_capacity_gib: 100,
            };
            match state.create_bundle(&request) {
                Ok(record) => record.bundle_id,
                Err(failure) => {
                    panic!("case {index}: create_bundle must persist a bundle, got {failure:?}")
                }
            }
        })
        .collect()
}

/// Walk every page of `DescribeWorkspaceBundles` for `filters` and return the bundles
/// reported, asserting the paging contract on the way: a page is never longer than the
/// page size, a page that carries a `NextToken` is full, and the walk terminates.
async fn walk_bundles(router: &MultiServiceRouter, filters: &Value) -> Vec<Value> {
    let mut reported: Vec<Value> = Vec::new();
    let mut page_filters = filters.clone();
    let mut pages = 0usize;
    loop {
        let (status, body) = call(router, "DescribeWorkspaceBundles", &page_filters).await;
        assert_eq!(
            status, 200,
            "case {filters:?}: DescribeWorkspaceBundles must answer 200, body {body:?}"
        );
        let page = body["Bundles"]
            .as_array()
            .unwrap_or_else(|| panic!("case {filters:?}: Bundles must be a list, got {body:?}"))
            .clone();
        assert!(
            page.len() <= DESCRIBE_BUNDLE_PAGE_SIZE,
            "case {filters:?}: a page holds at most {DESCRIBE_BUNDLE_PAGE_SIZE} bundles, got {}",
            page.len()
        );
        pages += 1;
        assert!(
            pages <= 64,
            "case {filters:?}: paging must terminate, walked {} bundles",
            reported.len()
        );
        let token = body["NextToken"].as_str().map(str::to_string);
        if token.is_some() {
            assert_eq!(
                page.len(),
                DESCRIBE_BUNDLE_PAGE_SIZE,
                "case {filters:?}: a page that carries a NextToken is full, got {}",
                page.len()
            );
        }
        reported.extend(page);
        match token {
            Some(token) => page_filters["NextToken"] = json!(token),
            None => break,
        }
    }
    reported
}

/// The `BundleId` a reported bundle carries.
fn bundle_id(bundle: &Value) -> String {
    bundle["BundleId"]
        .as_str()
        .unwrap_or_else(|| panic!("every bundle carries a BundleId, got {bundle:?}"))
        .to_string()
}

/// Does `value` match `^{prefix}[0-9a-z]{{min,max}}$`?
///
/// The two patterns the model states for the ids a bundle carries —
/// `BundleId` is `^wsb-[0-9a-z]{8,63}$` and `WorkspaceImageId` is
/// `^wsi-[0-9a-z]{9,63}$` — are both of this form.
fn matches_id(value: &str, prefix: &str, min: usize, max: usize) -> bool {
    let Some(rest) = value.strip_prefix(prefix) else {
        return false;
    };
    (min..=max).contains(&rest.len())
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// A capacity member (`RootStorage$Capacity`, `UserStorage$Capacity`) as GiB.
fn capacity_gib(bundle: &Value, member: &str) -> u32 {
    let text = bundle[member]["Capacity"].as_str().unwrap_or_else(|| {
        panic!("case {member}: a bundle carries a {member} capacity string, got {bundle:?}")
    });
    text.parse().unwrap_or_else(|error| {
        panic!("case {member}: the capacity {text:?} must be a number, got {error}")
    })
}

#[tokio::test]
async fn amazon_owned_bundles_report_every_member_the_store_holds() {
    let (router, _engine) = emulator_with_engine();

    let bundles = walk_bundles(&router, &json!({ "Owner": AMAZON_BUNDLE_OWNER })).await;

    // The catalog holds one Amazon-owned bundle per compute type the model's `Compute`
    // enum declares (`COMPUTE_VALUES` in `ops::types`).
    let computes: BTreeSet<String> = bundles
        .iter()
        .map(|bundle| {
            bundle["ComputeType"]["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("a bundle carries a ComputeType.Name, got {bundle:?}"))
                .to_string()
        })
        .collect();
    let expected: BTreeSet<String> = COMPUTE_VALUES.iter().map(|value| (*value).to_string()).collect();
    assert_eq!(
        computes, expected,
        "the Amazon catalog covers every modelled compute type exactly once"
    );
    assert_eq!(
        bundles.len(),
        COMPUTE_VALUES.len(),
        "the Amazon catalog reports one bundle per compute type, got {} bundles",
        bundles.len()
    );

    for bundle in &bundles {
        let case = bundle_id(bundle);
        assert!(
            matches_id(&case, "wsb-", 8, 63),
            "case {case:?}: BundleId must match ^wsb-[0-9a-z]{{8,63}}$"
        );
        let image_id = bundle["ImageId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a bundle carries an ImageId, got {bundle:?}"));
        assert!(
            matches_id(image_id, "wsi-", 9, 63),
            "case {case:?}: ImageId must match ^wsi-[0-9a-z]{{9,63}}$, got {image_id:?}"
        );
        assert_eq!(
            bundle["Owner"].as_str(),
            Some(AMAZON_BUNDLE_OWNER),
            "case {case:?}: an Amazon-provided bundle is owned by AMAZON, got {bundle:?}"
        );
        for member in ["Name", "Description"] {
            let text = bundle[member].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a bundle carries a {member}, got {bundle:?}")
            });
            assert!(
                !text.is_empty(),
                "case {case:?}: {member} must not be empty, got {bundle:?}"
            );
        }
        let state = bundle["State"].as_str().unwrap_or_default();
        assert!(
            WORKSPACE_BUNDLE_STATE_VALUES.contains(&state),
            "case {case:?}: State must be a modelled WorkspaceBundleState, got {state:?}"
        );
        let bundle_type = bundle["BundleType"].as_str().unwrap_or_default();
        assert!(
            BUNDLE_TYPE_VALUES.contains(&bundle_type),
            "case {case:?}: BundleType must be a modelled BundleType, got {bundle_type:?}"
        );
        for member in ["RootStorage", "UserStorage"] {
            let capacity = capacity_gib(bundle, member);
            assert!(
                capacity > 0,
                "case {case:?}: {member} capacity must be positive, got {capacity}"
            );
        }
        for member in ["CreationTime", "LastUpdatedTime"] {
            let seconds = bundle[member].as_f64().unwrap_or_else(|| {
                panic!("case {case:?}: a bundle carries a {member} timestamp, got {bundle:?}")
            });
            assert!(
                seconds > 0.0,
                "case {case:?}: {member} must be an instant after the epoch, got {seconds}"
            );
        }
    }

    // An entry picked from the middle of the catalog, to pin that it is the whole
    // compute range and not just the common tiers.
    let graphics = bundles
        .iter()
        .find(|bundle| bundle["ComputeType"]["Name"].as_str() == Some("GRAPHICS_G6_8XLARGE"))
        .unwrap_or_else(|| panic!("the catalog holds a GRAPHICS_G6_8XLARGE bundle, got {bundles:?}"));
    assert_eq!(
        capacity_gib(graphics, "RootStorage"),
        80,
        "case GRAPHICS_G6_8XLARGE: the catalog's root volume is 80 GiB, got {graphics:?}"
    );

    // Describing is a report: a second walk answers exactly the same bundles.
    let again = walk_bundles(&router, &json!({ "Owner": AMAZON_BUNDLE_OWNER })).await;
    assert_eq!(
        again, bundles,
        "two reads of the catalog must report identical bundles"
    );
}

#[tokio::test]
async fn owner_selects_the_bundles_that_owner_owns() {
    let (router, engine) = emulator_with_engine();
    let account = engine::emulator_account_id();

    // Nothing has created a bundle yet, so this account owns none.
    let (status, body) = call(&router, "DescribeWorkspaceBundles", &json!({})).await;
    assert_eq!(status, 200, "an empty request must answer 200, body {body:?}");
    assert_eq!(
        body["Bundles"].as_array().map(Vec::len),
        Some(0),
        "this account owns no bundle until one is created, body {body:?}"
    );
    assert!(
        body["NextToken"].is_null(),
        "a single short page carries no NextToken, body {body:?}"
    );

    let created = seed_account_bundles(&engine, 3);

    // Each row is an `Owner` filter and the ids it must report.
    let catalog: Vec<String> = walk_bundles(&router, &json!({ "Owner": AMAZON_BUNDLE_OWNER }))
        .await
        .iter()
        .map(bundle_id)
        .collect();
    let cases: Vec<(Value, Vec<String>)> = vec![
        (json!({}), created.clone()),
        (json!({ "Owner": account }), created.clone()),
        (json!({ "Owner": AMAZON_BUNDLE_OWNER }), catalog),
        // An account that owns nothing in this store.
        (json!({ "Owner": "999999999999" }), Vec::new()),
    ];

    for (filters, expected) in cases {
        let reported: Vec<String> = walk_bundles(&router, &filters)
            .await
            .iter()
            .map(bundle_id)
            .collect();
        assert_eq!(
            reported, expected,
            "case {filters:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // An account-owned bundle reports the account id as its owner, not AMAZON.
    let mine = walk_bundles(&router, &json!({})).await;
    for bundle in &mine {
        assert_eq!(
            bundle["Owner"].as_str(),
            Some(account),
            "case {:?}: a bundle this account created is owned by it, got {bundle:?}",
            bundle_id(bundle)
        );
    }
    assert_eq!(
        mine.iter().map(bundle_id).collect::<Vec<String>>(),
        created,
        "the bundles created through the store are read back in creation order"
    );
}

#[tokio::test]
async fn bundle_ids_report_exactly_the_stored_bundles_named() {
    let (router, engine) = emulator_with_engine();
    let account_bundles = seed_account_bundles(&engine, 2);
    let catalog: Vec<String> = walk_bundles(&router, &json!({ "Owner": AMAZON_BUNDLE_OWNER }))
        .await
        .iter()
        .map(bundle_id)
        .collect();

    // A `BundleId` that matches the model's pattern but names nothing stored.
    let unknown = "wsb-nosuchbundle";

    // Each row is the `BundleIds` a request names and the ids it must report; the
    // rows differ in count, in which owner the named bundles belong to, in ordering
    // and in whether an id names a stored bundle at all.
    let cases: Vec<(Vec<String>, Vec<String>)> = vec![
        (vec![catalog[0].clone()], vec![catalog[0].clone()]),
        (
            vec![catalog[0].clone(), catalog[4].clone(), catalog[9].clone()],
            vec![catalog[0].clone(), catalog[4].clone(), catalog[9].clone()],
        ),
        // Reversing the request order changes nothing about which bundles exist.
        (
            vec![catalog[9].clone(), catalog[4].clone(), catalog[0].clone()],
            vec![catalog[0].clone(), catalog[4].clone(), catalog[9].clone()],
        ),
        // An id nobody created is simply absent from the result.
        (
            vec![unknown.to_string(), catalog[1].clone()],
            vec![catalog[1].clone()],
        ),
        (vec![unknown.to_string()], Vec::new()),
        // `BundleIds` names bundles whatever their owner, mixing both owners.
        (
            vec![account_bundles[1].clone(), catalog[2].clone()],
            vec![catalog[2].clone(), account_bundles[1].clone()],
        ),
        (account_bundles.clone(), account_bundles.clone()),
    ];

    for (requested, expected) in cases {
        let filters = json!({ "BundleIds": requested });
        let reported: BTreeSet<String> = walk_bundles(&router, &filters)
            .await
            .iter()
            .map(bundle_id)
            .collect();
        let expected: BTreeSet<String> = expected.into_iter().collect();
        assert_eq!(
            reported, expected,
            "case {requested:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn paging_walks_every_matching_bundle_exactly_once() {
    // Each row is a number of account-owned bundles, spanning both sides of the page
    // boundary, and gets its own emulator so the store holds exactly that many.
    let counts = [
        0,
        1,
        DESCRIBE_BUNDLE_PAGE_SIZE - 1,
        DESCRIBE_BUNDLE_PAGE_SIZE,
        DESCRIBE_BUNDLE_PAGE_SIZE + 1,
        2 * DESCRIBE_BUNDLE_PAGE_SIZE + 3,
    ];

    for count in counts {
        let (router, engine) = emulator_with_engine();
        let created = seed_account_bundles(&engine, count);
        let walked: Vec<String> = walk_bundles(&router, &json!({}))
            .await
            .iter()
            .map(bundle_id)
            .collect();
        assert_eq!(
            walked, created,
            "case count={count}: paging must walk every bundle once, in creation order"
        );

        // The shape of the first page: full plus a NextToken whenever a bundle is
        // left beyond it, and nothing more once the walk is done.
        let (status, first) = call(&router, "DescribeWorkspaceBundles", &json!({})).await;
        assert_eq!(status, 200, "case count={count}: must answer 200, body {first:?}");
        let page = first["Bundles"].as_array().map(Vec::len).unwrap_or_default();
        let expected_page = count.min(DESCRIBE_BUNDLE_PAGE_SIZE);
        assert_eq!(
            page, expected_page,
            "case count={count}: the first page holds {expected_page} bundles, body {first:?}"
        );
        assert_eq!(
            first["NextToken"].is_string(),
            count > DESCRIBE_BUNDLE_PAGE_SIZE,
            "case count={count}: a NextToken is reported exactly when a bundle is left, body {first:?}"
        );
    }

    // The Amazon catalog is longer than one page, so the same contract holds for a
    // result this emulator did not have to be told to create.
    let (router, _engine) = emulator_with_engine();
    let (_, first) = call(
        &router,
        "DescribeWorkspaceBundles",
        &json!({ "Owner": AMAZON_BUNDLE_OWNER }),
    )
    .await;
    let token = first["NextToken"].as_str().unwrap_or_else(|| {
        panic!("the catalog is longer than one page, so its first page carries a NextToken, got {first:?}")
    });
    let resumed = json!({ "Owner": AMAZON_BUNDLE_OWNER, "NextToken": token });
    let (status, second) = call(&router, "DescribeWorkspaceBundles", &resumed).await;
    assert_eq!(status, 200, "case {resumed:?}: must answer 200, body {second:?}");
    let first_ids: Vec<String> = first["Bundles"]
        .as_array()
        .map(|page| page.iter().map(bundle_id).collect())
        .unwrap_or_default();
    let second_ids: Vec<String> = second["Bundles"]
        .as_array()
        .map(|page| page.iter().map(bundle_id).collect())
        .unwrap_or_default();
    assert_eq!(
        first_ids.len(),
        DESCRIBE_BUNDLE_PAGE_SIZE,
        "the catalog's first page is full, got {first_ids:?}"
    );
    assert_eq!(
        second_ids.len(),
        COMPUTE_VALUES.len() - DESCRIBE_BUNDLE_PAGE_SIZE,
        "the catalog's second page holds the rest, got {second_ids:?}"
    );
    assert!(
        first_ids.iter().all(|id| !second_ids.contains(id)),
        "the second page repeats nothing from the first: {first_ids:?} then {second_ids:?}"
    );
}

#[tokio::test]
async fn combined_filters_and_unminted_tokens_are_refused() {
    let (router, engine) = emulator_with_engine();
    seed_account_bundles(&engine, 1);

    // `BundleIds` and `Owner` each document "You cannot combine this parameter with
    // any other filter", and a NextToken this service never minted names no page.
    let cases: Vec<Value> = vec![
        json!({ "BundleIds": ["wsb-amznvalue"], "Owner": AMAZON_BUNDLE_OWNER }),
        json!({ "BundleIds": ["wsb-amznvalue"], "Owner": "000000000000" }),
        json!({ "NextToken": "not-a-token" }),
        // The service mints a fixed-width lower-case hex token; neither a shorter
        // one nor an upper-case spelling is one of them.
        json!({ "NextToken": "0000000000000001FF" }),
        json!({ "NextToken": "000000000000000F" }),
        json!({ "NextToken": "1" }),
        json!({ "Owner": AMAZON_BUNDLE_OWNER, "NextToken": "not-a-token" }),
    ];

    for filters in cases {
        let (status, body) = call(&router, "DescribeWorkspaceBundles", &filters).await;
        assert_eq!(
            status, 400,
            "case {filters:?}: the request must be refused, body {body:?}"
        );
        let kind = body["__type"].as_str().unwrap_or_default();
        assert!(
            kind.ends_with("InvalidParameterValuesException"),
            "case {filters:?}: expected InvalidParameterValuesException, got {body:?}"
        );
    }

    // A token this service DID mint is accepted, so the refusals above are about the
    // token's provenance and not about sending one at all.
    let (_, first) = call(
        &router,
        "DescribeWorkspaceBundles",
        &json!({ "Owner": AMAZON_BUNDLE_OWNER }),
    )
    .await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the catalog's first page carries a NextToken, got {first:?}"));
    let resumed = json!({ "Owner": AMAZON_BUNDLE_OWNER, "NextToken": token });
    let (status, body) = call(&router, "DescribeWorkspaceBundles", &resumed).await;
    assert_eq!(
        status, 200,
        "case {resumed:?}: a minted token must be accepted, body {body:?}"
    );
}
