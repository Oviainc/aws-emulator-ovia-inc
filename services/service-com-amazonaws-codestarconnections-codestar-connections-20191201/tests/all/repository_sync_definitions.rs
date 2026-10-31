// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRepositorySyncDefinitions`.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) exactly as the
//! emulator's front door does it (`POST /` + `X-Amz-Target:
//! CodeStar_connections_20191201.ListRepositorySyncDefinitions`, awsJson1_0 body), so what
//! is asserted here is the service's HTTP behaviour and not a handler called directly.
//!
//! RULE — the operation reports one repository sync definition per sync configuration this
//! service HOLDS for the `(SyncType, RepositoryLinkId)` pair the request names, in the
//! order those configurations were registered, without changing any of them: a
//! definition's `Branch` and `Directory` are the configuration's `Branch` and
//! `ConfigFile` (`RepositorySyncDefinition$Directory` is modelled "The configuration file
//! for a repository sync definition. This value comes from creating or updating the
//! `config-file` field of a `sync-configuration`"), and its `Parent` and `Target` are both
//! the configuration's `ResourceName`, because `Target` is modelled "In some cases, such as
//! CFN_STACK_SYNC, the parent and target resource are the same" and `CFN_STACK_SYNC` is the
//! only value `SyncConfigurationType` admits
//! (`smithy_query(query="shape", shape="RepositorySyncDefinition")`,
//! `smithy_query(query="shape", shape="SyncConfigurationType")`). A pair holding no
//! configuration reports an EMPTY list — the output's one `@required` member is a list, so
//! "this link syncs nothing" is `[]` — while an id naming no link at all is refused as
//! INVALID INPUT: the recorded wire of this endpoint answers a well-formed
//! `RepositoryLinkId` naming no link with `InvalidInputException` and the sentence that
//! names that id (corpus `simparity-behavioral-aws-v1`, unit
//! `codestar-connections.list-repository-sync-definitions-empty-without-stack`), where the
//! link's own readers answer `ResourceNotFoundException` wrapped in the Git-sync tail.
//!
//! The operation is not `@paginated`: its input binds neither `NextToken` nor `MaxResults`
//! (`smithy_query(query="operation", operation="ListRepositorySyncDefinitions")`), so every
//! definition the pair holds is reported in one answer and no cursor is emitted.
//!
//! A sync configuration is registered by `CreateSyncConfiguration`, which is still a
//! scaffold stub (its own issue,
//! `issues/things-that-do-not-work/20261029T181302Z-codestar-connections-createsyncconfiguration-stub.md`),
//! so the configurations listed here are put in the store directly, through the SAME map
//! the handler reads (`codestar::state::sync_configurations`) — which is what makes these
//! assertions evidence that the response is computed from persisted state rather than from
//! a constant.
//!
//! The tables span what the rule generalises over: how many configurations a link holds
//! (none, one, several), every modelled `SyncConfigurationType`, the published `BranchName`
//! and `ResourceName` length bounds and the forms their patterns admit, the config-file
//! path forms, and which link and sync type a configuration was registered against. The
//! recorded request (a link that syncs no stack) is ONE row.

use serde_json::{json, Value};

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use codestar::state::{
    resource_sync_key, ComAmazonawsCodestarconnectionsCodestarConnections20191201State as CodestarState,
    StoredSyncConfiguration,
};

// The router harness this crate's suites share (one emulator, one engine, awsJson
// dispatch), the two readers of a wire answer and the link-creation path they already
// state, rather than a second copy of any of them.
use super::connection_lifecycle::{error_code, error_message, Emulator, ACCOUNT_ID};
use super::repository_sync_status::create_link;

/// Every value the model's `SyncConfigurationType` enum accepts.
///
/// Referenced rather than copied: this is the generated catalog
/// (`src/ops/types/shared_types.rs::SYNC_CONFIGURATION_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="SyncConfigurationType")`), so the tables below widen
/// with the model instead of drifting from it.
const SYNC_TYPES: &[&str] = codestar::ops::types::shared_types::SYNC_CONFIGURATION_TYPE_VALUES;

/// The `SyncType` the single-type tables address.
const SYNC_TYPE: &str = SYNC_TYPES[0];

