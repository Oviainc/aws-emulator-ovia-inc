// SPDX-License-Identifier: CC0-1.0

//! `ListSyncConfigurations` over the wire: the paginated report of the sync
//! configurations of ONE repository link.
//!
//! The rule these suites assert: a paginated report of this service reports exactly the
//! stored rows its input selects — here the sync configurations whose `SyncType` and
//! `RepositoryLinkId` the request names — in the store's own total `ResourceName` order,
//! at most `MaxResults` of them per response (clamped to the member's published
//! maximum, and that maximum when the caller asks for nothing), with the remainder
//! reachable through `NextToken` and the token absent exactly when nothing is left; and
//! the resource its input names is RESOLVED first, so a well-formed `RepositoryLinkId`
//! the store does not hold, or a `NextToken` this service never issued, is refused with
//! the operation's modeled `InvalidInputException` (400) rather than answered with an
//! empty page.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing
//! `connection_lifecycle`'s and `git_sync`'s helpers, so these tests exercise the same
//! path a signed SDK request takes. The recorded case the issue cited — a list for a
//! repository link that does not exist — is ONE row of the refusal table below.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{call, router};
use super::git_sync::{
    ABSENT_LINK_IDS, LinkCase, SYNC_TYPE, SyncCase, create_link, create_sync_body, error_code,
};

/// The published bounds of the modeled `MaxResults` member (`range 0..100`, from
/// `smithy_query(query="operation", operation="ListSyncConfigurations")`): zero is its
/// modeled default, and 100 the largest page the service renders.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// The published maximum length of the modeled `ResourceName` (`length 1..100`, from
/// `smithy_query(query="shape", shape="ResourceName")`).
const RESOURCE_NAME_MAX_LEN: usize = 100;

/// One row of the listing table: a repository link, and the `ResourceName`s of the sync
/// configurations created against it.
struct ListCase {
    /// What a failing assertion names this row by.
    label: &'static str,
    /// The provider type of the connection the link is created on.
    provider_type: &'static str,
    /// The configurations this link holds, in the order they are created — deliberately
    /// NOT in `ResourceName` order, so a report that merely echoes creation order fails.
    resource_names: Vec<String>,
}

/// The table: links differing in how many configurations they hold (none, one, several)
/// and in `ResourceName` form and length — each form the modeled pattern
/// `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$` admits (digits, underscores, hyphens, mixed case) and
/// each published bound of its `length 1..100`.
fn list_cases() -> Vec<ListCase> {
    vec![
        ListCase {
            label: "a link with no configuration at all",
            provider_type: "GitHub",
            resource_names: Vec::new(),
        },
        ListCase {
            label: "a link with one configuration",
            provider_type: "Bitbucket",
            resource_names: vec!["SoleStack".to_string()],
        },
        ListCase {
            label: "names that sort against their creation order",
            provider_type: "GitLab",
            resource_names: vec![
                "zeta".to_string(),
                "Alpha".to_string(),
                "middle_stack".to_string(),
                "Stack-9".to_string(),
            ],
        },
        ListCase {
            label: "names at each published length bound",
            provider_type: "AzureDevOps",
            resource_names: vec![
                "A".to_string(),
                format!("L{}", "o".repeat(RESOURCE_NAME_MAX_LEN - 1)),
            ],
        },
        ListCase {
            label: "a provider type and name forms the issue did not cite",
            provider_type: "GitHubEnterpriseServer",
            resource_names: vec![
                "0leading-digit".to_string(),
                "Mixed_Case-123".to_string(),
                "9".to_string(),
            ],
        },
    ]
}

/// One `CreateSyncConfiguration` row for `resource_name`, with every other member
/// varying by `position` so two configurations of one link are never identical.
fn sync_case(label: &'static str, resource_name: &str, position: usize) -> SyncCase {
    SyncCase {
        label,
        resource_name: resource_name.to_string(),
        branch: format!("branch-{position}"),
        config_file: format!("stacks/{position}.yaml"),
        publish_deployment_status: None,
        trigger_resource_update_on: None,
        pull_request_comment: None,
        update: serde_json::Value::Null,
    }
}

/// Create one row's repository link and every configuration it names, and return the
/// `RepositoryLinkId`.
async fn seed(router: &MultiServiceRouter, case: &ListCase, index: usize) -> String {
    let link = LinkCase {
        label: case.label,
        provider_type: case.provider_type,
        owner_id: format!("owner-{index}"),
        repository_name: format!("repo-{index}"),
        encryption_key_arn: None,
        tags: Vec::new(),
    };
    let (link_id, _) = create_link(router, &link, &format!("list-conn-{index}")).await;

    for (position, resource_name) in case.resource_names.iter().enumerate() {
        let (status, created) = call(
            router,
            "CreateSyncConfiguration",
            create_sync_body(&sync_case(case.label, resource_name, position), &link_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateSyncConfiguration for {resource_name:?}, got {status} {created}",
            case.label
        );
    }
    link_id
}

/// Call `ListSyncConfigurations` for one link, with the page size and token a case names.
async fn list(
    router: &MultiServiceRouter,
    link_id: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = json!({ "RepositoryLinkId": link_id, "SyncType": SYNC_TYPE });
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["NextToken"] = json!(next_token);
    }
    call(router, "ListSyncConfigurations", body).await
}

