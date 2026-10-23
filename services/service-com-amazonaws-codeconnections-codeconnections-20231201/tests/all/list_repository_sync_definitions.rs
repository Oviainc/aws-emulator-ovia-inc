// SPDX-License-Identifier: CC0-1.0

//! `ListRepositorySyncDefinitions` over the wire: the report of the repository sync
//! definitions of ONE repository link.
//!
//! The rule these suites assert: this report holds exactly one definition per stored
//! sync configuration of the `RepositoryLinkId` and `SyncType` the request names, in the
//! store's own total `ResourceName` order, each definition's members read off that same
//! stored row — `Branch` is the configuration's branch, `Directory` its `ConfigFile`
//! ("This value comes from creating or updating the `config-file` field of a
//! `sync-configuration`", from the `RepositorySyncDefinition` shape), and `Parent` and
//! `Target` the resource it syncs, equal to each other because the `Target` member's own
//! documentation says "In some cases, such as CFN_STACK_SYNC, the parent and target
//! resource are the same" and `CFN_STACK_SYNC` is the only value the modeled
//! `SyncConfigurationType` admits; the link the input names is RESOLVED first, so a
//! well-formed `RepositoryLinkId` the store does not hold is refused with the
//! operation's modeled `InvalidInputException` (400) rather than answered with an empty
//! list; and the report is pure — reading it twice answers the same thing, and what it
//! reports changes only when a sync configuration is created, updated or deleted.
//!
//! The operation's input models NEITHER `MaxResults` NOR `NextToken`
//! (`smithy_query(query="operation", operation="ListRepositorySyncDefinitions")`), so no
//! page can be resumed: the whole selection is one response and no `NextToken` ever
//! comes back. That is asserted on every row below.
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
    role_arn,
};

/// The published bounds of the modeled `BranchName` (`length 1..255`, from
/// `smithy_query(query="operation", operation="CreateSyncConfiguration")`).
const BRANCH_NAME_MAX_LEN: usize = 255;

/// The published maximum length of the modeled `ResourceName` (`length 1..100`, from
/// `smithy_query(query="shape", shape="ResourceName")`).
const RESOURCE_NAME_MAX_LEN: usize = 100;

/// One stored sync configuration of a row: the three members a repository sync
/// definition is projected from.
#[derive(Clone)]
struct Definition {
    resource_name: String,
    branch: String,
    config_file: String,
}

/// One row of the table: a repository link, and the sync configurations created against
/// it.
struct DefinitionCase {
    /// What a failing assertion names this row by.
    label: &'static str,
    /// The provider type of the connection the link is created on.
    provider_type: &'static str,
    /// The configurations this link holds, in the order they are created — deliberately
    /// NOT in `ResourceName` order, so a report that merely echoes creation order fails.
    definitions: Vec<Definition>,
}

fn definition(resource_name: &str, branch: &str, config_file: &str) -> Definition {
    Definition {
        resource_name: resource_name.to_string(),
        branch: branch.to_string(),
        config_file: config_file.to_string(),
    }
}

/// The table: links differing in how many configurations they hold (none, one, several),
/// in `ResourceName` form and length (each form the modeled pattern
/// `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$` admits, and each published bound of its
/// `length 1..100`), in branch form and length (each published bound of `BranchName`
/// 1..255), and in config-file path form.
fn definition_cases() -> Vec<DefinitionCase> {
    vec![
        DefinitionCase {
            label: "a link that syncs nothing",
            provider_type: "GitHub",
            definitions: Vec::new(),
        },
        DefinitionCase {
            label: "a link with one definition",
            provider_type: "Bitbucket",
            definitions: vec![definition("SoleStack", "main", "stacks/sole.yaml")],
        },
        DefinitionCase {
            label: "names and branches that sort against their creation order",
            provider_type: "GitLab",
            definitions: vec![
                definition("zeta", "release", "deploy/zeta.yml"),
                definition("Alpha", "main", "alpha.yaml"),
                definition("middle_stack", "feature/middle", "a/b/c/middle.json"),
                definition("Stack-9", "hotfix-9", ".aws/stack9.yaml"),
            ],
        },
        DefinitionCase {
            label: "branch and name at each published length bound",
            provider_type: "AzureDevOps",
            definitions: vec![
                definition("A", "b", "min.yaml"),
                definition(
                    &format!("L{}", "o".repeat(RESOURCE_NAME_MAX_LEN - 1)),
                    &"b".repeat(BRANCH_NAME_MAX_LEN),
                    &format!("{}/long.yaml", "d".repeat(64)),
                ),
            ],
        },
        DefinitionCase {
            label: "provider type and name forms the issue did not cite",
            provider_type: "GitHubEnterpriseServer",
            definitions: vec![
                definition("0leading-digit", "main", "digits.yaml"),
                definition("Mixed_Case-123", "MiXeD/Branch", "Mixed Case Path.yaml"),
                definition("9", "9", "9.yaml"),
            ],
        },
    ]
}

