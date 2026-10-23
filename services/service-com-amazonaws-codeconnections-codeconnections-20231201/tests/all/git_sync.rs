// SPDX-License-Identifier: CC0-1.0

//! The Git sync surface over the wire: repository links and sync configurations.
//!
//! The rule these suites assert: every repository-link and sync-configuration
//! operation RESOLVES the resource its input names against this service's store and
//! answers from the stored row — a request naming a resource the store does not hold
//! is refused with that operation's own modeled error, and a request naming one it
//! does holds reports (or updates) exactly what was persisted. Each table below spans
//! the class the rule generalises over; the cases the issue cited are single rows.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers (`router`, `call`, `create`), so these tests
//! exercise the same path a signed SDK request takes.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codeconnections_codeconnections_20231201::ops::types::SyncBlockerContext;
use service_com_amazonaws_codeconnections_codeconnections_20231201::state::{
    ComAmazonawsCodeconnectionsCodeconnections20231201State, StoredSyncBlocker,
};

use super::connection_lifecycle::{PROVIDER_TYPES, REGION, call, create, router, router_with_engine};

/// The only value the modeled `SyncConfigurationType` enum accepts, from
/// `smithy_query(query="shape", shape="SyncConfigurationType")`.
pub(crate) const SYNC_TYPE: &str = "CFN_STACK_SYNC";

/// A `RoleArn` matching the modeled `IamRoleArn` pattern
/// `^arn:aws(-[\w]+)*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+$`.
pub(crate) fn role_arn(name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{name}",
        engine::emulator_account_id()
    )
}

/// An `EncryptionKeyArn` matching the modeled `KmsKeyArn` pattern
/// `^arn:aws(-[\w]+)*:kms:[a-z\-0-9]+:\d{12}:key/[a-zA-Z0-9\-]+$`.
fn kms_key_arn(key: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:key/{key}",
        engine::emulator_account_id()
    )
}

/// One `CreateRepositoryLink` request whose round trip a suite asserts.
pub(crate) struct LinkCase {
    /// What a failing assertion names this row by.
    pub(crate) label: &'static str,
    /// The provider type of the connection the link is created on — and therefore the
    /// `ProviderType` the link must report, since `RepositoryLinkInfo.ProviderType` is
    /// the provider type OF THE CONNECTION.
    pub(crate) provider_type: &'static str,
    pub(crate) owner_id: String,
    pub(crate) repository_name: String,
    pub(crate) encryption_key_arn: Option<String>,
    /// Tags as `(Key, Value)`, in the order the create supplies them.
    pub(crate) tags: Vec<(&'static str, &'static str)>,
}

/// The table: one row per modeled `ProviderType`, differing also in owner-id and
/// repository-name form and length (each published bound of `OwnerId` 1..255 and
/// `RepositoryName` 1..128, from `smithy_query(query="operation",
/// operation="CreateRepositoryLink")`), whether an encryption key is supplied, and how
/// many tags the link carries.
fn link_cases() -> Vec<LinkCase> {
    vec![
        LinkCase {
            label: "plain link, no key, no tags",
            provider_type: "GitHub",
            owner_id: "acme".to_string(),
            repository_name: "service".to_string(),
            encryption_key_arn: None,
            tags: Vec::new(),
        },
        LinkCase {
            label: "encryption key and one tag",
            provider_type: "Bitbucket",
            owner_id: "acme-team".to_string(),
            repository_name: "infra/platform".to_string(),
            encryption_key_arn: Some(kms_key_arn("1234abcd-12ab-34cd-56ef-1234567890ab")),
            tags: vec![("team", "platform")],
        },
        LinkCase {
            label: "owner and repository at their published minimum length",
            provider_type: "GitLab",
            owner_id: "o".to_string(),
            repository_name: "r".to_string(),
            encryption_key_arn: None,
            tags: vec![("only", "one")],
        },
        LinkCase {
            label: "owner and repository at their published maximum length",
            provider_type: "GitLabSelfManaged",
            owner_id: "o".repeat(255),
            repository_name: "r".repeat(128),
            encryption_key_arn: Some(kms_key_arn("abcdef01-2345-6789-abcd-ef0123456789")),
            tags: vec![("a", "1"), ("b", "2")],
        },
        LinkCase {
            label: "a provider type the issue did not cite",
            provider_type: "AzureDevOps",
            owner_id: "contoso".to_string(),
            repository_name: "Repo.With.Dots".to_string(),
            encryption_key_arn: None,
            tags: vec![("env", "")],
        },
        LinkCase {
            label: "host-backed provider",
            provider_type: "GitHubEnterpriseServer",
            owner_id: "enterprise-owner".to_string(),
            repository_name: "deep/nested/name".to_string(),
            encryption_key_arn: None,
            tags: Vec::new(),
        },
    ]
}

/// Render a `(Key, Value)` table row as the wire `TagList`.
fn tag_list(tags: &[(&str, &str)]) -> serde_json::Value {
    serde_json::Value::Array(
        tags.iter()
            .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// The `CreateRepositoryLink` body for one row, on `connection_arn`.
fn create_link_body(case: &LinkCase, connection_arn: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "ConnectionArn": connection_arn,
        "OwnerId": case.owner_id,
        "RepositoryName": case.repository_name,
    });
    if let Some(key) = &case.encryption_key_arn {
        body["EncryptionKeyArn"] = serde_json::Value::String(key.clone());
    }
    if !case.tags.is_empty() {
        body["Tags"] = tag_list(&case.tags);
    }
    body
}

