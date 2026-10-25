// SPDX-License-Identifier: CC0-1.0

//! What a child-resource listing of a workflow does with a pagination cursor that was
//! issued on a DIFFERENT workflow's history.
//!
//! RULE under test: a `NextToken` is a position in the one walk it was issued for — the
//! list, and the parent resource whose history that walk is over — so a child listing
//! accepts a cursor back only on the workflow it was issued on and refuses every other
//! with the modelled `ValidationException` (HTTP 400, no page), instead of applying a
//! position taken from a history it says nothing about, which silently drops or repeats
//! the children the walk should have reported.
//!
//! The table spans what the rule generalises over: which child listing is driven
//! (`ListWorkflowRuns`, `ListWorkflowVersions`), which direction the cursor travels
//! (from an older workflow's history into a newer one's and back), how many children the
//! workflow the cursor is presented to holds next to the one that issued it (more, as
//! many, fewer, none), whether that workflow is one the endpoint holds at all, the page
//! size at each published `MaxResults` bound, and — for the run listing — whether a
//! version filter is in play. The control rows present each cursor back on the workflow
//! that issued it, so the refusal is shown to be about the cursor's scope rather than
//! about carrying a cursor at all.

use crate::missing_resource_lookups::ghost_arns;
use crate::workflow_lifecycle::Harness;
use crate::workflow_run_listing::{appended_version, created_workflow, started_run};

/// The region the workflows under test are created at.
const HOME: &str = "us-west-2";

/// How many children the equally sized workflows hold: enough that a page size of 1
/// leaves a cursor with more than one child still to come.
const CHILDREN: usize = 3;

/// The published bounds of `MaxResults` (`@range(min: 1, max: 100)`,
/// `smithy_query(query="operation", operation="ListWorkflowRuns")` and the same on
/// `ListWorkflowVersions`), plus a size between them.
const PAGE_SIZES: &[(&str, i64)] = &[
    ("MaxResults at the minimum of 1", 1),
    ("MaxResults of 2", 2),
    ("MaxResults at the maximum of 100", 100),
];

/// A `WorkflowVersion` of the modelled form (`^.+$` for the `VersionId` selector,
/// `smithy_query(query="operation", operation="ListWorkflowRuns")`) that no run carries.
fn ghost_version() -> String {
    "f".repeat(32)
}

/// The two implemented child-resource listings of a workflow, each paging the history of
/// the one workflow its `WorkflowArn` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChildListing {
    Runs,
    Versions,
}

impl ChildListing {
    fn operation(self) -> &'static str {
        match self {
            Self::Runs => "ListWorkflowRuns",
            Self::Versions => "ListWorkflowVersions",
        }
    }

    /// The `@paginated` `items` member this listing reports a page in.
    fn items(self) -> &'static str {
        match self {
            Self::Runs => "WorkflowRuns",
            Self::Versions => "WorkflowVersions",
        }
    }

    /// The member of one summary that identifies the child it reports.
    fn id_member(self) -> &'static str {
        match self {
            Self::Runs => "RunId",
            Self::Versions => "WorkflowVersion",
        }
    }

    /// Only the run listing models a `WorkflowVersion` filter, so only its rows vary it.
    fn version_filters(self) -> Vec<(&'static str, Option<String>)> {
        match self {
            Self::Runs => vec![
                ("no version filter", None),
                ("narrowed to a version no run carries", Some(ghost_version())),
            ],
            Self::Versions => vec![("no version filter", None)],
        }
    }

    /// Create a workflow holding `children` children of this listing's kind, and report
    /// its ARN. A create already mints the workflow's first version, so a version row
    /// asks for at least one.
    async fn seeded(self, harness: &Harness, name: &str, children: usize) -> String {
        let arn = created_workflow(harness, name).await;
        match self {
            Self::Runs => {
                for _ in 0..children {
                    started_run(harness, &arn, None).await;
                }
            }
            Self::Versions => {
                assert!(
                    children >= 1,
                    "case {name:?}: a created workflow always holds the version its create \
                     minted, so {children} versions cannot be seeded"
                );
                for index in 1..children {
                    appended_version(harness, &arn, &format!("dags/{name}-{index}.yaml")).await;
                }
            }
        }
        arn
    }
}

