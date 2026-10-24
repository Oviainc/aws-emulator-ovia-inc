// SPDX-License-Identifier: CC0-1.0

//! `ListServiceLevelObjectiveExclusionWindows` (GET `/slo/{Id}/exclusion-windows`) as a
//! REPORTING read.
//!
//! The rule under test: the operation PROJECTS the stored exclusion-window set of the SLO its
//! `Id` names — every window of that set exactly once, in the order it was added, bounded by
//! the page the caller asked for — so reading it, at any page bound and from any page
//! boundary, leaves that set and every other SLO's set exactly as it was, and either id form
//! the modelled `ServiceLevelObjectiveId` admits resolves to the same SLO and so answers the
//! same page.
//!
//! Every rule is asserted over a TABLE of stored set SIZES, which is what a projection rule
//! generalises over; the window CONFIGURATIONS it spans are the table `exclusion_windows`
//! drives, and the helpers here are that suite's, so both speak to one harness.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::exclusion_windows::{
    batch, create_slo, keys_of, keys_sent, list, paged, query, window, DURATION_UNITS,
    MAX_WINDOWS_PER_SLO,
};
use crate::slo_lifecycle::{arn_path_segment, call, router};

/// The stored set sizes every rule below is asserted over: an SLO holding none, one, a
/// middling number, and the modelled maximum of `ExclusionWindows` (`length ..10`, from
/// `smithy_query(shape="ExclusionWindows")`).
const SET_SIZES: [usize; 5] = [0, 1, 2, 5, MAX_WINDOWS_PER_SLO];

/// `count` window configurations for the SLO in `slot`, distinct within the SLO and across
/// slots — the stored set is identified by configuration, so no two rows may collide.
///
/// Every row states `StartTime`, so what these reads compare is the projection of the stored
/// set rather than how an omitted start time is derived.
fn windows_for(slot: usize, count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            window(
                DURATION_UNITS[index % DURATION_UNITS.len()],
                // `Window.Duration` targets `ExclusionDuration`, `range 1..`
                // (`smithy_query(shape="Window")`), so every duration here is at least 1.
                (slot * 100 + index + 1) as i64,
                Some(1_700_000_000 + (slot * 1_000 + index) as i64),
                None,
                Some(&format!("slot {slot} window {index}")),
            )
        })
        .collect()
}

/// Create the SLO called `name` holding exactly `windows`, failing the test if the setup does
/// not apply.
async fn store(router: &MultiServiceRouter, name: &str, windows: &[Value]) {
    create_slo(router, name).await;
    if windows.is_empty() {
        return;
    }
    let (status, updated) = batch(router, &[name.to_string()], windows, &[]).await;
    assert_eq!(
        status,
        200,
        "case {name:?}: expected the setup add of {} windows to succeed, got {status} with {updated}",
        windows.len()
    );
}

/// The read reports the stored set and NOTHING it does changes what a later caller sees:
/// repeating the read, and paging the whole set one window at a time, both answer the same
/// set in the same order.
#[tokio::test]
async fn a_read_reports_the_stored_set_and_repeating_it_changes_nothing() {
    let router = router();
    for size in SET_SIZES {
        let name = format!("read-stable-{size}");
        let sent = windows_for(size, size);
        store(&router, &name, &sent).await;

        let (status, first) = list(&router, &name, "").await;
        assert_eq!(
            status, 200,
            "case size {size}: expected the read to succeed, got {status} with {first}"
        );
        assert_eq!(
            keys_of(&first),
            keys_sent(&sent),
            "case size {size}: expected the stored set in the order added, got {first}"
        );
        assert!(
            first["NextToken"].is_null(),
            "case size {size}: expected no NextToken on a page holding the whole set, got {first}"
        );

        // A read reports the set, it does not consume it: the same call answers the same page.
        let (status, again) = list(&router, &name, "").await;
        assert_eq!(
            status, 200,
            "case size {size}: expected the repeated read to succeed, got {status} with {again}"
        );
        assert_eq!(
            keys_of(&again),
            keys_sent(&sent),
            "case size {size}: expected the repeated read to answer the same page, got {again}"
        );

        // Walking the set at the smallest bound the model admits reports every window exactly
        // once, in the stored order, one window per page.
        let (collected, pages) = paged(&router, &name, Some(1)).await;
        assert_eq!(
            collected,
            keys_sent(&sent),
            "case size {size}: expected paging at a bound of 1 to report every window once"
        );
        assert_eq!(
            pages,
            size.max(1),
            "case size {size}: expected one page per window (at least one), got {pages}"
        );

        // And the walk left the set as it was.
        let (status, after) = list(&router, &name, "").await;
        assert_eq!(
            status, 200,
            "case size {size}: expected the read after paging to succeed, got {status} with {after}"
        );
        assert_eq!(
            keys_of(&after),
            keys_sent(&sent),
            "case size {size}: expected paging to leave the stored set untouched, got {after}"
        );
    }
}