/// Assert `id` is the 8-4-4-4-12 lowercase-hex label the modeled `RepositoryLinkId`
/// pattern `^[0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{4}\b-[0-9a-fA-F]{12}$`
/// requires.
fn assert_repository_link_id(case: &str, id: &str) {
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected an 8-4-4-4-12 RepositoryLinkId, got {id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "case {case:?}: expected hex in every group, got {id:?}"
    );
}

/// Assert `arn` satisfies the modeled `RepositoryLinkArn` pattern
/// `^arn:aws(?:-[a-z]+)*:(codestar-connections|codeconnections):[a-z\-0-9]+:\d{12}:repository-link\/[a-zA-Z0-9\-:/]+$`
/// and names the link `id` in the region the caller signed for.
fn assert_repository_link_arn(case: &str, arn: &str, id: &str) {
    assert_eq!(
        arn,
        format!(
            "arn:aws:codeconnections:{REGION}:{}:repository-link/{id}",
            engine::emulator_account_id()
        ),
        "case {case:?}: expected a repository-link ARN for {id:?} in the signed region, got {arn:?}"
    );
}

/// Create a repository link for one row and return `(RepositoryLinkId, connection ARN)`.
pub(crate) async fn create_link(
    router: &MultiServiceRouter,
    case: &LinkCase,
    connection_name: &str,
) -> (String, String) {
    let connection_arn = create(router, connection_name, case.provider_type).await;
    let (status, body) = call(
        router,
        "CreateRepositoryLink",
        create_link_body(case, &connection_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200 from CreateRepositoryLink, got {status} {body}",
        case.label
    );
    let id = body["RepositoryLinkInfo"]["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: CreateRepositoryLink returned no RepositoryLinkId: {body}",
                case.label
            )
        })
        .to_string();
    (id, connection_arn)
}

/// The error code a response body carries (`__type` on awsJson), or the empty string.
pub(crate) fn error_code(body: &serde_json::Value) -> &str {
    body["__type"]
        .as_str()
        .map(|kind| kind.rsplit('#').next().unwrap_or(kind))
        .unwrap_or_default()
}