/// The `CreateSyncConfiguration` row for one definition.
fn sync_case(label: &'static str, wanted: &Definition) -> SyncCase {
    SyncCase {
        label,
        resource_name: wanted.resource_name.clone(),
        branch: wanted.branch.clone(),
        config_file: wanted.config_file.clone(),
        publish_deployment_status: None,
        trigger_resource_update_on: None,
        pull_request_comment: None,
        update: serde_json::Value::Null,
    }
}

/// Create one row's repository link and every sync configuration it names, and return
/// the `RepositoryLinkId`.
async fn seed(router: &MultiServiceRouter, case: &DefinitionCase, index: usize) -> String {
    let link = LinkCase {
        label: case.label,
        provider_type: case.provider_type,
        owner_id: format!("defs-owner-{index}"),
        repository_name: format!("defs-repo-{index}"),
        encryption_key_arn: None,
        tags: Vec::new(),
    };
    let (link_id, _) = create_link(router, &link, &format!("defs-conn-{index}")).await;

    for wanted in &case.definitions {
        let (status, created) = call(
            router,
            "CreateSyncConfiguration",
            create_sync_body(&sync_case(case.label, wanted), &link_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateSyncConfiguration for {:?}, got {status} {created}",
            case.label, wanted.resource_name
        );
    }
    link_id
}

/// Call `ListRepositorySyncDefinitions` for one link and sync type.
async fn list(
    router: &MultiServiceRouter,
    link_id: &str,
    sync_type: &str,
) -> (u16, serde_json::Value) {
    call(
        router,
        "ListRepositorySyncDefinitions",
        json!({ "RepositoryLinkId": link_id, "SyncType": sync_type }),
    )
    .await
}

/// The definitions a response reports, as `(Branch, Directory, Parent, Target)` in the
/// order it reported them. The modeled `RepositorySyncDefinitions` member is required,
/// so it must be an array even when empty, and every member of an entry is required too.
fn reported(case: &str, body: &serde_json::Value) -> Vec<(String, String, String, String)> {
    let definitions = body["RepositorySyncDefinitions"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: RepositorySyncDefinitions must be a list, got {body}")
    });
    definitions
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry[name]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: every entry carries a {name}, got {body}")
                    })
                    .to_string()
            };
            (
                member("Branch"),
                member("Directory"),
                member("Parent"),
                member("Target"),
            )
        })
        .collect()
}

/// What a set of stored configurations is expected to report, in `ResourceName` order:
/// `Branch` and `Directory` off the stored row, and `Parent` == `Target` == the resource
/// it syncs.
fn expected(definitions: &[Definition]) -> Vec<(String, String, String, String)> {
    let mut rows: Vec<Definition> = definitions.to_vec();
    rows.sort_by(|left, right| left.resource_name.cmp(&right.resource_name));
    rows.into_iter()
        .map(|row| {
            (
                row.branch,
                row.config_file,
                row.resource_name.clone(),
                row.resource_name,
            )
        })
        .collect()
}