/// The published bounds of the modelled `Branch` member (`length 1..255`, `pattern ^.*$`),
/// from `smithy_query(query="operation", operation="CreateSyncConfiguration")`.
const BRANCH_MIN_LEN: usize = 1;
const BRANCH_MAX_LEN: usize = 255;

/// The published bounds of the modelled `ResourceName` member (`length 1..100`, `pattern
/// ^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`), from
/// `smithy_query(query="shape", shape="ResourceName")`.
const RESOURCE_NAME_MIN_LEN: usize = 1;
const RESOURCE_NAME_MAX_LEN: usize = 100;

/// One row of the registered-configuration table: a sync configuration, and the definition
/// it must be reported as.
struct Row {
    /// What a failing assertion names this row by.
    label: String,
    branch: String,
    /// The `config-file` of the configuration — the definition's `Directory`.
    config_file: String,
    /// The synced resource — the definition's `Parent` and `Target`.
    resource_name: String,
}

impl Row {
    /// This row as the record the store holds, registered against `repository_link_id`
    /// under `sync_type` at creation order `sequence`.
    fn to_stored(
        &self,
        sync_type: &str,
        repository_link_id: &str,
        sequence: u64,
    ) -> StoredSyncConfiguration {
        StoredSyncConfiguration {
            branch: self.branch.clone(),
            config_file: self.config_file.clone(),
            owner_id: format!("owner-{sequence}"),
            provider_type: "GitHub".to_string(),
            repository_link_id: repository_link_id.to_string(),
            repository_name: format!("repo-{sequence}"),
            resource_name: self.resource_name.clone(),
            role_arn: format!("arn:aws:iam::{ACCOUNT_ID}:role/git-sync-{sequence}"),
            sync_type: sync_type.to_string(),
            publish_deployment_status: None,
            trigger_resource_update_on: None,
            sequence,
        }
    }
}

/// The registered-configuration table: the branch forms the published `Branch` pattern
/// (`^.*$`, i.e. any text) admits crossed with the config-file path forms and the
/// `ResourceName` forms its published pattern admits, including each published length
/// bound. None of these is the link the issue's recording cited, which syncs nothing at
/// all.
fn rows() -> Vec<Row> {
    vec![
        (
            "a trunk branch with a config file in a dot directory",
            "main".to_string(),
            ".aws/deployment.yaml".to_string(),
            "appstack".to_string(),
        ),
        (
            "a branch and a resource name at their published minimum length",
            "b".repeat(BRANCH_MIN_LEN),
            "deployment.yaml".to_string(),
            "r".repeat(RESOURCE_NAME_MIN_LEN),
        ),
        (
            "a branch and a resource name at their published maximum length",
            "b".repeat(BRANCH_MAX_LEN),
            "deep/nested/path/deployment.json".to_string(),
            "r".repeat(RESOURCE_NAME_MAX_LEN),
        ),
        (
            "a namespaced ref and a resource name carrying an underscore",
            "feature/add-git-sync".to_string(),
            "cfn/stacks/deployment.yml".to_string(),
            "Stack_With_Underscore".to_string(),
        ),
        (
            "a ref carrying dots and digits, and a hyphenated resource name",
            "release/1.0.0".to_string(),
            "deployment.yaml".to_string(),
            "stack-9-with-hyphen".to_string(),
        ),
        (
            "a digit-only ref and a config file at the repository root",
            "2026".to_string(),
            "d".to_string(),
            "0stack".to_string(),
        ),
    ]
    .into_iter()
    .map(|(label, branch, config_file, resource_name)| Row {
        label: label.to_string(),
        branch,
        config_file,
        resource_name,
    })
    .collect()
}

/// `ListRepositorySyncDefinitions` for one `(SyncType, RepositoryLinkId)` pair: the status
/// and the parsed body.
async fn list_definitions(
    emulator: &Emulator,
    sync_type: &str,
    repository_link_id: &str,
) -> (u16, Value) {
    emulator
        .call(
            "ListRepositorySyncDefinitions",
            json!({
                "SyncType": sync_type,
                "RepositoryLinkId": repository_link_id,
            }),
        )
        .await
}

/// Register one sync configuration in the store — what `CreateSyncConfiguration` does, and
/// what this reader must report.
fn seed_configuration(emulator: &Emulator, configuration: StoredSyncConfiguration) {
    emulator
        .engine()
        .state::<CodestarState>()
        .lock()
        .sync_configurations
        .insert(
            resource_sync_key(&configuration.sync_type, &configuration.resource_name),
            configuration,
        );
}

