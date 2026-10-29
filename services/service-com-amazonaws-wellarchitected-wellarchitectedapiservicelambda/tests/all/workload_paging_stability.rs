// SPDX-License-Identifier: CC0-1.0

//! `ListWorkloads` paging while a concurrent caller writes a workload member.
//!
//! The enumeration order and the `NextToken` cursor are keyed on a value that is fixed for
//! a workload's whole life, so a write between two pages moves nothing relative to the
//! resume point: every workload that existed for the whole walk is served exactly once, in
//! one stable order. The rows span which workload is written (already served, just served,
//! not yet served), where the new name would sort relative to the cursor (before every
//! stored name, between two of them, after every one), how many pages the walk takes, and
//! a baseline walk with no write at all.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Client};

/// One concurrent write a walk performs: after the page at `after_page` (0-based), rename
/// the workload created at `target` (0-based, in creation order) to `new_name`.
#[derive(Debug, Clone, Copy)]
struct Rename {
    after_page: usize,
    target: usize,
    new_name: &'static str,
}

/// The `WorkloadName`s the walks below start from, in ascending order, each within the
/// published `@length(3, 100)` bound of `WorkloadName`.
const NAMES: [&str; 4] = ["aaa", "bbb", "ccc", "ddd"];

/// One row of the prefix-filter table: the walk's `WorkloadNamePrefix`, the rename it
/// performs, the workloads (by creation index) whose name carries the prefix both before
/// and after that rename, and the superset the walk may serve — the first plus the one the
/// rename moves, which the walk serves or not depending on whether it had already passed
/// it.
#[derive(Debug, Clone, Copy)]
struct PrefixCase {
    case: &'static str,
    prefix: &'static str,
    rename: Rename,
    always: &'static [usize],
    allowed: &'static [usize],
}

