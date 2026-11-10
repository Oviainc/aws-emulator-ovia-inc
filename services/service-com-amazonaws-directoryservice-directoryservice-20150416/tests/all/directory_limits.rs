// SPDX-License-Identifier: CC0-1.0

//! `GetDirectoryLimits`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body — and
//! builds the state it reports on through `CreateDirectory`, `CreateMicrosoftAD` and
//! `DeleteDirectory`. Nothing reaches into the state struct, so a response that is not
//! computed from persisted state cannot pass. The harness (router, call helpers, quotas)
//! is the sibling suites', reached as `crate::directory_lifecycle::<item>` and
//! `crate::microsoft_ad::<item>`.
//!
//! The rule under test: each counter of the modelled `DirectoryLimits` structure reports
//! ITS OWN category's figures — that category's published Region quota as the limit, how
//! many stored directories fall in it (decided by the stored `DirectoryType`) as the
//! current count, and the comparison of the two as the limit-reached flag. The cases
//! assert it over a TABLE of stored populations differing in count and in the MIX of
//! directory types, over the delete direction, and at each category's own bound — rather
//! than over one recorded call.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    create_directory, minimal_create, ok, refused, service, CLOUD_ONLY_QUOTA, CONNECTED_QUOTA,
    MICROSOFT_AD_QUOTA,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The `DirectoryLimits` members of one category — its current count, its limit and its
/// limit-reached flag — paired with that category's published Region quota. The three
/// categories the structure carries
/// (`smithy_query(query="shape",shape="DirectoryLimits")`), in its own member order.
const COUNTERS: [(&str, &str, &str, usize); 3] = [
    (
        "CloudOnlyDirectoriesCurrentCount",
        "CloudOnlyDirectoriesLimit",
        "CloudOnlyDirectoriesLimitReached",
        CLOUD_ONLY_QUOTA,
    ),
    (
        "CloudOnlyMicrosoftADCurrentCount",
        "CloudOnlyMicrosoftADLimit",
        "CloudOnlyMicrosoftADLimitReached",
        MICROSOFT_AD_QUOTA,
    ),
    (
        "ConnectedDirectoriesCurrentCount",
        "ConnectedDirectoriesLimit",
        "ConnectedDirectoriesLimitReached",
        CONNECTED_QUOTA,
    ),
];

/// The `DirectoryLimits` structure of a `GetDirectoryLimits` answer, which the request
/// takes no members to narrow.
async fn limits(router: &MultiServiceRouter, case: &str) -> Value {
    let answer = ok(router, "GetDirectoryLimits", json!({}), case).await;
    let limits = answer["DirectoryLimits"].clone();
    assert!(
        limits.is_object(),
        "case {case:?}: expected a DirectoryLimits object, got {answer}"
    );
    limits
}

/// One member of a limits body as an integer.
fn number(limits: &Value, member: &str, case: &str) -> i64 {
    limits[member].as_i64().unwrap_or_else(|| {
        panic!("case {case:?}: expected {member} to be an integer, got {limits}")
    })
}

/// Assert the whole structure: every category's count, its own published quota and its
/// own flag, against the population the case built.
fn assert_limits(limits: &Value, counts: [i64; 3], case: &str) {
    for (index, (current, limit, reached, quota)) in COUNTERS.into_iter().enumerate() {
        let allowance = i64::try_from(quota).expect("the quota fits an i64");
        assert_eq!(
            number(limits, current, case),
            counts[index],
            "case {case:?}: expected {current} to be the number of stored directories of \
             that type, got {limits}"
        );
        assert_eq!(
            number(limits, limit, case),
            allowance,
            "case {case:?}: expected {limit} to be that category's own Region quota \
             {allowance}, got {limits}"
        );
        assert_eq!(
            limits[reached],
            json!(counts[index] >= allowance),
            "case {case:?}: expected {reached} to be on only once THAT category's quota \
             is used up ({} of {allowance} stored), got {limits}",
            counts[index]
        );
    }
}

#[tokio::test]
async fn a_fresh_region_reports_every_counter_empty_and_below_its_own_quota() {
    let router = service();
    let limits = limits(&router, "fresh Region").await;

    assert_limits(&limits, [0, 0, 0], "fresh Region");
    // Nothing is left absent: a client reading the structure sees all nine members.
    for (current, limit, reached, _) in COUNTERS {
        for member in [current, limit] {
            assert!(
                limits[member].is_i64(),
                "case \"fresh Region\": expected {member} to be reported, got {limits}"
            );
        }
        assert!(
            limits[reached].is_boolean(),
            "case \"fresh Region\": expected {reached} to be reported, got {limits}"
        );
    }
}