/// Every configuration the store holds, as `(key, Branch, ConfigFile, RepositoryLinkId)` —
/// enough to tell that a read changed none of them.
fn stored_configurations(emulator: &Emulator) -> Vec<((String, String), String, String, String)> {
    emulator
        .engine()
        .state::<CodestarState>()
        .lock()
        .sync_configurations
        .iter()
        .map(|(key, configuration)| {
            (
                key.clone(),
                configuration.branch.clone(),
                configuration.config_file.clone(),
                configuration.repository_link_id.clone(),
            )
        })
        .collect()
}

/// The definitions a successful answer reports, asserting the answer's shape first.
fn definitions_of(case: &str, status: u16, body: &Value) -> Vec<Value> {
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListRepositorySyncDefinitions to answer 200, got {status} \
         {body:?}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {case:?}: the operation binds no input cursor, so no NextToken may be reported, \
         got {body:?}"
    );
    body["RepositorySyncDefinitions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: RepositorySyncDefinitions must be a list, got {body:?}")
        })
        .clone()
}

/// Assert one reported definition is exactly the stored configuration's projection.
fn assert_reports_definition(case: &str, reported: &Value, stored: &StoredSyncConfiguration) {
    for (member, expected) in [
        ("Branch", stored.branch.as_str()),
        ("Directory", stored.config_file.as_str()),
        ("Parent", stored.resource_name.as_str()),
        ("Target", stored.resource_name.as_str()),
    ] {
        let actual = reported[member].as_str();
        assert_eq!(
            actual,
            Some(expected),
            "case {case:?}: {member} must be the stored {expected:?}, got {actual:?} in \
             {reported:?}"
        );
    }
}

/// Assert one answer is this endpoint's refusal for an id naming no link: invalid input,
/// carrying the recorded sentence that names the id and nothing else.
fn assert_no_such_link(case: &str, repository_link_id: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the recorded client-error status for an id naming no link, got \
         {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException, got {body:?}"
    );
    let message = error_message(body);
    // The recorded wording, as a LITERAL: the point of asserting it is to pin what a client
    // reads, so it is not read back out of the constant the handler writes. It names the id
    // that resolved to nothing and carries none of the Git-sync tail the link readers' own
    // refusals carry.
    let expected = format!("Repository Link with id {repository_link_id} was not found");
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal message {expected:?}, got {message:?}"
    );
}

/// Every configuration a link holds is reported as its definition, member for member and in
/// registration order, for every modelled sync type.
#[tokio::test]
async fn every_sync_configuration_of_a_link_is_reported_as_its_definition() {
    for sync_type in SYNC_TYPES {
        let emulator = Emulator::new();
        let link_id = create_link(&emulator, "the synced link", "definitions-repo").await;

        let rows = rows();
        let stored: Vec<StoredSyncConfiguration> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| row.to_stored(sync_type, &link_id, index as u64))
            .collect();
        for configuration in &stored {
            seed_configuration(&emulator, configuration.clone());
        }

        let case = format!("{} configurations under {sync_type}", stored.len());
        let (status, body) = list_definitions(&emulator, sync_type, &link_id).await;
        let reported = definitions_of(&case, status, &body);
        assert_eq!(
            reported.len(),
            stored.len(),
            "case {case:?}: expected one definition per stored configuration, got {body:?}"
        );
        for (reported, configuration) in reported.iter().zip(&stored) {
            let row_case = format!("{} under {sync_type}", configuration.resource_name);
            assert_reports_definition(&row_case, reported, configuration);
            assert_eq!(
                reported["Parent"], reported["Target"],
                "case {row_case:?}: for {sync_type} the parent and the target are the same \
                 resource, got {reported:?}"
            );
        }

        // The order is the order the configurations were registered in, so paging a link's
        // definitions is reproducible.
        let branches: Vec<&str> = reported
            .iter()
            .map(|definition| definition["Branch"].as_str().unwrap_or_default())
            .collect();
        let expected: Vec<&str> = stored
            .iter()
            .map(|configuration| configuration.branch.as_str())
            .collect();
        assert_eq!(
            branches, expected,
            "case {case:?}: the definitions must be reported in registration order, expected \
             {expected:?}, got {branches:?}"
        );
    }
}