/// Either id form the modelled `ServiceLevelObjectiveId` admits resolves to the same SLO, so
/// a read by bare name and a read by ARN answer the same page — at every page bound, and for
/// the page token each hands back.
#[tokio::test]
async fn either_id_form_answers_the_same_page() {
    let router = router();
    for size in SET_SIZES {
        let name = format!("read-id-form-{size}");
        let sent = windows_for(size, size);
        store(&router, &name, &sent).await;

        // The omitted bound, both published bounds of
        // `ListServiceLevelObjectiveExclusionWindowsMaxResults` (`range 1..10`) and one size
        // between them.
        for max_results in [None, Some(1), Some(3), Some(MAX_WINDOWS_PER_SLO)] {
            let label = format!("size {size}, MaxResults {max_results:?}");
            let (by_name_status, by_name) = list(&router, &name, &query(max_results, None)).await;
            let (by_arn_status, by_arn) = call(
                &router,
                "GET",
                &format!(
                    "/slo/{}/exclusion-windows{}",
                    arn_path_segment(&name),
                    query(max_results, None)
                ),
                None,
            )
            .await;
            assert_eq!(
                by_arn_status, by_name_status,
                "case {label}: expected both id forms to answer the same status, got {by_name_status} by name and {by_arn_status} by ARN"
            );
            assert_eq!(
                keys_of(&by_arn),
                keys_of(&by_name),
                "case {label}: expected both id forms to report the same windows, got {by_arn} by ARN and {by_name} by name"
            );
            assert_eq!(
                by_arn["NextToken"], by_name["NextToken"],
                "case {label}: expected both id forms to answer the same NextToken, got {by_arn} by ARN and {by_name} by name"
            );

            // A token one form handed back resumes the same page on the other form.
            if let Some(token) = by_name["NextToken"].as_str() {
                let (status, resumed_by_arn) = call(
                    &router,
                    "GET",
                    &format!(
                        "/slo/{}/exclusion-windows{}",
                        arn_path_segment(&name),
                        query(max_results, Some(token))
                    ),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {label}: expected the resumed read by ARN to succeed, got {status} with {resumed_by_arn}"
                );
                let (by_name_status, resumed_by_name) =
                    list(&router, &name, &query(max_results, Some(token))).await;
                assert_eq!(
                    by_name_status, status,
                    "case {label}: expected the resumed read to answer the same status for either id form, got {status} by ARN and {by_name_status} by name"
                );
                assert_eq!(
                    keys_of(&resumed_by_arn),
                    keys_of(&resumed_by_name),
                    "case {label}: expected the token to resume the same page for either id form, got {resumed_by_arn} by ARN and {resumed_by_name} by name"
                );
            }
        }
    }
}

/// A read reports the set of the SLO its `Id` names and no other's: SLOs holding different
/// sets keep them, whatever order the reads come in, and a change to one SLO's set is not
/// reported by any other.
#[tokio::test]
async fn a_read_reports_only_the_set_of_the_slo_it_names() {
    let router = router();
    // One SLO per set size, each holding configurations no other SLO holds.
    let table: Vec<(String, Vec<Value>)> = SET_SIZES
        .iter()
        .enumerate()
        .map(|(slot, &size)| (format!("read-isolated-{size}"), windows_for(slot + 1, size)))
        .collect();
    for (name, windows) in &table {
        store(&router, name, windows).await;
    }

    // Two interleaved rounds, so no read is the first read of its SLO in the second.
    for round in 0..2 {
        for (name, windows) in &table {
            let (status, page) = list(&router, name, "").await;
            assert_eq!(
                status, 200,
                "case {name:?} round {round}: expected the read to succeed, got {status} with {page}"
            );
            assert_eq!(
                keys_of(&page),
                keys_sent(windows),
                "case {name:?} round {round}: expected only this SLO's own windows, got {page}"
            );
        }
    }

    // Emptying the largest set is reported by that SLO alone.
    let (emptied, removed) = table
        .last()
        .expect("the table holds one SLO per set size")
        .clone();
    let (status, updated) = batch(&router, std::slice::from_ref(&emptied), &[], &removed).await;
    assert_eq!(
        status, 200,
        "case {emptied:?}: expected the removal of every window to succeed, got {status} with {updated}"
    );
    for (name, windows) in &table {
        let expected = if *name == emptied {
            Vec::new()
        } else {
            keys_sent(windows)
        };
        let (status, page) = list(&router, name, "").await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the read after the removal to succeed, got {status} with {page}"
        );
        assert_eq!(
            keys_of(&page),
            expected,
            "case {name:?}: expected the removal to be reported by {emptied:?} alone, got {page}"
        );
    }
}