/// The report holds exactly the definitions of the link and sync type it names, in
/// `ResourceName` order, each projected from the row `GetSyncConfiguration` reports; it
/// never leaks another link's definitions, it reports nothing for a link that syncs
/// nothing, it carries no `NextToken`, and it changes nothing a later caller sees.
#[tokio::test]
async fn the_report_holds_exactly_the_named_links_sync_definitions() {
    let router = router();
    let cases = definition_cases();

    let mut link_ids: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        link_ids.push(seed(&router, case, index).await);
    }

    for (case, link_id) in cases.iter().zip(&link_ids) {
        let (status, body) = list(&router, link_id, SYNC_TYPE).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListRepositorySyncDefinitions, got {status} {body}",
            case.label
        );
        assert_eq!(
            reported(case.label, &body),
            expected(&case.definitions),
            "case {:?}: expected this link's definitions in ResourceName order, got {body}",
            case.label
        );
        assert!(
            body["NextToken"].is_null(),
            "case {:?}: this operation's input models no continuation token, so none may \
             come back, got {body}",
            case.label
        );

        // Every reported entry is projected from the stored configuration, not invented:
        // its members are exactly what the single-resource reader reports for the same
        // resource, and that row belongs to THIS link.
        let order = expected(&case.definitions);
        for (position, (branch, directory, parent, target)) in order.iter().enumerate() {
            let (status, got) = call(
                &router,
                "GetSyncConfiguration",
                json!({ "SyncType": SYNC_TYPE, "ResourceName": parent }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected 200 from GetSyncConfiguration for {parent:?}, got {status} {got}",
                case.label
            );
            let stored = &got["SyncConfiguration"];
            assert_eq!(
                (
                    stored["Branch"].as_str().unwrap_or_default(),
                    stored["ConfigFile"].as_str().unwrap_or_default(),
                    stored["ResourceName"].as_str().unwrap_or_default(),
                    stored["RepositoryLinkId"].as_str().unwrap_or_default(),
                ),
                (
                    branch.as_str(),
                    directory.as_str(),
                    parent.as_str(),
                    link_id.as_str(),
                ),
                "case {:?}: entry {position} must be projected from the stored row of this \
                 link, got {body} against {got}",
                case.label
            );
            assert_eq!(
                parent, target,
                "case {:?}: entry {position} syncs one CFN_STACK_SYNC resource, so Parent and \
                 Target are the same, got {body}",
                case.label
            );
        }

        // A report changes nothing: the same call answers the same thing.
        let (status, again) = list(&router, link_id, SYNC_TYPE).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from the repeated list, got {status} {again}",
            case.label
        );
        assert_eq!(
            again["RepositorySyncDefinitions"], body["RepositorySyncDefinitions"],
            "case {:?}: listing must not change what a later caller sees, got {again}",
            case.label
        );
    }
}