#[tokio::test]
async fn the_three_quotas_are_each_their_own_published_allowance() {
    // The limits are a property of the account/Region, not of the population, so they
    // are the same on a fresh Region and on one holding a mix of directories. The row
    // the recorded fixture covers (a fresh account) is the first of the two.
    for population in [0_usize, 3] {
        let case = format!("{population} Simple AD + {population} Managed Microsoft AD");
        let router = service();
        for index in 0..population {
            create_directory(
                &router,
                minimal_create(&format!("quota-simple{index}.example.com")),
                &case,
            )
            .await;
            create_microsoft_ad(
                &router,
                minimal_microsoft_ad(&format!("quota-managed{index}.example.com")),
                &case,
            )
            .await;
        }

        let limits = limits(&router, &case).await;
        // Each category's limit is read member by member, so a counter reported from
        // another category's allowance fails here whatever the counts are.
        for (_, limit, _, quota) in COUNTERS {
            assert_eq!(
                number(&limits, limit, &case),
                i64::try_from(quota).expect("the quota fits an i64"),
                "case {case:?}: expected {limit} to report its own published quota, got \
                 {limits}"
            );
        }
        // And the three are NOT one shared allowance: Managed Microsoft AD's is its own,
        // larger figure.
        assert!(
            number(&limits, "CloudOnlyMicrosoftADLimit", &case)
                > number(&limits, "CloudOnlyDirectoriesLimit", &case),
            "case {case:?}: expected the Managed Microsoft AD allowance to exceed the \
             cloud-only one, got {limits}"
        );
    }
}

#[tokio::test]
async fn each_counter_follows_the_stored_directories_of_its_own_type() {
    // The rows differ along what the rule generalises over: how many directories are
    // stored and the MIX of types that makes up the population — none, one of each,
    // several of one type only, and a population past the SMALLER of the two quotas so
    // the two counters cannot be read from one figure.
    // `[cloud-only (Simple AD), Managed Microsoft AD]`; the connected column stays zero
    // because `ConnectDirectory` is still a scaffold stub, so no AD Connector can be
    // persisted yet.
    let populations: [(usize, usize); 5] = [
        (0, 0),
        (1, 0),
        (0, 1),
        (3, 2),
        (CLOUD_ONLY_QUOTA, CLOUD_ONLY_QUOTA + 2),
    ];

    for (simple_ad, microsoft_ad) in populations {
        let case = format!("{simple_ad} Simple AD + {microsoft_ad} Managed Microsoft AD");
        // A fresh Region per row, so each row's report is derived from its own
        // population alone.
        let router = service();
        for index in 0..simple_ad {
            create_directory(
                &router,
                minimal_create(&format!("simple{index}.example.com")),
                &case,
            )
            .await;
        }
        for index in 0..microsoft_ad {
            create_microsoft_ad(
                &router,
                minimal_microsoft_ad(&format!("managed{index}.example.com")),
                &case,
            )
            .await;
        }

        let limits = limits(&router, &case).await;
        let counts = [
            i64::try_from(simple_ad).expect("the row fits an i64"),
            i64::try_from(microsoft_ad).expect("the row fits an i64"),
            0,
        ];
        assert_limits(&limits, counts, &case);
    }
}