/// The `ResourceName`s a response reports, in the order it reported them. The modeled
/// `SyncConfigurations` member is required, so it must be an array even when empty.
fn reported_names(case: &str, body: &serde_json::Value) -> Vec<String> {
    let configurations = body["SyncConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: SyncConfigurations must be a list, got {body}"));
    configurations
        .iter()
        .map(|configuration| {
            configuration["ResourceName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every entry carries a ResourceName, got {body}")
                })
                .to_string()
        })
        .collect()
}

/// The report holds exactly the configurations of the link and sync type it names, in
/// `ResourceName` order, each entry identical to the row `GetSyncConfiguration` reports;
/// it never leaks another link's configurations, it reports nothing for a link that holds
/// none, it changes nothing a later caller sees, and a deleted configuration leaves it.
#[tokio::test]
async fn listing_reports_exactly_the_named_links_configurations() {
    let router = router();
    let cases = list_cases();

    let mut link_ids: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        link_ids.push(seed(&router, case, index).await);
    }

    for (case, link_id) in cases.iter().zip(&link_ids) {
        let (status, body) = list(&router, link_id, None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListSyncConfigurations, got {status} {body}",
            case.label
        );

        let mut expected = case.resource_names.clone();
        expected.sort();
        assert_eq!(
            reported_names(case.label, &body),
            expected,
            "case {:?}: expected this link's configurations in ResourceName order, got {body}",
            case.label
        );
        assert!(
            body["NextToken"].is_null(),
            "case {:?}: nothing is left, so no NextToken may come back, got {body}",
            case.label
        );

        // Every reported entry is the stored row, not a recomputed one: it is exactly
        // what the single-resource reader reports, and it belongs to THIS link.
        for (position, resource_name) in expected.iter().enumerate() {
            let (status, got) = call(
                &router,
                "GetSyncConfiguration",
                json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected 200 from GetSyncConfiguration for {resource_name:?}, got {status} {got}",
                case.label
            );
            assert_eq!(
                body["SyncConfigurations"][position], got["SyncConfiguration"],
                "case {:?}: entry {position} must be the stored row, got {body}",
                case.label
            );
            assert_eq!(
                body["SyncConfigurations"][position]["RepositoryLinkId"],
                json!(link_id),
                "case {:?}: entry {position} must belong to the link that was listed, got {body}",
                case.label
            );
        }

        // A report changes nothing: the same call answers the same thing.
        let (status, again) = list(&router, link_id, None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from the repeated list, got {status} {again}",
            case.label
        );
        assert_eq!(
            again["SyncConfigurations"], body["SyncConfigurations"],
            "case {:?}: listing must not change what a later caller sees, got {again}",
            case.label
        );
    }

    // A configuration this service no longer holds leaves the report, and only it does.
    for (case, link_id) in cases.iter().zip(&link_ids) {
        let Some(removed) = case.resource_names.first() else {
            continue;
        };
        let (status, deleted) = call(
            &router,
            "DeleteSyncConfiguration",
            json!({ "SyncType": SYNC_TYPE, "ResourceName": removed }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from DeleteSyncConfiguration, got {status} {deleted}",
            case.label
        );

        let (_, body) = list(&router, link_id, None, None).await;
        let mut expected: Vec<String> = case
            .resource_names
            .iter()
            .filter(|name| *name != removed)
            .cloned()
            .collect();
        expected.sort();
        assert_eq!(
            reported_names(case.label, &body),
            expected,
            "case {:?}: the deleted configuration {removed:?} must leave the report, got {body}",
            case.label
        );
    }
}

/// Page sizes: each published bound of the modeled `MaxResults` (its `0` default and its
/// maximum of 100), the member omitted entirely, and sizes below, equal to and above the
/// number of configurations the link holds.
const PAGE_SIZES: &[Option<i64>] = &[
    None,
    Some(MAX_RESULTS_MIN),
    Some(1),
    Some(2),
    Some(3),
    Some(6),
    Some(7),
    Some(MAX_RESULTS_MAX),
];