/// Walk every page of `ListWorkloads` at `MaxResults` = `page_size`, applying each rename
/// in `renames` after the page it names, and answer the `WorkloadId`s the walk served in
/// the order the pages served them.
async fn walk(
    client: &Client,
    case: &str,
    page_size: usize,
    ids: &[String],
    renames: &[Rename],
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut page_index = 0usize;
    loop {
        assert!(
            page_index <= ids.len() + 1,
            "case {case:?}: paging did not terminate after {page_index} pages, got {seen:?}"
        );
        let mut request = json!({ "MaxResults": page_size });
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let answer = client.list(request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListWorkloads 200, got {answer:?}"
        );
        let summaries = answer
            .body
            .get("WorkloadSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            summaries.len() <= page_size,
            "case {case:?}: a page must not exceed MaxResults {page_size}, got {} entries",
            summaries.len()
        );
        for summary in &summaries {
            seen.push(
                summary
                    .get("WorkloadId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        for rename in renames.iter().filter(|rename| rename.after_page == page_index) {
            let answer = client
                .update(
                    &ids[rename.target],
                    json!({ "WorkloadName": rename.new_name }),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected UpdateWorkload 200 for {rename:?}, got {answer:?}"
            );
        }

        page_index += 1;
        if next_token.is_none() {
            break;
        }
    }
    seen
}

/// A workload written between two pages is still served exactly once, and the pages keep
/// one order the write does not disturb.
#[tokio::test]
async fn paging_serves_every_workload_once_across_a_concurrent_rename() {
    let cases: Vec<(&str, usize, Vec<Rename>)> = vec![
        ("no-rename", 1, Vec::new()),
        (
            "served-workload-renamed-after-every-name",
            1,
            vec![Rename {
                after_page: 0,
                target: 0,
                new_name: "zzz",
            }],
        ),
        (
            "served-workload-renamed-between-two-names",
            1,
            vec![Rename {
                after_page: 0,
                target: 0,
                new_name: "bb5",
            }],
        ),
        (
            "unserved-workload-renamed-before-every-name",
            1,
            vec![Rename {
                after_page: 0,
                target: 3,
                new_name: "aa0",
            }],
        ),
        (
            "unserved-workload-renamed-after-every-name",
            1,
            vec![Rename {
                after_page: 1,
                target: 2,
                new_name: "zz9",
            }],
        ),
        (
            "a-rename-after-every-page",
            1,
            vec![
                Rename {
                    after_page: 0,
                    target: 0,
                    new_name: "zzz",
                },
                Rename {
                    after_page: 1,
                    target: 3,
                    new_name: "aa0",
                },
                Rename {
                    after_page: 2,
                    target: 1,
                    new_name: "zzy",
                },
            ],
        ),
        (
            "two-per-page-unserved-renamed-first",
            2,
            vec![Rename {
                after_page: 0,
                target: 3,
                new_name: "aa0",
            }],
        ),
        (
            "two-per-page-served-renamed-last",
            2,
            vec![Rename {
                after_page: 0,
                target: 0,
                new_name: "zzz",
            }],
        ),
        (
            // `MaxResults` at its published maximum: one page, so the rename lands after
            // the walk is already over and changes nothing.
            "single-page-walk",
            50,
            vec![Rename {
                after_page: 0,
                target: 0,
                new_name: "zzz",
            }],
        ),
    ];

    for (case, page_size, renames) in cases {
        // One emulator per row, so a row observes only the workloads it created.
        let client = Client::new();
        let mut ids: Vec<String> = Vec::new();
        for (index, name) in NAMES.iter().enumerate() {
            let created = client
                .create(minimal_create(name, &format!("tok-{case}-{index}")))
                .await;
            ids.push(created_id(&created, case));
        }

        let seen = walk(&client, case, page_size, &ids, &renames).await;

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: paging returned a workload twice: {seen:?}"
        );
        let expected: BTreeSet<String> = ids.iter().cloned().collect();
        assert_eq!(
            unique, expected,
            "case {case:?}: paging must serve every workload that existed for the whole \
             walk exactly once, got {seen:?}"
        );
        // The order the pages serve is keyed on what cannot move, so it is the same order
        // whatever a concurrent caller writes.
        let mut stable: Vec<String> = ids.clone();
        stable.sort();
        assert_eq!(
            seen, stable,
            "case {case:?}: paging must keep one order a concurrent write does not move, \
             expected {stable:?}, got {seen:?}"
        );
    }
}

/// The `WorkloadNamePrefix` filter reads the CURRENT name of each workload the page
/// reaches, and still composes with paging across a rename: a workload whose name carries
/// the prefix for the WHOLE walk is served exactly once, and no workload is ever served
/// twice, however a rename moves another workload into or out of the prefix mid-walk.
///
#[tokio::test]
async fn the_name_prefix_filter_composes_with_paging_across_a_rename() {
    let cases = [
        PrefixCase {
            case: "renamed-into-the-prefix",
            prefix: "kept",
            rename: Rename {
                after_page: 0,
                target: 3,
                new_name: "kept-late",
            },
            always: &[0, 1, 2],
            allowed: &[0, 1, 2, 3],
        },
        PrefixCase {
            case: "renamed-out-of-the-prefix",
            prefix: "kept",
            rename: Rename {
                after_page: 0,
                target: 2,
                new_name: "other-late",
            },
            always: &[0, 1],
            allowed: &[0, 1, 2],
        },
        PrefixCase {
            case: "prefix-nothing-matches",
            prefix: "no-such-prefix",
            rename: Rename {
                after_page: 0,
                target: 0,
                new_name: "kept-early",
            },
            always: &[],
            allowed: &[],
        },
    ];

    for PrefixCase {
        case,
        prefix,
        rename,
        always,
        allowed,
    } in cases
    {
        let client = Client::new();
        let mut ids: Vec<String> = Vec::new();
        for (index, name) in ["kept-one", "kept-two", "kept-three", "other-one"]
            .iter()
            .enumerate()
        {
            let created = client
                .create(minimal_create(name, &format!("tok-{case}-{index}")))
                .await;
            ids.push(created_id(&created, case));
        }

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut page_index = 0usize;
        loop {
            assert!(
                page_index <= ids.len() + 1,
                "case {case:?}: paging did not terminate after {page_index} pages"
            );
            let mut request = json!({ "MaxResults": 1, "WorkloadNamePrefix": prefix });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let answer = client.list(request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected ListWorkloads 200, got {answer:?}"
            );
            for summary in answer
                .body
                .get("WorkloadSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let name = summary
                    .get("WorkloadName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    name.starts_with(prefix),
                    "case {case:?}: {name:?} does not start with {prefix:?}"
                );
                seen.push(
                    summary
                        .get("WorkloadId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            next_token = answer
                .body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if rename.after_page == page_index {
                let answer = client
                    .update(
                        &ids[rename.target],
                        json!({ "WorkloadName": rename.new_name }),
                    )
                    .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected UpdateWorkload 200 for {rename:?}, got {answer:?}"
                );
            }
            page_index += 1;
            if next_token.is_none() {
                break;
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: paging returned a workload twice: {seen:?}"
        );
        let required: BTreeSet<String> = always.iter().map(|index| ids[*index].clone()).collect();
        let permitted: BTreeSet<String> = allowed.iter().map(|index| ids[*index].clone()).collect();
        assert!(
            required.is_subset(&unique),
            "case {case:?}: every workload carrying the prefix for the whole walk must be \
             served, expected at least {required:?}, got {unique:?}"
        );
        assert!(
            unique.is_subset(&permitted),
            "case {case:?}: the walk served a workload the prefix never selected, \
             permitted {permitted:?}, got {unique:?}"
        );
    }
}