/// Call one child listing and report its status and body.
async fn list(
    harness: &Harness,
    listing: ChildListing,
    arn: &str,
    size: i64,
    token: Option<&str>,
    version: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = serde_json::json!({"WorkflowArn": arn, "MaxResults": size});
    if let Some(token) = token {
        body["NextToken"] = serde_json::Value::String(token.to_string());
    }
    if let Some(version) = version {
        body["WorkflowVersion"] = serde_json::Value::String(version.to_string());
    }
    harness.call_in(HOME, listing.operation(), body).await
}

/// The identifiers one page reports, in the order it reported them.
fn ids(listing: ChildListing, page: &serde_json::Value) -> Vec<String> {
    page[listing.items()]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item[listing.id_member()].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One workflow in the table: its ARN, how many children of the listing's kind it holds,
/// and what this row varies.
struct Candidate {
    label: &'static str,
    arn: String,
    children: usize,
}

/// Every workflow a cursor may be issued on or presented to, seeded in creation order so
/// one workflow's children are older than another's — which is what makes a foreign
/// cursor's position fall before, inside or after the history it is presented to.
async fn candidates(harness: &Harness, listing: ChildListing) -> Vec<Candidate> {
    let mut rows = vec![
        Candidate {
            label: "the oldest workflow, as many children",
            arn: listing.seeded(harness, "cursorScopeOldest", CHILDREN).await,
            children: CHILDREN,
        },
        Candidate {
            label: "a newer workflow with more children",
            arn: listing
                .seeded(harness, "cursorScopeWider", CHILDREN + 2)
                .await,
            children: CHILDREN + 2,
        },
        Candidate {
            label: "the newest workflow, as many children",
            arn: listing.seeded(harness, "cursorScopeNewest", CHILDREN).await,
            children: CHILDREN,
        },
        Candidate {
            label: "a workflow with a single child",
            arn: listing.seeded(harness, "cursorScopeSingle", 1).await,
            children: 1,
        },
    ];
    // A workflow holding NO child of this kind: only the run listing can have one, since
    // a create mints a workflow's first version.
    if listing == ChildListing::Runs {
        rows.push(Candidate {
            label: "a workflow with no child at all",
            arn: listing.seeded(harness, "cursorScopeEmpty", 0).await,
            children: 0,
        });
    }
    rows
}

/// How many children the store holds for a workflow, read directly rather than through
/// the listing under test.
fn stored_children(harness: &Harness, listing: ChildListing, arn: &str) -> usize {
    match listing {
        ChildListing::Runs => harness.stored_run_count(arn),
        ChildListing::Versions => harness.stored_versions(arn).len(),
    }
}

#[tokio::test]
async fn a_cursor_issued_on_another_workflows_history_is_refused() {
    for listing in [ChildListing::Runs, ChildListing::Versions] {
        let harness = Harness::new(HOME);
        let rows = candidates(&harness, listing).await;

        // Every workflow holding more than one child issues a cursor at a page size of 1.
        let mut cursors: Vec<(&'static str, String, String)> = Vec::new();
        for row in &rows {
            if row.children < 2 {
                continue;
            }
            let (status, first) = list(&harness, listing, &row.arn, 1, None, None).await;
            assert_eq!(
                status, 200,
                "setup {:?}/{}: expected the first page to succeed, got {status} {first:?}",
                row.label,
                listing.operation()
            );
            let token = first["NextToken"].as_str().unwrap_or_else(|| {
                panic!(
                    "setup {:?}/{}: {} children at MaxResults 1 must leave a cursor, got \
                     {first:?}",
                    row.label,
                    listing.operation(),
                    row.children
                )
            });
            cursors.push((row.label, row.arn.clone(), token.to_string()));
        }
        assert!(
            cursors.len() >= 3,
            "setup {}: expected several workflows to issue a cursor, got {}",
            listing.operation(),
            cursors.len()
        );

        // Every workflow the cursor may be presented to: the seeded ones, plus ARNs no
        // store ever held — the refusal is about the cursor, so it does not depend on
        // what the named workflow resolves to.
        let mut targets: Vec<(String, String)> = rows
            .iter()
            .map(|row| (row.label.to_string(), row.arn.clone()))
            .collect();
        for arn in ghost_arns() {
            targets.push((format!("a workflow this endpoint does not hold/{arn}"), arn));
        }

        for (issuer_label, issuer_arn, token) in &cursors {
            for (target_label, target_arn) in &targets {
                if target_arn == issuer_arn {
                    continue;
                }
                for (size_label, size) in PAGE_SIZES {
                    for (filter_label, filter) in listing.version_filters() {
                        let case = format!(
                            "{}/cursor of {issuer_label} on {target_label}/{size_label}/\
                             {filter_label}",
                            listing.operation()
                        );
                        let (status, answered) = list(
                            &harness,
                            listing,
                            target_arn,
                            *size,
                            Some(token),
                            filter.as_deref(),
                        )
                        .await;
                        assert_eq!(
                            status, 400,
                            "case {case:?}: expected HTTP 400 for a cursor issued on another \
                             workflow's history, got {status} {answered:?}"
                        );
                        assert!(
                            answered["__type"]
                                .as_str()
                                .is_some_and(|kind| kind.contains("ValidationException")),
                            "case {case:?}: expected a ValidationException, got {:?}",
                            answered["__type"]
                        );
                        assert!(
                            answered[listing.items()].is_null(),
                            "case {case:?}: a refusal reports no page, got {:?}",
                            answered[listing.items()]
                        );
                    }
                }
            }
        }

        // The refusals changed nothing a later caller sees: every workflow still holds
        // exactly the children it was seeded with.
        for row in &rows {
            assert_eq!(
                stored_children(&harness, listing, &row.arn),
                row.children,
                "case {:?}/{}: a refused cursor must not change what the store holds, got {}",
                row.label,
                listing.operation(),
                stored_children(&harness, listing, &row.arn)
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_presented_on_the_workflow_that_issued_it_continues_that_walk() {
    for listing in [ChildListing::Runs, ChildListing::Versions] {
        let harness = Harness::new(HOME);
        let rows = candidates(&harness, listing).await;

        for row in &rows {
            // The whole history, as one page at the maximum page size: the set the walk
            // must cover exactly.
            let (status, whole) = list(&harness, listing, &row.arn, 100, None, None).await;
            assert_eq!(
                status, 200,
                "case {:?}/{}: expected the whole history to list, got {status} {whole:?}",
                row.label,
                listing.operation()
            );
            let expected = ids(listing, &whole);
            assert_eq!(
                expected.len(),
                row.children,
                "case {:?}/{}: expected {} children, got {expected:?}",
                row.label,
                listing.operation(),
                row.children
            );

            // The same history walked one child at a time through the cursors this
            // workflow itself issues: accepted, and continuing rather than restarting.
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            for step in 0..=row.children {
                let (status, page) =
                    list(&harness, listing, &row.arn, 1, token.as_deref(), None).await;
                assert_eq!(
                    status, 200,
                    "case {:?}/{}/step {step}: expected its own cursor to be accepted, got \
                     {status} {page:?}",
                    row.label,
                    listing.operation()
                );
                let page_ids = ids(listing, &page);
                assert!(
                    page_ids.iter().all(|id| !seen.contains(id)),
                    "case {:?}/{}/step {step}: the walk must not repeat a child, got \
                     {page_ids:?} after {seen:?}",
                    row.label,
                    listing.operation()
                );
                seen.extend(page_ids);
                token = page["NextToken"].as_str().map(str::to_string);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, expected,
                "case {:?}/{}: paging one child at a time must cover the whole history in \
                 the same order, got {seen:?}",
                row.label,
                listing.operation()
            );
        }
    }
}