/// A repository link round trips: every member `CreateRepositoryLink` persisted is what
/// it reported and what `GetRepositoryLink` reports back, its `ProviderType` is the
/// connection's, and its tags come back from `ListTagsForResource`.
#[tokio::test]
async fn a_repository_link_round_trips_from_create_to_get() {
    let router = router();
    let mut ids: Vec<String> = Vec::new();

    for (index, case) in link_cases().iter().enumerate() {
        let (id, connection_arn) = create_link(&router, case, &format!("conn-{index}")).await;
        assert_repository_link_id(case.label, &id);
        assert!(
            !ids.contains(&id),
            "case {:?}: every repository link gets a distinct id, saw {id:?} twice",
            case.label
        );
        ids.push(id.clone());

        let (status, got) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRepositoryLink, got {status} {got}",
            case.label
        );
        let info = &got["RepositoryLinkInfo"];
        assert_eq!(
            info["ConnectionArn"],
            serde_json::json!(connection_arn),
            "case {:?}: expected the stored ConnectionArn, got {got}",
            case.label
        );
        assert_eq!(
            info["OwnerId"],
            serde_json::json!(case.owner_id),
            "case {:?}: expected the stored OwnerId, got {got}",
            case.label
        );
        assert_eq!(
            info["RepositoryName"],
            serde_json::json!(case.repository_name),
            "case {:?}: expected the stored RepositoryName, got {got}",
            case.label
        );
        // Derived from the connection, never echoed from the request: the create body
        // carries no ProviderType at all.
        assert_eq!(
            info["ProviderType"],
            serde_json::json!(case.provider_type),
            "case {:?}: expected the connection's ProviderType, got {got}",
            case.label
        );
        assert_eq!(
            info["RepositoryLinkId"],
            serde_json::json!(id),
            "case {:?}: expected the stored RepositoryLinkId, got {got}",
            case.label
        );
        let arn = info["RepositoryLinkArn"].as_str().unwrap_or_else(|| {
            panic!("case {:?}: no RepositoryLinkArn: {got}", case.label)
        });
        assert_repository_link_arn(case.label, arn, &id);
        match &case.encryption_key_arn {
            Some(key) => assert_eq!(
                info["EncryptionKeyArn"],
                serde_json::json!(key),
                "case {:?}: expected the stored EncryptionKeyArn, got {got}",
                case.label
            ),
            None => assert!(
                info["EncryptionKeyArn"].is_null(),
                "case {:?}: a link created with no EncryptionKeyArn reports none, got {got}",
                case.label
            ),
        }

        let (status, tags) = call(
            &router,
            "ListTagsForResource",
            serde_json::json!({ "ResourceArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from ListTagsForResource, got {status} {tags}",
            case.label
        );
        assert_eq!(
            tags["Tags"],
            tag_list(&case.tags),
            "case {:?}: expected the tags the create supplied, got {tags}",
            case.label
        );
    }
}

/// Registering the same (connection, owner, repository) triple twice is refused with
/// the modeled `ResourceAlreadyExistsException` (409), and a link on a connection the
/// store does not hold is refused with the modeled `InvalidInputException` (400) — one
/// row per table case, so the refusal is not keyed to one shape of input.
#[tokio::test]
async fn a_repository_link_is_refused_for_a_duplicate_or_an_unresolvable_connection() {
    let router = router();

    for (index, case) in link_cases().iter().enumerate() {
        let (_, connection_arn) = create_link(&router, case, &format!("conn-{index}")).await;

        let (status, body) = call(
            &router,
            "CreateRepositoryLink",
            create_link_body(case, &connection_arn),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {:?}: expected 409 for a duplicate link, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ResourceAlreadyExistsException",
            "case {:?}: expected ResourceAlreadyExistsException, got {body}",
            case.label
        );

        // The same owner and repository through ANOTHER connection is a different
        // link, so it is accepted: only the whole triple is the duplicate.
        let other = create(&router, &format!("other-conn-{index}"), case.provider_type).await;
        let (status, body) = call(
            &router,
            "CreateRepositoryLink",
            create_link_body(case, &other),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the same repository through another connection is a new link, got {status} {body}",
            case.label
        );

        // And a connection ARN this store does not hold resolves to nothing.
        let absent = format!(
            "arn:aws:codeconnections:{REGION}:{}:connection/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
            engine::emulator_account_id()
        );
        let (status, body) = call(
            &router,
            "CreateRepositoryLink",
            create_link_body(case, &absent),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400 for an unresolvable ConnectionArn, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {:?}: expected InvalidInputException, got {body}",
            case.label
        );
    }
}

/// `UpdateRepositoryLink` replaces the members the request carries and leaves the rest
/// alone, and a replaced connection re-derives the link's `ProviderType`: one row per
/// provider type, each moved onto a connection of a different provider.
#[tokio::test]
async fn updating_a_repository_link_replaces_only_what_it_names() {
    let router = router();

    for (index, provider) in PROVIDER_TYPES.iter().enumerate() {
        // The provider the link is moved to: the next one in the list, so every row
        // changes provider and the last row wraps round.
        let replacement_provider = PROVIDER_TYPES[(index + 1) % PROVIDER_TYPES.len()];
        let case = LinkCase {
            label: "update row",
            provider_type: provider,
            owner_id: format!("owner-{index}"),
            repository_name: format!("repo-{index}"),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        let (id, original_connection) = create_link(&router, &case, &format!("conn-{index}")).await;

        // An update that names nothing changes nothing.
        let (status, unchanged) = call(
            &router,
            "UpdateRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {provider:?}: expected 200 from an empty UpdateRepositoryLink, got {status} {unchanged}"
        );
        assert_eq!(
            unchanged["RepositoryLinkInfo"]["ConnectionArn"],
            serde_json::json!(original_connection),
            "case {provider:?}: an update naming nothing must change nothing, got {unchanged}"
        );
        assert_eq!(
            unchanged["RepositoryLinkInfo"]["ProviderType"],
            serde_json::json!(provider),
            "case {provider:?}: an update naming nothing must change nothing, got {unchanged}"
        );

        // Then the replacement connection and encryption key.
        let replacement_connection = create(
            &router,
            &format!("replacement-{index}"),
            replacement_provider,
        )
        .await;
        let key = kms_key_arn(&format!("0000000{index}-12ab-34cd-56ef-1234567890ab"));
        let (status, updated) = call(
            &router,
            "UpdateRepositoryLink",
            serde_json::json!({
                "RepositoryLinkId": id,
                "ConnectionArn": replacement_connection,
                "EncryptionKeyArn": key,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {provider:?}: expected 200 from UpdateRepositoryLink, got {status} {updated}"
        );
        assert_eq!(
            updated["RepositoryLinkInfo"]["ConnectionArn"],
            serde_json::json!(replacement_connection),
            "case {provider:?}: expected the replacement ConnectionArn, got {updated}"
        );
        assert_eq!(
            updated["RepositoryLinkInfo"]["ProviderType"],
            serde_json::json!(replacement_provider),
            "case {provider:?}: a link's ProviderType is its connection's, got {updated}"
        );
        assert_eq!(
            updated["RepositoryLinkInfo"]["EncryptionKeyArn"],
            serde_json::json!(key),
            "case {provider:?}: expected the replacement EncryptionKeyArn, got {updated}"
        );
        // The members the update did not name are untouched.
        assert_eq!(
            updated["RepositoryLinkInfo"]["OwnerId"],
            serde_json::json!(case.owner_id),
            "case {provider:?}: OwnerId was not named, so it must be unchanged, got {updated}"
        );
        assert_eq!(
            updated["RepositoryLinkInfo"]["RepositoryName"],
            serde_json::json!(case.repository_name),
            "case {provider:?}: RepositoryName was not named, so it must be unchanged, got {updated}"
        );

        // The write is what a later reader sees.
        let (status, got) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {provider:?}: expected 200 from GetRepositoryLink, got {status} {got}"
        );
        assert_eq!(
            got["RepositoryLinkInfo"], updated["RepositoryLinkInfo"],
            "case {provider:?}: GetRepositoryLink must report the update, got {got}"
        );

        // A replacement connection the store does not hold is refused, and the link is
        // left as it was.
        let absent = format!(
            "arn:aws:codeconnections:{REGION}:{}:connection/1a2b3c4d-5e6f-7081-9293-a4b5c6d7e8f9",
            engine::emulator_account_id()
        );
        let (status, body) = call(
            &router,
            "UpdateRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id, "ConnectionArn": absent }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {provider:?}: expected 400 for an unresolvable ConnectionArn, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {provider:?}: expected InvalidInputException, got {body}"
        );
        let (_, after) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            after["RepositoryLinkInfo"], updated["RepositoryLinkInfo"],
            "case {provider:?}: a refused update must write nothing, got {after}"
        );
    }
}

/// One `CreateSyncConfiguration` request, and the member an update then replaces.
pub(crate) struct SyncCase {
    pub(crate) label: &'static str,
    pub(crate) resource_name: String,
    pub(crate) branch: String,
    pub(crate) config_file: String,
    pub(crate) publish_deployment_status: Option<&'static str>,
    pub(crate) trigger_resource_update_on: Option<&'static str>,
    pub(crate) pull_request_comment: Option<&'static str>,
    /// The `UpdateSyncConfiguration` members this row then replaces, as wire fields.
    pub(crate) update: serde_json::Value,
}

/// The table: configurations differing in resource-name form, branch length (each
/// published bound of `BranchName` 1..255), config-file path, and which of the three
/// modeled toggles they carry — `PublishDeploymentStatus` (ENABLED|DISABLED),
/// `TriggerResourceUpdateOn` (ANY_CHANGE|FILE_CHANGE) and `PullRequestComment`
/// (ENABLED|DISABLED), from `smithy_query(query="shape",
/// shape="PublishDeploymentStatus,TriggerResourceUpdateOn,PullRequestComment")` — plus
/// one update per row naming a different member.
fn sync_cases() -> Vec<SyncCase> {
    vec![
        SyncCase {
            label: "no toggles, update the branch",
            resource_name: "DemoStack".to_string(),
            branch: "main".to_string(),
            config_file: "deploy.yaml".to_string(),
            publish_deployment_status: None,
            trigger_resource_update_on: None,
            pull_request_comment: None,
            update: serde_json::json!({ "Branch": "release" }),
        },
        SyncCase {
            label: "every toggle enabled, update the publish status",
            resource_name: "stack_with_underscores".to_string(),
            branch: "feature/sync".to_string(),
            config_file: "infra/stack.yml".to_string(),
            publish_deployment_status: Some("ENABLED"),
            trigger_resource_update_on: Some("ANY_CHANGE"),
            pull_request_comment: Some("ENABLED"),
            update: serde_json::json!({ "PublishDeploymentStatus": "DISABLED" }),
        },
        SyncCase {
            label: "every toggle disabled, update the trigger",
            resource_name: "stack-with-hyphens".to_string(),
            branch: "b".to_string(),
            config_file: "a.yaml".to_string(),
            publish_deployment_status: Some("DISABLED"),
            trigger_resource_update_on: Some("FILE_CHANGE"),
            pull_request_comment: Some("DISABLED"),
            update: serde_json::json!({ "TriggerResourceUpdateOn": "ANY_CHANGE" }),
        },
        SyncCase {
            label: "branch at its published maximum length, update the config file",
            resource_name: "Stack4".to_string(),
            branch: "b".repeat(255),
            config_file: "deeply/nested/path/to/template.yaml".to_string(),
            publish_deployment_status: Some("ENABLED"),
            trigger_resource_update_on: None,
            pull_request_comment: Some("DISABLED"),
            update: serde_json::json!({ "ConfigFile": "other.yaml" }),
        },
        SyncCase {
            label: "update the role",
            resource_name: "a".to_string(),
            branch: "main".to_string(),
            config_file: "t.yaml".to_string(),
            publish_deployment_status: None,
            trigger_resource_update_on: Some("ANY_CHANGE"),
            pull_request_comment: None,
            update: serde_json::json!({ "RoleArn": "arn:aws:iam::000000000000:role/replacement-role" }),
        },
        SyncCase {
            label: "update several members at once",
            resource_name: "ManyMembers".to_string(),
            branch: "main".to_string(),
            config_file: "t.yaml".to_string(),
            publish_deployment_status: None,
            trigger_resource_update_on: None,
            pull_request_comment: Some("ENABLED"),
            update: serde_json::json!({
                "Branch": "next",
                "ConfigFile": "next.yaml",
                "PullRequestComment": "DISABLED",
                "TriggerResourceUpdateOn": "FILE_CHANGE",
            }),
        },
    ]
}

/// The `CreateSyncConfiguration` body for one row, against `repository_link_id`.
pub(crate) fn create_sync_body(case: &SyncCase, repository_link_id: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "Branch": case.branch,
        "ConfigFile": case.config_file,
        "RepositoryLinkId": repository_link_id,
        "ResourceName": case.resource_name,
        "RoleArn": role_arn("git-sync"),
        "SyncType": SYNC_TYPE,
    });
    for (field, value) in [
        ("PublishDeploymentStatus", case.publish_deployment_status),
        ("TriggerResourceUpdateOn", case.trigger_resource_update_on),
        ("PullRequestComment", case.pull_request_comment),
    ] {
        if let Some(value) = value {
            body[field] = serde_json::Value::String(value.to_string());
        }
    }
    body
}