/// Paging covers every configuration of the link exactly once, in `ResourceName` order,
/// for every page size: the invariant `@paginated(inputToken=NextToken,
/// outputToken=NextToken, pageSize=MaxResults)` states. No page exceeds the size the
/// caller asked for, nor the member's published maximum when it asked for nothing, and
/// the token comes back exactly while items are left.
#[tokio::test]
async fn paging_covers_every_configuration_exactly_once() {
    let router = router();
    let case = ListCase {
        label: "paged link",
        provider_type: "GitLabSelfManaged",
        resource_names: (0..6).map(|index| format!("Paged{index}")).collect(),
    };
    let link_id = seed(&router, &case, 90).await;
    // A second link with its own configurations, so a page that over-reads the store
    // cannot pass as a correct page of this one.
    let other = ListCase {
        label: "other link",
        provider_type: "GitHub",
        resource_names: (0..4).map(|index| format!("Other{index}")).collect(),
    };
    seed(&router, &other, 91).await;

    let mut expected = case.resource_names.clone();
    expected.sort();

    for page_size in PAGE_SIZES {
        let bound = match page_size {
            Some(asked) if *asked > 0 => (*asked).min(MAX_RESULTS_MAX),
            _ => MAX_RESULTS_MAX,
        };
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let (status, body) = list(&router, &link_id, *page_size, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {page_size:?}: expected 200 from ListSyncConfigurations, got {status} {body}"
            );
            let page = reported_names("paged link", &body);
            assert!(
                page.len() as i64 <= bound,
                "case {page_size:?}: a page holds at most {bound} entries, got {} ({body})",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {page_size:?}: paging must terminate, saw {pages} pages and {seen:?}"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {page_size:?}: paging must report every configuration exactly once in ResourceName order, got {seen:?}"
        );
    }
}

/// `NextToken` values this service never issued: text that is not hex at all, hex of
/// text no `ResourceName` could be (the modeled pattern
/// `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$` admits neither a colon, a slash nor a leading
/// hyphen), and hex of a name one character past the published `ResourceName` maximum.
fn foreign_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("not hex at all", "not-a-page-token".to_string()),
        ("hex with an odd length", "abc".to_string()),
        (
            "hex of an ARN rather than a resource name",
            hex::encode("arn:aws:codeconnections:eu-west-1:000000000000:connection/abc"),
        ),
        (
            "hex of a name the modeled pattern refuses",
            hex::encode("-leading-hyphen"),
        ),
        (
            "hex of a name one past the published length maximum",
            hex::encode("n".repeat(RESOURCE_NAME_MAX_LEN + 1)),
        ),
    ]
}

/// A `RepositoryLinkId` the store does not hold is refused with the operation's modeled
/// `InvalidInputException` (400) — the recorded case this issue was filed for — and so
/// is a `NextToken` this service never issued; a `MaxResults` past the member's published
/// maximum is refused by the modeled range. None of them is answered with an empty page,
/// a success or the unmodeled 501.
#[tokio::test]
async fn listing_refuses_an_unresolvable_link_a_foreign_token_or_an_out_of_range_page() {
    let router = router();
    // A link that IS there, holding a configuration, so "unresolvable" can never mean
    // "the store is empty".
    let live = ListCase {
        label: "live link",
        provider_type: "GitHub",
        resource_names: vec!["LiveResource".to_string()],
    };
    let live_link = seed(&router, &live, 80).await;

    for id in ABSENT_LINK_IDS {
        let (status, body) = list(&router, id, None, None).await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected 400 for a RepositoryLinkId the store does not hold, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {id:?}: expected InvalidInputException, got {body}"
        );
        assert!(
            body["SyncConfigurations"].is_null(),
            "case {id:?}: a refusal reports no configuration, got {body}"
        );
    }

    for (label, token) in foreign_tokens() {
        let (status, body) = list(&router, &live_link, None, Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for a token this service did not issue, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {label:?}: expected InvalidInputException, got {body}"
        );
    }

    // One past the published `MaxResults` maximum: the modeled range refuses it before
    // the handler runs, so the answer is a 400 and never a page.
    let (status, body) = list(&router, &live_link, Some(MAX_RESULTS_MAX + 1), None).await;
    assert_eq!(
        status, 400,
        "case \"MaxResults past the published maximum\": expected 400, got {status} {body}"
    );
    assert!(
        body["SyncConfigurations"].is_null(),
        "case \"MaxResults past the published maximum\": a refusal reports no configuration, got {body}"
    );

    // And the live link still reports its own configuration: a refusal wrote nothing.
    let (status, body) = list(&router, &live_link, None, None).await;
    assert_eq!(
        status, 200,
        "case \"live link\": expected 200 from ListSyncConfigurations, got {status} {body}"
    );
    assert_eq!(
        reported_names("live link", &body),
        live.resource_names,
        "case \"live link\": the live configuration must still be reported, got {body}"
    );
}