/// A definition belongs to the link and the sync type its configuration was registered
/// against, and to no other pair.
#[tokio::test]
async fn a_definition_belongs_only_to_the_pair_it_was_registered_against() {
    let emulator = Emulator::new();
    let link_id = create_link(&emulator, "the link under test", "owning-repo").await;
    let other_link_id = create_link(&emulator, "a second link", "other-repo").await;

    for (index, row) in rows().into_iter().enumerate() {
        for (seeded_index, seeded_type) in SYNC_TYPES.iter().enumerate() {
            let sequence = (index * SYNC_TYPES.len() + seeded_index) as u64;
            // One configuration, on ONE link under ONE sync type; every other pair must
            // report nothing for it.
            let mut configuration = row.to_stored(seeded_type, &link_id, sequence);
            configuration.resource_name = format!("stack{sequence}");
            seed_configuration(&emulator, configuration.clone());

            for requested_type in SYNC_TYPES {
                let case = format!(
                    "{} registered under {seeded_type}, requested under {requested_type}",
                    row.label
                );
                let (status, body) = list_definitions(&emulator, requested_type, &link_id).await;
                let reported = definitions_of(&case, status, &body);
                let targets: Vec<&str> = reported
                    .iter()
                    .map(|definition| definition["Target"].as_str().unwrap_or_default())
                    .collect();
                let holds = targets.contains(&configuration.resource_name.as_str());
                assert_eq!(
                    holds,
                    requested_type == seeded_type,
                    "case {case:?}: the definition must be reported only under the sync type \
                     its configuration was registered with, got {targets:?}"
                );

                // And never under another link.
                let (status, body) =
                    list_definitions(&emulator, requested_type, &other_link_id).await;
                let reported = definitions_of(&format!("other link of {case}"), status, &body);
                assert!(
                    reported.is_empty(),
                    "case {case:?}: a link holding no configuration reports no definition, got \
                     {reported:?}"
                );
            }
        }
    }
}

/// A link that syncs nothing reports an EMPTY list — a 200 with no definitions, never the
/// scaffold's 501 and never a synthesised row — for every modelled sync type, even while
/// another link of the same account holds configurations.
#[tokio::test]
async fn a_link_that_syncs_nothing_reports_an_empty_list() {
    let emulator = Emulator::new();
    let synced = create_link(&emulator, "a link that syncs a stack", "synced-repo").await;
    for (index, row) in rows().into_iter().enumerate() {
        seed_configuration(&emulator, row.to_stored(SYNC_TYPE, &synced, index as u64));
    }

    // Links differing in how they were registered, none of them configured to sync
    // anything: the recorded request is this class.
    let unsynced = [
        create_link(&emulator, "a freshly created link", "unsynced-repo").await,
        create_link(&emulator, "a second unsynced link", "quiet-repo").await,
    ];

    for sync_type in SYNC_TYPES {
        for link_id in &unsynced {
            let case = format!("link {link_id} under {sync_type}");
            let (status, body) = list_definitions(&emulator, sync_type, link_id).await;
            assert_ne!(
                status, 501,
                "case {case:?}: the operation must not answer the scaffold's 501, got {body:?}"
            );
            let reported = definitions_of(&case, status, &body);
            assert!(
                reported.is_empty(),
                "case {case:?}: a link that syncs nothing reports no definition, got {reported:?}"
            );
        }
    }
}