/// What a `SyncConfiguration` must carry for one row, given the link it was created
/// against: the members the request supplied, plus the three the LINK determines.
fn expected_sync_configuration(
    case: &SyncCase,
    link: &serde_json::Value,
    repository_link_id: &str,
) -> serde_json::Value {
    let mut expected = serde_json::json!({
        "Branch": case.branch,
        "ConfigFile": case.config_file,
        "OwnerId": link["OwnerId"],
        "ProviderType": link["ProviderType"],
        "RepositoryLinkId": repository_link_id,
        "RepositoryName": link["RepositoryName"],
        "ResourceName": case.resource_name,
        "RoleArn": role_arn("git-sync"),
        "SyncType": SYNC_TYPE,
    });
    for (field, value) in [
        ("PublishDeploymentStatus", case.publish_deployment_status),
        ("TriggerResourceUpdateOn", case.trigger_resource_update_on),
        ("PullRequestComment", case.pull_request_comment),
    ] {
        if let Some(value) = value {
            expected[field] = serde_json::Value::String(value.to_string());
        }
    }
    expected
}

/// A sync configuration round trips from create to get, and `UpdateSyncConfiguration`
/// replaces exactly the members the request names — every other member, including the
/// three the repository link determines, is left alone.
#[tokio::test]
async fn a_sync_configuration_round_trips_and_updates_only_what_it_names() {
    let router = router();

    for (index, case) in sync_cases().iter().enumerate() {
        let link_case = LinkCase {
            label: "sync row",
            provider_type: PROVIDER_TYPES[index % PROVIDER_TYPES.len()],
            owner_id: format!("owner-{index}"),
            repository_name: format!("repo-{index}"),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        let (link_id, _) = create_link(&router, &link_case, &format!("conn-{index}")).await;
        let (status, link) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": link_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRepositoryLink, got {status} {link}",
            case.label
        );
        let link = link["RepositoryLinkInfo"].clone();

        let (status, created) = call(
            &router,
            "CreateSyncConfiguration",
            create_sync_body(case, &link_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateSyncConfiguration, got {status} {created}",
            case.label
        );
        let expected = expected_sync_configuration(case, &link, &link_id);
        assert_eq!(
            created["SyncConfiguration"], expected,
            "case {:?}: expected the configuration the create persisted, got {created}",
            case.label
        );

        let (status, got) = call(
            &router,
            "GetSyncConfiguration",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": case.resource_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetSyncConfiguration, got {status} {got}",
            case.label
        );
        assert_eq!(
            got["SyncConfiguration"], expected,
            "case {:?}: GetSyncConfiguration must report what was persisted, got {got}",
            case.label
        );

        // A second create for the same (SyncType, ResourceName) pair names a resource
        // that is already there.
        let (status, duplicate) = call(
            &router,
            "CreateSyncConfiguration",
            create_sync_body(case, &link_id),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {:?}: expected 409 for a duplicate configuration, got {status} {duplicate}",
            case.label
        );
        assert_eq!(
            error_code(&duplicate),
            "ResourceAlreadyExistsException",
            "case {:?}: expected ResourceAlreadyExistsException, got {duplicate}",
            case.label
        );

        // Now the update: only the named members move.
        let mut update = case.update.clone();
        update["SyncType"] = serde_json::Value::String(SYNC_TYPE.to_string());
        update["ResourceName"] = serde_json::Value::String(case.resource_name.clone());
        let (status, updated) = call(&router, "UpdateSyncConfiguration", update.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from UpdateSyncConfiguration, got {status} {updated}",
            case.label
        );
        let mut expected_after = expected.clone();
        for (field, value) in case.update.as_object().expect("the update is an object") {
            expected_after[field] = value.clone();
        }
        assert_eq!(
            updated["SyncConfiguration"], expected_after,
            "case {:?}: expected only the named members to move, got {updated}",
            case.label
        );

        let (status, got) = call(
            &router,
            "GetSyncConfiguration",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": case.resource_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetSyncConfiguration, got {status} {got}",
            case.label
        );
        assert_eq!(
            got["SyncConfiguration"], expected_after,
            "case {:?}: GetSyncConfiguration must report the update, got {got}",
            case.label
        );
    }
}

/// Repointing a sync configuration at another repository link re-derives the three
/// members the LINK determines (`OwnerId`, `ProviderType`, `RepositoryName`), and a
/// link id the store does not hold is refused with `InvalidInputException` (400).
#[tokio::test]
async fn repointing_a_sync_configuration_re_derives_the_repository_members() {
    let router = router();

    for (index, case) in sync_cases().iter().enumerate() {
        let first = LinkCase {
            label: "first link",
            provider_type: PROVIDER_TYPES[index % PROVIDER_TYPES.len()],
            owner_id: format!("first-owner-{index}"),
            repository_name: format!("first-repo-{index}"),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        let second = LinkCase {
            label: "second link",
            provider_type: PROVIDER_TYPES[(index + 2) % PROVIDER_TYPES.len()],
            owner_id: format!("second-owner-{index}"),
            repository_name: format!("second-repo-{index}"),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        let (first_id, _) = create_link(&router, &first, &format!("first-conn-{index}")).await;
        let (second_id, _) = create_link(&router, &second, &format!("second-conn-{index}")).await;

        let (status, created) = call(
            &router,
            "CreateSyncConfiguration",
            create_sync_body(case, &first_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateSyncConfiguration, got {status} {created}",
            case.label
        );

        let (status, updated) = call(
            &router,
            "UpdateSyncConfiguration",
            serde_json::json!({
                "SyncType": SYNC_TYPE,
                "ResourceName": case.resource_name,
                "RepositoryLinkId": second_id,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from UpdateSyncConfiguration, got {status} {updated}",
            case.label
        );
        let configuration = &updated["SyncConfiguration"];
        assert_eq!(
            configuration["RepositoryLinkId"],
            serde_json::json!(second_id),
            "case {:?}: expected the replacement RepositoryLinkId, got {updated}"
        , case.label);
        assert_eq!(
            configuration["OwnerId"],
            serde_json::json!(second.owner_id),
            "case {:?}: OwnerId follows the link, got {updated}",
            case.label
        );
        assert_eq!(
            configuration["RepositoryName"],
            serde_json::json!(second.repository_name),
            "case {:?}: RepositoryName follows the link, got {updated}",
            case.label
        );
        assert_eq!(
            configuration["ProviderType"],
            serde_json::json!(second.provider_type),
            "case {:?}: ProviderType follows the link, got {updated}",
            case.label
        );

        // A link id the store does not hold is refused, and nothing moves.
        let (status, body) = call(
            &router,
            "UpdateSyncConfiguration",
            serde_json::json!({
                "SyncType": SYNC_TYPE,
                "ResourceName": case.resource_name,
                "RepositoryLinkId": "deadbeef-dead-beef-dead-beefdeadbeef",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400 for an unresolvable RepositoryLinkId, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {:?}: expected InvalidInputException, got {body}",
            case.label
        );
        let (_, after) = call(
            &router,
            "GetSyncConfiguration",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": case.resource_name }),
        )
        .await;
        assert_eq!(
            after["SyncConfiguration"], *configuration,
            "case {:?}: a refused update must write nothing, got {after}",
            case.label
        );
    }
}

/// Well-formed `RepositoryLinkId` values the store does not hold: UUID-shaped labels
/// the modeled pattern admits, including both hex cases and the all-zero id.
pub(crate) const ABSENT_LINK_IDS: &[&str] = &[
    "00000000-0000-0000-0000-000000000000",
    "deadbeef-dead-beef-dead-beefdeadbeef",
    "DEADBEEF-DEAD-BEEF-DEAD-BEEFDEADBEEF",
    "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
];

/// `ResourceName` values the modeled pattern `^[0-9A-Za-z]+[0-9A-Za-z_\\-]*$` admits
/// that no sync configuration is created for.
const ABSENT_RESOURCE_NAMES: &[&str] = &[
    "nosuchresource",
    "A",
    "Stack-1",
    "stack_2",
    "MixedCase123",
];

/// Every operation that addresses a resource by id or by name refuses one the store
/// does not hold with ITS OWN modeled error, and at that error's HTTP status — never
/// the unmodeled 501 `NotImplemented`, and never a success. One row per well-formed
/// value above, so no refusal is keyed to a single recorded input.
#[tokio::test]
async fn an_unresolvable_resource_is_refused_with_the_operations_modeled_error() {
    let router = router();

    // A configuration that IS there, so "unresolvable" can never mean "the store is
    // empty".
    let link_case = LinkCase {
        label: "live link",
        provider_type: "GitHub",
        owner_id: "live-owner".to_string(),
        repository_name: "live-repo".to_string(),
        encryption_key_arn: None,
        tags: Vec::new(),
    };
    let (live_link, _) = create_link(&router, &link_case, "live-conn").await;
    let (status, _) = call(
        &router,
        "CreateSyncConfiguration",
        create_sync_body(
            &SyncCase {
                label: "live",
                resource_name: "LiveResource".to_string(),
                branch: "main".to_string(),
                config_file: "live.yaml".to_string(),
                publish_deployment_status: None,
                trigger_resource_update_on: None,
                pull_request_comment: None,
                update: serde_json::Value::Null,
            },
            &live_link,
        ),
    )
    .await;
    assert_eq!(status, 200, "the live configuration must be created");

    for id in ABSENT_LINK_IDS {
        // `GetRepositoryLink`: the link is the resource, so a missing one is the
        // modeled ResourceNotFoundException (404).
        let (status, body) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from GetRepositoryLink, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body}"
        );

        // `UpdateRepositoryLink`: same resource, same modeled 404.
        let (status, body) = call(
            &router,
            "UpdateRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": id }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 from UpdateRepositoryLink, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {body}"
        );

        // `GetRepositorySyncStatus`: an id naming no link makes the INPUT invalid, so
        // the modeled InvalidInputException (400) — a different code for a different
        // refusal.
        let (status, body) = call(
            &router,
            "GetRepositorySyncStatus",
            serde_json::json!({
                "Branch": "main",
                "RepositoryLinkId": id,
                "SyncType": SYNC_TYPE,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected 400 from GetRepositorySyncStatus, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {id:?}: expected InvalidInputException, got {body}"
        );

        // `CreateSyncConfiguration` resolves the link the same way.
        let (status, body) = call(
            &router,
            "CreateSyncConfiguration",
            serde_json::json!({
                "Branch": "main",
                "ConfigFile": "t.yaml",
                "RepositoryLinkId": id,
                "ResourceName": "ResourceForAbsentLink",
                "RoleArn": role_arn("git-sync"),
                "SyncType": SYNC_TYPE,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected 400 from CreateSyncConfiguration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {id:?}: expected InvalidInputException, got {body}"
        );
    }

    for name in ABSENT_RESOURCE_NAMES {
        // `GetSyncConfiguration`: nothing configured for the pair is the modeled
        // ResourceNotFoundException (404).
        let (status, body) = call(
            &router,
            "GetSyncConfiguration",
            serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": name }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404 from GetSyncConfiguration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {body}"
        );

        // `UpdateSyncConfiguration`: the same missing pair is the modeled
        // InvalidInputException (400) — the operation's own code.
        let (status, body) = call(
            &router,
            "UpdateSyncConfiguration",
            serde_json::json!({
                "SyncType": SYNC_TYPE,
                "ResourceName": name,
                "Branch": "main",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400 from UpdateSyncConfiguration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {name:?}: expected InvalidInputException, got {body}"
        );

        // `UpdateSyncBlocker`: the BLOCKER ID is resolved first, so an unknown blocker
        // surfaces SyncBlockerDoesNotExistException (404) even though this resource has
        // no sync configuration either.
        let (status, body) = call(
            &router,
            "UpdateSyncBlocker",
            serde_json::json!({
                "Id": "blocker-1",
                "SyncType": SYNC_TYPE,
                "ResourceName": name,
                "ResolvedReason": "resolved by the test",
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404 from UpdateSyncBlocker, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "SyncBlockerDoesNotExistException",
            "case {name:?}: expected SyncBlockerDoesNotExistException, got {body}"
        );
    }

    // The blocker id is resolved BEFORE the resource: the live resource, which DOES
    // have a sync configuration, answers the same way for an unknown blocker id.
    for blocker_id in ["blocker-1", "b", &"x".repeat(50)] {
        let (status, body) = call(
            &router,
            "UpdateSyncBlocker",
            serde_json::json!({
                "Id": blocker_id,
                "SyncType": SYNC_TYPE,
                "ResourceName": "LiveResource",
                "ResolvedReason": "resolved by the test",
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {blocker_id:?}: expected 404 from UpdateSyncBlocker, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "SyncBlockerDoesNotExistException",
            "case {blocker_id:?}: expected SyncBlockerDoesNotExistException, got {body}"
        );
    }

    // And a repository link that IS there still reports no sync attempt: nothing in
    // this emulator performs a Git sync, so the modeled ResourceNotFoundException (404)
    // is the answer — never a synthesised status for work nothing ran.
    for branch in ["main", "release/1.0", "b"] {
        let (status, body) = call(
            &router,
            "GetRepositorySyncStatus",
            serde_json::json!({
                "Branch": branch,
                "RepositoryLinkId": live_link,
                "SyncType": SYNC_TYPE,
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {branch:?}: expected 404 from GetRepositorySyncStatus, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {branch:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            body["LatestSync"].is_null(),
            "case {branch:?}: no sync attempt may be reported, got {body}"
        );
    }
}

/// `DeleteSyncConfiguration` models no not-found error, so deleting a
/// `(SyncType, ResourceName)` pair that holds nothing succeeds and removes nothing:
/// the delete is idempotent, and `GetSyncConfiguration` reports 404 both before the
/// create and after the delete.
#[tokio::test]
async fn deleting_a_sync_configuration_is_idempotent() {
    let router = router();

    for (index, case) in sync_cases().iter().enumerate() {
        let delete = serde_json::json!({
            "SyncType": SYNC_TYPE,
            "ResourceName": case.resource_name,
        });
        let get = delete.clone();

        // Before anything is created: the read is a 404 and the delete is a success.
        let (status, body) = call(&router, "GetSyncConfiguration", get.clone()).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 before the create, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {:?}: expected ResourceNotFoundException before the create, got {body}",
            case.label
        );
        let (status, body) = call(&router, "DeleteSyncConfiguration", delete.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: deleting what is not there is a success, got {status} {body}",
            case.label
        );

        // Create it, delete it, and the read is a 404 again.
        let link_case = LinkCase {
            label: "delete row",
            provider_type: PROVIDER_TYPES[index % PROVIDER_TYPES.len()],
            owner_id: format!("owner-{index}"),
            repository_name: format!("repo-{index}"),
            encryption_key_arn: None,
            tags: Vec::new(),
        };
        let (link_id, _) = create_link(&router, &link_case, &format!("conn-{index}")).await;
        let (status, body) = call(
            &router,
            "CreateSyncConfiguration",
            create_sync_body(case, &link_id),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from CreateSyncConfiguration, got {status} {body}",
            case.label
        );
        let (status, body) = call(&router, "GetSyncConfiguration", get.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetSyncConfiguration, got {status} {body}",
            case.label
        );

        for attempt in 1..=3 {
            let (status, body) = call(&router, "DeleteSyncConfiguration", delete.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?}: delete attempt {attempt} must succeed, got {status} {body}",
                case.label
            );
            let (status, body) = call(&router, "GetSyncConfiguration", get.clone()).await;
            assert_eq!(
                status, 404,
                "case {:?}: expected 404 after delete attempt {attempt}, got {status} {body}",
                case.label
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {:?}: expected ResourceNotFoundException after delete attempt {attempt}, got {body}",
                case.label
            );
        }

        // The repository link the configuration named is untouched by its delete.
        let (status, body) = call(
            &router,
            "GetRepositoryLink",
            serde_json::json!({ "RepositoryLinkId": link_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: deleting a configuration must not remove its link, got {status} {body}",
            case.label
        );
    }
}

/// One sync blocker a test seeds into state, and the reason it then resolves it with.
///
/// A blocker is minted only by the SERVICE, when a Git sync it performed is blocked, so
/// a test that drives `UpdateSyncBlocker`'s resolve path seeds the row the same way the
/// service would.
struct BlockerCase {
    id: String,
    resource_name: &'static str,
    parent_resource_name: Option<&'static str>,
    /// `SyncBlockerContext` entries as `(Key, Value)`.
    contexts: Vec<(&'static str, &'static str)>,
    resolved_reason: String,
}

/// The table: blockers differing in id length (each published bound of `Id` 1..50),
/// resolved-reason length (each published bound of `ResolvedReason` 1..250), whether
/// they carry contexts and whether they have a parent resource.
fn blocker_cases() -> Vec<BlockerCase> {
    vec![
        BlockerCase {
            id: "b".to_string(),
            resource_name: "StackOne",
            parent_resource_name: None,
            contexts: Vec::new(),
            resolved_reason: "r".to_string(),
        },
        BlockerCase {
            id: "blocker-1".to_string(),
            resource_name: "StackTwo",
            parent_resource_name: Some("ParentStack"),
            contexts: vec![("TargetId", "resource-1")],
            resolved_reason: "resolved by hand".to_string(),
        },
        BlockerCase {
            id: "x".repeat(50),
            resource_name: "StackThree",
            parent_resource_name: Some("ParentStack"),
            contexts: vec![("TargetId", "resource-2"), ("Reason", "drift")],
            resolved_reason: "r".repeat(250),
        },
    ]
}

/// `UpdateSyncBlocker` resolves the blocker its id names: a blocker the store holds
/// moves to `RESOLVED` with the reason the caller gave and the instant it happened,
/// and a later read of state shows the same row. Rows differ in blocker id length
/// (each published bound of `Id` 1..50), reason length (`ResolvedReason` 1..250),
/// whether the blocker carries contexts and whether it has a parent resource.
#[tokio::test]
async fn resolving_a_sync_blocker_records_the_reason_and_the_instant() {
    let (router, engine) = router_with_engine();

    let cases = blocker_cases();

    let created_at = ::aws_smithy_types::DateTime::from_secs(1_700_000_000);
    for case in &cases {
        engine
            .state::<ComAmazonawsCodeconnectionsCodeconnections20231201State>()
            .lock()
            .sync_blockers
            .insert(
                case.id.clone(),
                StoredSyncBlocker {
                    id: case.id.clone(),
                    sync_type: SYNC_TYPE.to_string(),
                    resource_name: case.resource_name.to_string(),
                    parent_resource_name: case.parent_resource_name.map(str::to_string),
                    blocker_type: "AUTOMATED".to_string(),
                    status: "ACTIVE".to_string(),
                    created_reason: "a sync was blocked".to_string(),
                    created_at,
                    contexts: case
                        .contexts
                        .iter()
                        .map(|(key, value)| SyncBlockerContext {
                            key: (*key).to_string(),
                            value: (*value).to_string(),
                        })
                        .collect(),
                    resolved_reason: None,
                    resolved_at: None,
                },
            );
    }

    for case in &cases {
        let BlockerCase {
            id,
            resource_name,
            parent_resource_name: parent,
            contexts,
            resolved_reason: reason,
        } = case;
        // A blocker that exists, but under ANOTHER resource, is not a blocker of the
        // resource this request names.
        let (status, body) = call(
            &router,
            "UpdateSyncBlocker",
            serde_json::json!({
                "Id": id,
                "SyncType": SYNC_TYPE,
                "ResourceName": "SomeOtherResource",
                "ResolvedReason": reason,
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {id:?}: a blocker of another resource must not resolve, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "SyncBlockerDoesNotExistException",
            "case {id:?}: expected SyncBlockerDoesNotExistException, got {body}"
        );

        let (status, body) = call(
            &router,
            "UpdateSyncBlocker",
            serde_json::json!({
                "Id": id,
                "SyncType": SYNC_TYPE,
                "ResourceName": resource_name,
                "ResolvedReason": reason,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected 200 from UpdateSyncBlocker, got {status} {body}"
        );
        assert_eq!(
            body["ResourceName"],
            serde_json::json!(resource_name),
            "case {id:?}: expected the blocker's resource name, got {body}"
        );
        match parent {
            Some(parent) => assert_eq!(
                body["ParentResourceName"],
                serde_json::json!(parent),
                "case {id:?}: expected the stored parent resource, got {body}"
            ),
            None => assert!(
                body["ParentResourceName"].is_null(),
                "case {id:?}: a blocker with no parent reports none, got {body}"
            ),
        }
        let blocker = &body["SyncBlocker"];
        assert_eq!(
            blocker["Id"],
            serde_json::json!(id),
            "case {id:?}: expected the blocker's id, got {body}"
        );
        assert_eq!(
            blocker["Status"],
            serde_json::json!("RESOLVED"),
            "case {id:?}: a resolved blocker is RESOLVED, got {body}"
        );
        assert_eq!(
            blocker["ResolvedReason"],
            serde_json::json!(reason),
            "case {id:?}: expected the reason the caller gave, got {body}"
        );
        assert_eq!(
            blocker["Type"],
            serde_json::json!("AUTOMATED"),
            "case {id:?}: expected the stored blocker type, got {body}"
        );
        assert_eq!(
            blocker["CreatedReason"],
            serde_json::json!("a sync was blocked"),
            "case {id:?}: expected the stored created reason, got {body}"
        );
        assert!(
            !blocker["ResolvedAt"].is_null(),
            "case {id:?}: a resolved blocker records when, got {body}"
        );
        if contexts.is_empty() {
            assert!(
                blocker["Contexts"].is_null(),
                "case {id:?}: a blocker with no contexts reports none, got {body}"
            );
        } else {
            assert_eq!(
                blocker["Contexts"],
                serde_json::Value::Array(
                    contexts
                        .iter()
                        .map(|(key, value)| serde_json::json!({ "Key": key, "Value": value }))
                        .collect()
                ),
                "case {id:?}: expected the stored contexts, got {body}"
            );
        }

        // The resolution was PERSISTED: the stored row carries it.
        let stored = engine
            .state::<ComAmazonawsCodeconnectionsCodeconnections20231201State>()
            .lock()
            .sync_blockers
            .get(id)
            .cloned()
            .unwrap_or_else(|| panic!("case {id:?}: the blocker must still be stored"));
        assert_eq!(
            stored.status, "RESOLVED",
            "case {id:?}: expected the stored blocker to be RESOLVED, got {:?}",
            stored.status
        );
        assert_eq!(
            stored.resolved_reason.as_deref(),
            Some(reason.as_str()),
            "case {id:?}: expected the stored reason, got {:?}",
            stored.resolved_reason
        );
        assert!(
            stored.resolved_at.is_some(),
            "case {id:?}: expected a stored resolution instant, got {:?}",
            stored.resolved_at
        );
    }
}