#[tokio::test]
async fn a_quota_is_reported_reached_exactly_where_a_create_of_that_type_is_refused() {
    let router = service();

    // Every count up to the cloud-only quota: that counter's flag must come on at its
    // own quota and nowhere below it — the bound `DirectoryLimitExceededException`
    // reports — while the Managed Microsoft AD counter stays below its own, larger one.
    for created in 1..=CLOUD_ONLY_QUOTA {
        let case = format!("{created} stored Simple AD directories");
        create_directory(
            &router,
            minimal_create(&format!("bound{created}.example.com")),
            &case,
        )
        .await;
        let limits = limits(&router, &case).await;
        let count = i64::try_from(created).expect("the row fits an i64");
        assert_limits(&limits, [count, 0, 0], &case);
    }

    // One past the cloud-only bound: the Simple AD create is refused, the count stays at
    // the quota and the flag stays on...
    let case = "one past the cloud-only quota";
    refused(
        &router,
        "CreateDirectory",
        minimal_create("one-too-many.example.com"),
        "DirectoryLimitExceededException",
        case,
    )
    .await;
    let allowance = i64::try_from(CLOUD_ONLY_QUOTA).expect("the quota fits an i64");
    let at_the_bound = limits(&router, case).await;
    assert_limits(&at_the_bound, [allowance, 0, 0], case);

    // ...and a Managed Microsoft AD create is NOT refused by it: a used-up allowance is
    // the one its own category reports.
    let case = "a Managed Microsoft AD create past the cloud-only quota";
    create_microsoft_ad(
        &router,
        minimal_microsoft_ad("still-allowed.example.com"),
        case,
    )
    .await;
    let after_managed = limits(&router, case).await;
    assert_limits(&after_managed, [allowance, 1, 0], case);
}

#[tokio::test]
async fn deleting_directories_frees_the_reported_allowance() {
    let router = service();

    // Fill the cloud-only allowance with a mix of both types, then delete the
    // directories one at a time: each count must fall with the store, in its own
    // category.
    let managed = CLOUD_ONLY_QUOTA / 2;
    let mut simple_ids = Vec::new();
    let mut managed_ids = Vec::new();
    for index in 0..CLOUD_ONLY_QUOTA {
        if index < managed {
            managed_ids.push(
                create_microsoft_ad(
                    &router,
                    minimal_microsoft_ad(&format!("drop-managed{index}.example.com")),
                    "filling the allowance",
                )
                .await,
            );
        } else {
            simple_ids.push(
                create_directory(
                    &router,
                    minimal_create(&format!("drop-simple{index}.example.com")),
                    "filling the allowance",
                )
                .await,
            );
        }
    }

    let mut simple_left = simple_ids.len();
    let mut managed_left = managed_ids.len();
    let mut delete_simple_next = true;
    while !simple_ids.is_empty() || !managed_ids.is_empty() {
        // Alternate the type deleted — taking the other list only when the chosen one is
        // empty — so each counter is observed falling while the other still holds rows.
        let (id, simple) = match (delete_simple_next, simple_ids.is_empty()) {
            (true, false) => (simple_ids.pop().expect("the list is non-empty"), true),
            (false, _) if !managed_ids.is_empty() => {
                (managed_ids.pop().expect("the list is non-empty"), false)
            }
            _ => match simple_ids.pop() {
                Some(id) => (id, true),
                None => (managed_ids.pop().expect("one list is non-empty"), false),
            },
        };
        delete_simple_next = !simple;
        if simple {
            simple_left -= 1;
        } else {
            managed_left -= 1;
        }
        let case = format!("after deleting {id}");
        // A delete answers while the directory is still being torn down, and a directory
        // the account still holds still counts against its allowance, so each delete is
        // waited out before the counters are read.
        crate::harness::delete_directory_and_wait(&router, &id, &case).await;

        let limits = limits(&router, &case).await;
        let counts = [
            i64::try_from(simple_left).expect("the count fits an i64"),
            i64::try_from(managed_left).expect("the count fits an i64"),
            0,
        ];
        assert_limits(&limits, counts, &case);
    }

    // And the freed allowance is usable again.
    create_directory(
        &router,
        minimal_create("after-the-deletes.example.com"),
        "after the deletes",
    )
    .await;
    let limits = limits(&router, "after the deletes").await;
    assert_limits(&limits, [1, 0, 0], "after the deletes");
}

#[tokio::test]
async fn reading_the_limits_changes_nothing_a_later_caller_sees() {
    let router = service();
    create_directory(
        &router,
        minimal_create("stable.example.com"),
        "one directory",
    )
    .await;

    // A reporting operation: repeated reads answer the same thing, and none of them
    // consumes allowance a later create needs.
    let first = limits(&router, "first read").await;
    for repeat in 1..=4 {
        let case = format!("read {repeat}");
        assert_eq!(
            limits(&router, &case).await,
            first,
            "case {case:?}: expected every read to report the same limits as {first}"
        );
    }
    create_directory(
        &router,
        minimal_create("after-reads.example.com"),
        "after four reads",
    )
    .await;
    let limits = limits(&router, "after four reads").await;
    assert_limits(&limits, [2, 0, 0], "after four reads");
}