/// An id naming no repository link is refused as invalid input — never as an empty success,
/// and never with the `ResourceNotFoundException` the link's own readers answer — including
/// an id whose link a delete has just freed.
#[tokio::test]
async fn an_id_naming_no_repository_link_is_refused() {
    let emulator = Emulator::new();
    let link_id = create_link(&emulator, "the link under test", "deleted-repo").await;
    // A link that DOES sync something, so a refusal can never mean "the store is empty".
    seed_configuration(&emulator, rows()[0].to_stored(SYNC_TYPE, &link_id, 0));

    // Ids of the modelled `RepositoryLinkId` form that name no link this service holds.
    for (label, id) in [
        (
            "a well-formed id never minted here",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
        (
            "an upper-case id the pattern admits",
            "AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE".to_string(),
        ),
        (
            "the all-zero id a client reaches for when it has none",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
    ] {
        let (status, body) = list_definitions(&emulator, SYNC_TYPE, &id).await;
        assert_no_such_link(label, &id, status, &body);

        // The link's own reader reports the same lookup miss under its own code, so the
        // difference asserted above is this operation's code and not a different failure.
        let (status, read) = emulator
            .call("GetRepositoryLink", json!({ "RepositoryLinkId": id }))
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected GetRepositoryLink to refuse the same id with 400, got \
             {status} {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {label:?}: GetRepositoryLink words an absent link as not found, got {read:?}"
        );
    }

    // A link of its own, holding no configuration, goes away: its id then resolves to
    // nothing and is refused exactly as an id never minted here is.
    let doomed = create_link(&emulator, "a link about to be deleted", "doomed-repo").await;
    let (status, deleted) = emulator
        .call("DeleteRepositoryLink", json!({ "RepositoryLinkId": doomed }))
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteRepositoryLink to answer 200, got {status} {deleted:?}"
    );
    let (status, body) = list_definitions(&emulator, SYNC_TYPE, &doomed).await;
    assert_no_such_link("an id a delete has freed", &doomed, status, &body);
}

/// The report is a read: it changes nothing a later caller sees. Repeated reads answer
/// identically, and the configurations the store holds after the reads are exactly the ones
/// that were registered — a read records none and a refused read records none either.
#[tokio::test]
async fn listing_the_definitions_leaves_the_store_as_it_found_it() {
    let emulator = Emulator::new();
    let link_id = create_link(&emulator, "the link under test", "read-only-repo").await;
    let unsynced = create_link(&emulator, "a link that syncs nothing", "idle-repo").await;
    for (index, row) in rows().into_iter().enumerate() {
        seed_configuration(&emulator, row.to_stored(SYNC_TYPE, &link_id, index as u64));
    }
    let before = stored_configurations(&emulator);

    for sync_type in SYNC_TYPES {
        for (case, target) in [
            ("the configured link", link_id.as_str()),
            ("a link that syncs nothing", unsynced.as_str()),
        ] {
            let case = format!("{case} under {sync_type}");
            let mut answers: Vec<(u16, Value)> = Vec::new();
            for _ in 0..3 {
                answers.push(list_definitions(&emulator, sync_type, target).await);
            }
            let first = &answers[0];
            for (read, answer) in answers.iter().enumerate() {
                assert_eq!(
                    answer, first,
                    "case {case:?}: read {read} must answer exactly as the first did, got \
                     {answer:?}"
                );
            }
        }

        // And a refused read, of an id naming no link at all.
        let absent = "99999999-8888-4777-8666-555555555555";
        let (status, body) = list_definitions(&emulator, sync_type, absent).await;
        assert_no_such_link(&format!("an absent id under {sync_type}"), absent, status, &body);
    }

    let after = stored_configurations(&emulator);
    assert_eq!(
        after, before,
        "the reads must leave the stored configurations as they were, expected {before:?}, got \
         {after:?}"
    );
}

/// The boundary of the modelled input constraints: an id breaking the published
/// `RepositoryLinkId` pattern, an unmodelled `SyncType` and a request missing a required
/// member are refused with HTTP 400 `ValidationException` before the handler runs — so the
/// empty list and the `ResourceNotFoundException` above are the handler's answers for a
/// well-formed pair, not a blanket refusal.
#[tokio::test]
async fn input_outside_the_modeled_constraints_is_refused() {
    let emulator = Emulator::new();
    let link_id = create_link(&emulator, "the link under test", "bounds-repo").await;

    for (label, body) in [
        (
            "an id that is not of the published UUID form",
            json!({ "SyncType": SYNC_TYPE, "RepositoryLinkId": "not-a-uuid" }),
        ),
        (
            "a SyncType the model does not admit",
            json!({ "SyncType": "NOT_A_SYNC_TYPE", "RepositoryLinkId": link_id }),
        ),
        (
            "a request naming no sync type at all",
            json!({ "RepositoryLinkId": link_id }),
        ),
        (
            "a request naming no repository link at all",
            json!({ "SyncType": SYNC_TYPE }),
        ),
    ] {
        let (status, response) = emulator.call("ListRepositorySyncDefinitions", body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected ListRepositorySyncDefinitions to refuse the input with \
             400, got {status} {response:?}"
        );
        assert!(
            error_code(&response).ends_with("ValidationException"),
            "case {label:?}: expected a ValidationException, got {response:?}"
        );
    }
}