/// The report is derived from the stored configurations, so every write to one shows up
/// in it: an update to a configuration's branch or config file moves the definition's
/// `Branch` and `Directory`, a configuration deleted leaves the report (and only it
/// does), and a configuration moved to another link moves between the two reports.
#[tokio::test]
async fn the_report_follows_every_write_to_a_sync_configuration() {
    let router = router();
    let case = DefinitionCase {
        label: "written link",
        provider_type: "GitLabSelfManaged",
        definitions: vec![
            definition("WrittenOne", "main", "one.yaml"),
            definition("WrittenTwo", "release", "two.yaml"),
            definition("WrittenThree", "trunk", "three.yaml"),
        ],
    };
    let link_id = seed(&router, &case, 70).await;
    // A second link with its own configuration, so a report that over-reads the store
    // cannot pass as a correct report of this one.
    let other = DefinitionCase {
        label: "other link",
        provider_type: "GitHub",
        definitions: vec![definition("OtherStack", "main", "other.yaml")],
    };
    let other_link = seed(&router, &other, 71).await;

    // An update to branch and config file: the definition is re-derived, not cached.
    let (status, updated) = call(
        &router,
        "UpdateSyncConfiguration",
        json!({
            "SyncType": SYNC_TYPE,
            "ResourceName": "WrittenTwo",
            "Branch": "moved-branch",
            "ConfigFile": "moved/two.yaml",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"update branch and config file\": expected 200 from UpdateSyncConfiguration, \
         got {status} {updated}"
    );
    let mut after_update = case.definitions.clone();
    after_update[1] = definition("WrittenTwo", "moved-branch", "moved/two.yaml");
    let (_, body) = list(&router, &link_id, SYNC_TYPE).await;
    assert_eq!(
        reported("update branch and config file", &body),
        expected(&after_update),
        "case \"update branch and config file\": the report must follow the stored row, got {body}"
    );

    // A configuration this service no longer holds leaves the report, and only it does.
    let (status, deleted) = call(
        &router,
        "DeleteSyncConfiguration",
        json!({ "SyncType": SYNC_TYPE, "ResourceName": "WrittenOne" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete\": expected 200 from DeleteSyncConfiguration, got {status} {deleted}"
    );
    let remaining: Vec<Definition> = after_update
        .iter()
        .filter(|row| row.resource_name != "WrittenOne")
        .cloned()
        .collect();
    let (_, body) = list(&router, &link_id, SYNC_TYPE).await;
    assert_eq!(
        reported("delete", &body),
        expected(&remaining),
        "case \"delete\": the deleted configuration must leave the report, got {body}"
    );

    // Re-pointing a configuration at the other link moves its definition between the two
    // reports: the report selects on the configuration's stored `RepositoryLinkId`.
    let (status, moved) = call(
        &router,
        "UpdateSyncConfiguration",
        json!({
            "SyncType": SYNC_TYPE,
            "ResourceName": "WrittenThree",
            "RepositoryLinkId": other_link,
            "RoleArn": role_arn("git-sync"),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"re-point at the other link\": expected 200 from UpdateSyncConfiguration, got \
         {status} {moved}"
    );
    let left_behind: Vec<Definition> = remaining
        .iter()
        .filter(|row| row.resource_name != "WrittenThree")
        .cloned()
        .collect();
    let (_, body) = list(&router, &link_id, SYNC_TYPE).await;
    assert_eq!(
        reported("re-point at the other link", &body),
        expected(&left_behind),
        "case \"re-point at the other link\": the moved configuration must leave the old \
         link's report, got {body}"
    );
    let mut joined = other.definitions.clone();
    joined.push(definition("WrittenThree", "trunk", "three.yaml"));
    let (_, body) = list(&router, &other_link, SYNC_TYPE).await;
    assert_eq!(
        reported("re-point at the other link", &body),
        expected(&joined),
        "case \"re-point at the other link\": the moved configuration must join the new \
         link's report, got {body}"
    );
}

/// A `RepositoryLinkId` the store does not hold is refused with the operation's modeled
/// `InvalidInputException` (400) — the recorded case this issue was filed for — and a
/// `SyncType` the modeled enum does not admit is refused by the model before the handler
/// runs. Neither is answered with an empty list, a success or the unmodeled 501.
#[tokio::test]
async fn the_report_refuses_an_unresolvable_link_or_an_unmodeled_sync_type() {
    let router = router();
    // A link that IS there, syncing a resource, so "unresolvable" can never mean "the
    // store is empty".
    let live = DefinitionCase {
        label: "live link",
        provider_type: "GitHub",
        definitions: vec![definition("LiveDefinition", "main", "live.yaml")],
    };
    let live_link = seed(&router, &live, 60).await;

    for id in ABSENT_LINK_IDS {
        let (status, body) = list(&router, id, SYNC_TYPE).await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected 400 for a RepositoryLinkId the store does not hold, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {id:?}: expected InvalidInputException, got {body}"
        );
        assert!(
            body["RepositorySyncDefinitions"].is_null(),
            "case {id:?}: a refusal reports no definition, got {body}"
        );
    }

    // Values outside the modeled `SyncConfigurationType` enum (whose only value is
    // `CFN_STACK_SYNC`): the request walk refuses them before the handler runs.
    for sync_type in ["cfn_stack_sync", "OTHER_SYNC", ""] {
        let (status, body) = list(&router, &live_link, sync_type).await;
        assert_eq!(
            status, 400,
            "case {sync_type:?}: expected 400 for a SyncType the enum does not admit, got \
             {status} {body}"
        );
        assert!(
            body["RepositorySyncDefinitions"].is_null(),
            "case {sync_type:?}: a refusal reports no definition, got {body}"
        );
    }

    // And the live link still reports its own definition: a refusal wrote nothing.
    let (status, body) = list(&router, &live_link, SYNC_TYPE).await;
    assert_eq!(
        status, 200,
        "case \"live link\": expected 200 from ListRepositorySyncDefinitions, got {status} {body}"
    );
    assert_eq!(
        reported("live link", &body),
        expected(&live.definitions),
        "case \"live link\": the live definition must still be reported, got {body}"
    );
}
