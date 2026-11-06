// SPDX-License-Identifier: CC0-1.0

//! The refusals this service AUTHORS for a request it will not serve, end to end
//! over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The rule the tables assert: a request Amazon Redshift refuses on its
//! PARAMETERS carries the query-protocol error code the service itself answers —
//! `InvalidParameterValue` for a value it cannot honour, and
//! `InvalidParameterCombination` for members that cannot stand together — and
//! never a code outside that contract. Four consequences are covered here, each
//! over a table whose rows span what the rule generalises over:
//!
//! * the shared pagination validation: `MaxRecords` is refused only BELOW the
//!   published minimum, and a page past the published maximum is served — at
//!   that maximum, with a `Marker` for the rest;
//! * `DeleteCluster`'s final-snapshot decision, which is a parameter
//!   combination with a fixed sentence and leaves the cluster alone;
//! * the parameter-group domains — a name the family does not publish, a value
//!   the published set does not admit, and the service-owned default group,
//!   which no caller may change or delete;
//! * the resource-policy operations, which resolve the `ResourceArn` against the
//!   control plane that owns the resource BEFORE any policy state is read or
//!   written, so no document is attached to a resource that does not exist.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator, REGION};
use crate::cluster_parameters::{create_group, default_family, default_group_name};
use crate::modify_cluster_parameter_group::{modify, reported};

/// Assert one refusal row: the answer's status and `<Code>`.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected the error code {code}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Assert one row was served.
fn assert_served(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected status 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// The paginated reads this suite drives `MaxRecords` through: every one of them
/// answers on a fresh account, so the rows differ along the page size alone.
const PAGINATED_READS: [&str; 6] = [
    "DescribeClusters",
    "DescribeClusterParameterGroups",
    "DescribeClusterSnapshots",
    "DescribeClusterVersions",
    "DescribeClusterTracks",
    "DescribeEvents",
];

/// `MaxRecords` is enforced at the published MINIMUM only: a smaller page is
/// refused with `InvalidParameterValue`, and a page larger than the published
/// maximum is served.
///
/// Source of the window: `smithy_query(query="doc",
/// shape="DescribeClustersMessage", member="MaxRecords")` — "Default: 100 …
/// Constraints: minimum 20, maximum 100" (`state::MAX_RECORDS_MIN` /
/// `state::MAX_RECORDS_MAX`). The maximum is what a paginating client is told to
/// stay inside, not a bound the service turns a larger request away at, so a
/// page past it is a call the service serves.
#[tokio::test]
async fn max_records_is_enforced_only_below_the_published_minimum() {
    let emulator = Emulator::new();

    // Each row is a `MaxRecords` and whether the service serves it: both
    // published bounds, one past each, far past the maximum, zero and a
    // negative.
    let rows: Vec<(&str, String, bool)> = vec![
        ("zero", "0".to_string(), false),
        ("negative", "-1".to_string(), false),
        (
            "one below the published minimum",
            (state::MAX_RECORDS_MIN - 1).to_string(),
            false,
        ),
        (
            "at the published minimum",
            state::MAX_RECORDS_MIN.to_string(),
            true,
        ),
        (
            "inside the window",
            ((state::MAX_RECORDS_MIN + state::MAX_RECORDS_MAX) / 2).to_string(),
            true,
        ),
        (
            "at the published maximum",
            state::MAX_RECORDS_MAX.to_string(),
            true,
        ),
        (
            "one past the published maximum",
            (state::MAX_RECORDS_MAX + 1).to_string(),
            true,
        ),
        (
            "far past the published maximum",
            (state::MAX_RECORDS_MAX * 100).to_string(),
            true,
        ),
    ];

    for action in PAGINATED_READS {
        for (row, max_records, served) in &rows {
            let case = format!("{action} with MaxRecords {row} ({max_records})");
            let answer = emulator
                .call(action, &[("MaxRecords", max_records.as_str())])
                .await;
            if *served {
                assert_served(&case, &answer);
                continue;
            }
            assert_refused(&case, &answer, 400, "InvalidParameterValue");
            let message = answer.text("Message").unwrap_or_default();
            assert!(
                message.contains("MaxRecords")
                    && message.contains(&state::MAX_RECORDS_MIN.to_string()),
                "case {case}: expected the refusal to name MaxRecords and the minimum \
                 {}, got {message:?}",
                state::MAX_RECORDS_MIN
            );
        }
    }

    // A `Marker` the service never handed out is the same class of refusal: a
    // pagination value it will not serve.
    for cursor in ["not-a-cursor", "7", "20.deadbeef"] {
        let case = format!("a forged Marker {cursor:?}");
        let answer = emulator.call("DescribeClusters", &[("Marker", cursor)]).await;
        assert_refused(&case, &answer, 400, "InvalidParameterValue");
    }
}

/// A page the caller asks for past the published maximum is served, and served
/// AT that maximum: the published window is the most records one page carries,
/// so the rest follow through the `Marker` the page hands back.
#[tokio::test]
async fn a_page_past_the_published_maximum_is_served_at_the_maximum() {
    let emulator = Emulator::new();
    let family = default_family();

    // More parameter groups than one page carries, so a clamped page leaves
    // records behind it.
    let seeded = state::MAX_RECORDS_MAX as usize + 5;
    for index in 0..seeded {
        let name = format!("paged-group-{index:03}");
        let created = emulator
            .call(
                "CreateClusterParameterGroup",
                &[
                    ("ParameterGroupName", name.as_str()),
                    ("ParameterGroupFamily", family.as_str()),
                    ("Description", "a paged group"),
                ],
            )
            .await;
        assert_served(&format!("seed {name}"), &created);
    }

    for (case, max_records) in [
        ("one past the published maximum", state::MAX_RECORDS_MAX + 1),
        ("far past the published maximum", state::MAX_RECORDS_MAX * 100),
        ("the largest page a caller can name", i32::MAX),
    ] {
        let value = max_records.to_string();
        let answer = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("MaxRecords", value.as_str())],
            )
            .await;
        assert_served(case, &answer);
        let page = all_text(&answer.body, "ParameterGroupName");
        assert_eq!(
            page.len(),
            state::MAX_RECORDS_MAX as usize,
            "case {case}: expected the page to carry the window's maximum of {} groups, got {}",
            state::MAX_RECORDS_MAX,
            page.len()
        );
        assert!(
            answer.text("Marker").is_some(),
            "case {case}: expected a Marker for the records the clamped page left, got body {}",
            answer.body
        );
    }
}

/// One row of the final-snapshot decision table: the members the delete carries
/// beside `ClusterIdentifier`, and the refusal it earns (`None` when the service
/// serves it).
struct DecisionCase {
    case: &'static str,
    members: Vec<(&'static str, &'static str)>,
    refusal: Option<&'static str>,
}

/// `DeleteCluster` decides on the final-snapshot members BEFORE it touches the
/// cluster: a request that carries neither half of the decision, and one whose
/// halves contradict each other, are refused with `InvalidParameterCombination`
/// and the cluster stays available; a request that carries one half is served and
/// reports the cluster `deleting`.
///
/// Source: `smithy_query(query="doc", shape="DeleteClusterMessage",
/// member="SkipFinalClusterSnapshot")` — "The FinalClusterSnapshotIdentifier
/// parameter must be specified if SkipFinalClusterSnapshot is false. Default:
/// false". The operation models no error for it, so the refusal is the service's
/// own parameter-combination code, with the fixed sentence
/// `state::FINAL_SNAPSHOT_DECISION_REQUIRED` for the request that names neither.
#[tokio::test]
async fn a_delete_without_a_final_snapshot_decision_is_a_parameter_combination() {
    let emulator = Emulator::new();

    let rows: Vec<DecisionCase> = vec![
        DecisionCase {
            case: "neither half of the decision",
            members: Vec::new(),
            refusal: Some("InvalidParameterCombination"),
        },
        DecisionCase {
            case: "the skip spelled false and no snapshot named",
            members: vec![("SkipFinalClusterSnapshot", "false")],
            refusal: Some("InvalidParameterCombination"),
        },
        DecisionCase {
            case: "both halves against each other",
            members: vec![
                ("SkipFinalClusterSnapshot", "true"),
                ("FinalClusterSnapshotIdentifier", "final-snap"),
            ],
            refusal: Some("InvalidParameterCombination"),
        },
        DecisionCase {
            case: "the skip alone",
            members: vec![("SkipFinalClusterSnapshot", "true")],
            refusal: None,
        },
        DecisionCase {
            case: "the final snapshot alone",
            members: vec![("FinalClusterSnapshotIdentifier", "final-snap")],
            refusal: None,
        },
        DecisionCase {
            case: "the skip spelled false beside the snapshot it needs",
            members: vec![
                ("SkipFinalClusterSnapshot", "false"),
                ("FinalClusterSnapshotIdentifier", "final-snap-two"),
            ],
            refusal: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One cluster per row: a served delete removes the one it names.
        let identifier = format!("decision-{index:02}");
        assert_served(
            &format!("seed {identifier}"),
            &emulator.create_minimal(&identifier).await,
        );
        // The cluster is available before the delete, exactly as a caller that
        // waited for it would see it.
        tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;

        let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", identifier.as_str())];
        params.extend(row.members.iter().copied());
        let answer = emulator.call("DeleteCluster", &params).await;

        let Some(code) = row.refusal else {
            assert_served(row.case, &answer);
            assert_eq!(
                answer.text("ClusterStatus").as_deref(),
                Some(state::STATUS_DELETING),
                "case {}: expected the served delete to report the cluster deleting, got {:?}",
                row.case,
                answer.text("ClusterStatus")
            );
            continue;
        };

        assert_refused(row.case, &answer, 400, code);
        if row.members.is_empty() {
            assert_eq!(
                answer.text("Message").as_deref(),
                Some(state::FINAL_SNAPSHOT_DECISION_REQUIRED),
                "case {}: expected the fixed decision sentence, got {:?}",
                row.case,
                answer.text("Message")
            );
        }

        // The refused delete destroyed nothing: the cluster is still there and
        // still available.
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
            .await;
        assert_served(&format!("{} read back", row.case), &described);
        assert_eq!(
            described.text("ClusterStatus").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {}: expected the cluster to stay available, got {:?}",
            row.case,
            described.text("ClusterStatus")
        );
        // Nor did it take a snapshot the request never decided on.
        let snapshots = emulator.call("DescribeClusterSnapshots", &[]).await;
        assert!(
            all_text(&snapshots.body, "SnapshotIdentifier").is_empty(),
            "case {}: expected the refusal to take no snapshot, got body {}",
            row.case,
            snapshots.body
        );
    }
}

/// One row of the parameter-domain table: the name-value pair a modify carries,
/// whether the family's published catalog admits it, and whether the name is one
/// the family does not publish at all (which carries its own sentence).
struct DomainCase {
    case: &'static str,
    parameter: &'static str,
    value: &'static str,
    admitted: bool,
    unpublished_name: bool,
}

/// A parameter a modify names is resolved against the catalog its family
/// publishes, and anything outside that domain — a name the family does not
/// publish, or a value the published `AllowedValues` set does not admit — is
/// refused with `InvalidParameterValue`, with the whole request left unapplied.
///
/// Source: the `AllowedValues` of `state::ENGINE_DEFAULT_PARAMETERS`, which is
/// what `smithy_query(query="doc", shape="Parameter", member="AllowedValues")`
/// ("The valid range of values for the parameter") publishes per parameter. The
/// operation models only `ClusterParameterGroupNotFoundFault` and
/// `InvalidClusterParameterGroupStateFault`, so the domain refusal is the
/// service's own parameter-value code.
#[tokio::test]
async fn a_parameter_outside_the_published_domain_is_an_invalid_parameter_value() {
    let emulator = Emulator::new();
    const GROUP: &str = "domain-group";
    create_group(&emulator, GROUP).await;

    let rows: Vec<DomainCase> = vec![
        DomainCase {
            case: "a boolean parameter given a value the set admits",
            parameter: "require_ssl",
            value: "true",
            admitted: true,
            unpublished_name: false,
        },
        DomainCase {
            case: "a boolean parameter given prose",
            parameter: "require_ssl",
            value: "maybe",
            admitted: false,
            unpublished_name: false,
        },
        DomainCase {
            case: "a boolean parameter given a number",
            parameter: "enable_user_activity_logging",
            value: "1",
            admitted: false,
            unpublished_name: false,
        },
        DomainCase {
            case: "a boolean parameter given the set's value in another case",
            parameter: "use_fips_ssl",
            value: "TRUE",
            admitted: false,
            unpublished_name: false,
        },
        DomainCase {
            case: "a numeric parameter inside its published range",
            parameter: "max_concurrency_scaling_clusters",
            value: "10",
            admitted: true,
            unpublished_name: false,
        },
        DomainCase {
            case: "a numeric parameter one past its published range",
            parameter: "max_concurrency_scaling_clusters",
            value: "11",
            admitted: false,
            unpublished_name: false,
        },
        DomainCase {
            case: "the JSON-document parameter given a list of queues",
            parameter: "wlm_json_configuration",
            value: "[{\"auto_wlm\":true}]",
            admitted: true,
            unpublished_name: false,
        },
        DomainCase {
            case: "the JSON-document parameter given malformed JSON",
            parameter: "wlm_json_configuration",
            value: "[{\"auto_wlm\":true",
            admitted: false,
            unpublished_name: false,
        },
        DomainCase {
            case: "the JSON-document parameter given a JSON object instead of a list",
            parameter: "wlm_json_configuration",
            value: "{\"auto_wlm\":true}",
            admitted: false,
            unpublished_name: false,
        },
        DomainCase {
            case: "the JSON-document parameter given prose",
            parameter: "wlm_json_configuration",
            value: "auto",
            admitted: false,
            unpublished_name: false,
        },
        DomainCase {
            case: "a parameter name the family does not publish",
            parameter: "no_such_parameter",
            value: "1",
            admitted: false,
            unpublished_name: true,
        },
        DomainCase {
            case: "a parameter name of another database engine",
            parameter: "shared_buffers",
            value: "128MB",
            admitted: false,
            unpublished_name: true,
        },
        DomainCase {
            case: "a published name in the wrong case",
            parameter: "REQUIRE_SSL",
            value: "true",
            admitted: false,
            unpublished_name: true,
        },
        DomainCase {
            case: "a parameter publishing no set, given free text",
            parameter: "search_path",
            value: "$user, public, staging",
            admitted: true,
            unpublished_name: false,
        },
    ];

    for row in &rows {
        let before = reported(&emulator, GROUP).await;
        let answer = modify(&emulator, GROUP, &[(row.parameter, row.value)]).await;
        let after = reported(&emulator, GROUP).await;

        if row.admitted {
            assert_served(row.case, &answer);
            let held = after
                .iter()
                .find(|(name, _, _)| name == row.parameter)
                .map(|(_, value, source)| (value.as_str(), source.as_str()));
            assert_eq!(
                held,
                Some((row.value, state::PARAMETER_SOURCE_USER)),
                "case {}: expected the group to hold the value with Source user, got {held:?}",
                row.case
            );
            continue;
        }

        assert_refused(row.case, &answer, 400, "InvalidParameterValue");
        if row.unpublished_name {
            assert_eq!(
                answer.text("Message").as_deref(),
                Some(state::parameter_not_found_message(row.parameter).as_str()),
                "case {}: expected the refusal to say the name is no parameter of the family, \
                 got {:?}",
                row.case,
                answer.text("Message")
            );
        }
        assert_eq!(
            after, before,
            "case {}: expected the refused modify to change nothing the group reports",
            row.case
        );
    }

    // A partially invalid modify is refused in its ENTIRETY: the good entry
    // beside it is not applied either.
    let before = reported(&emulator, GROUP).await;
    let partial = modify(
        &emulator,
        GROUP,
        &[("auto_analyze", "false"), ("no_such_parameter", "1")],
    )
    .await;
    assert_refused(
        "one good entry beside one the family does not publish",
        &partial,
        400,
        "InvalidParameterValue",
    );
    assert_eq!(
        reported(&emulator, GROUP).await,
        before,
        "case \"a partially invalid modify\": expected nothing to be applied"
    );
}

/// The DEFAULT parameter group of every published family is the service's own:
/// a modify, a reset and a delete of it are each refused with
/// `InvalidParameterValue` — not with the transient-sounding group-state fault —
/// and the group is still there afterwards, while an ordinary group the account
/// created deletes.
///
/// Source: the Amazon Redshift Cluster Management Guide, "Amazon Redshift
/// parameter groups" — you cannot change parameter values in a default parameter
/// group, and a default group is not the account's to delete.
#[tokio::test]
async fn the_service_owned_default_parameter_group_takes_no_change_from_a_caller() {
    let emulator = Emulator::new();

    // Every default group a fresh account has: one per published family, read
    // off the same catalog the store seeds them from.
    let defaults: Vec<String> = state::published_parameter_group_families()
        .iter()
        .map(|family| state::family_default_parameter_group(family))
        .collect();
    assert!(
        defaults.contains(&default_group_name()),
        "case \"the published families\": expected the default group of the published engine \
         version among {defaults:?}"
    );

    for group in &defaults {
        for (case, action, params) in [
            (
                format!("delete {group}"),
                "DeleteClusterParameterGroup",
                vec![("ParameterGroupName", group.as_str())],
            ),
            (
                format!("reset {group}"),
                "ResetClusterParameterGroup",
                vec![
                    ("ParameterGroupName", group.as_str()),
                    ("ResetAllParameters", "true"),
                ],
            ),
        ] {
            let answer = emulator.call(action, &params).await;
            assert_refused(&case, &answer, 400, "InvalidParameterValue");
            if action == "DeleteClusterParameterGroup" {
                assert_eq!(
                    answer.text("Message").as_deref(),
                    Some(format!("The Cluster Parameter Group cannot be deleted: {group}").as_str()),
                    "case {case}: expected the refusal to name the group it will not delete, \
                     got {:?}",
                    answer.text("Message")
                );
            }
        }

        let modified = modify(&emulator, group, &[("require_ssl", "true")]).await;
        assert_refused(
            &format!("modify {group}"),
            &modified,
            400,
            "InvalidParameterValue",
        );

        // The group survived every refusal, and still reports its family's
        // engine defaults.
        let described = emulator
            .call(
                "DescribeClusterParameterGroups",
                &[("ParameterGroupName", group.as_str())],
            )
            .await;
        assert_served(&format!("read {group} back"), &described);
        let sources: Vec<String> = reported(&emulator, group)
            .await
            .into_iter()
            .map(|(_, _, source)| source)
            .collect();
        assert!(
            sources
                .iter()
                .all(|source| source == state::PARAMETER_SOURCE_ENGINE_DEFAULT),
            "case \"{group} after the refusals\": expected every parameter at its engine \
             default, got {sources:?}"
        );
    }

    // The same delete of a group the ACCOUNT created is served: the refusal
    // above is about who owns the group, not about the operation.
    create_group(&emulator, "deletable-group").await;
    let deleted = emulator
        .call(
            "DeleteClusterParameterGroup",
            &[("ParameterGroupName", "deletable-group")],
        )
        .await;
    assert_served("delete a group the account created", &deleted);
}

/// A create under a name the account already holds a group under is refused with
/// the modelled `ClusterParameterGroupAlreadyExists`, worded as AWS words it:
/// the fixed text around the requested `ParameterGroupName`.
#[tokio::test]
async fn a_duplicate_parameter_group_create_names_the_group_that_already_exists() {
    let emulator = Emulator::new();
    let family = default_family();

    // Rows differ along the name the loser requests: the name as created, and
    // the same name in another case (a group name is stored lower-case, so
    // these are the SAME group) — uniqueness is on the stored name alone.
    create_group(&emulator, "taken-group").await;
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the same name again",
            "taken-group".to_string(),
            "taken-group".to_string(),
        ),
        (
            "the same name in another case",
            "Taken-Group".to_string(),
            "Taken-Group".to_string(),
        ),
        (
            "the same name in upper case",
            "TAKEN-GROUP".to_string(),
            "TAKEN-GROUP".to_string(),
        ),
    ];

    for (case, requested, named) in &rows {
        let answer = emulator
            .call(
                "CreateClusterParameterGroup",
                &[
                    ("ParameterGroupName", requested.as_str()),
                    ("ParameterGroupFamily", family.as_str()),
                    ("Description", "a duplicate"),
                ],
            )
            .await;
        assert_refused(case, &answer, 400, "ClusterParameterGroupAlreadyExists");
        assert_eq!(
            answer.text("Message").as_deref(),
            Some(state::cluster_parameter_group_already_exists_message(named).as_str()),
            "case {case}: expected the refusal to name the group that already exists, got {:?}",
            answer.text("Message")
        );
    }
}

/// `PutResourcePolicy` tells a document it could not read apart from the other
/// causes of `InvalidPolicyFault` by its MESSAGE, and stores nothing: the read
/// that follows reports no policy.
#[tokio::test]
async fn a_policy_document_that_cannot_be_parsed_says_exactly_that() {
    let emulator = Emulator::new();
    let holder = format!("arn:aws:redshift-serverless:{REGION}:000000000000:namespace/5533bed2-5c89-4049-91c3-77252462b84d");

    for (case, document) in [
        ("a truncated object", r#"{"Statement":"#),
        ("plain text", "not a policy"),
        ("an unquoted key", "{Version: 2012-10-17}"),
        ("a trailing comma", r#"{"Version":"2012-10-17",}"#),
        ("two documents in one value", "{}{}"),
    ] {
        let refused = emulator
            .call(
                "PutResourcePolicy",
                &[("ResourceArn", holder.as_str()), ("Policy", document)],
            )
            .await;
        assert_refused(case, &refused, 400, "InvalidPolicyFault");
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(state::RESOURCE_POLICY_NOT_A_POLICY_DOCUMENT),
            "case {case}: expected the parse-failure sentence, got {:?}",
            refused.text("Message")
        );

        let read = emulator
            .call("GetResourcePolicy", &[("ResourceArn", holder.as_str())])
            .await;
        assert_refused(
            &format!("{case}, read back"),
            &read,
            404,
            "ResourceNotFoundFault",
        );
    }
}

/// The resource-policy operations resolve the `ResourceArn` against the control
/// plane that OWNS the resource before any policy state is touched: a serverless
/// namespace that control plane holds is served, and one it answers it does not
/// hold is refused by the write, the read and the delete alike — with nothing
/// attached.
#[tokio::test]
async fn a_resource_policy_resolves_its_arn_against_the_control_plane_that_owns_it() {
    let emulator = Emulator::with_serverless();

    let (status, body) = emulator
        .call_serverless(
            "CreateNamespace",
            &serde_json::json!({ "namespaceName": "policy-namespace" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"seed the serverless namespace\": expected 200, got {status} with {body}"
    );
    let held = body["namespace"]["namespaceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"seed the serverless namespace\": expected a namespaceArn in {body}"))
        .to_string();

    let policy = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"redshift.amazonaws.com"},"Action":"redshift:CreateInboundIntegration","Resource":"*"}]}"#;
    let written = emulator
        .call(
            "PutResourcePolicy",
            &[("ResourceArn", held.as_str()), ("Policy", policy)],
        )
        .await;
    assert_served("a namespace that control plane holds", &written);
    let read = emulator
        .call("GetResourcePolicy", &[("ResourceArn", held.as_str())])
        .await;
    assert_served("read the attached policy back", &read);
    assert_eq!(
        read.text("Policy").as_deref(),
        Some(policy),
        "case \"read the attached policy back\": expected the document verbatim, got {:?}",
        read.text("Policy")
    );

    // An ARN of that same control plane naming a namespace it does not hold is
    // refused by all three operations.
    let unheld = format!(
        "arn:aws:redshift-serverless:{REGION}:000000000000:namespace/00000000-0000-4000-8000-000000000000"
    );
    for (case, action, params) in [
        (
            "a namespace that control plane does not hold, write",
            "PutResourcePolicy",
            vec![("ResourceArn", unheld.as_str()), ("Policy", policy)],
        ),
        (
            "a namespace that control plane does not hold, read",
            "GetResourcePolicy",
            vec![("ResourceArn", unheld.as_str())],
        ),
        (
            "a namespace that control plane does not hold, delete",
            "DeleteResourcePolicy",
            vec![("ResourceArn", unheld.as_str())],
        ),
    ] {
        let answer = emulator.call(action, &params).await;
        assert_refused(case, &answer, 400, "InvalidParameterValue");
    }

    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let attached: Vec<String> = store.lock().resource_policies.keys().cloned().collect();
    assert_eq!(
        attached,
        vec![held],
        "case \"a refused write\": expected only the namespace that exists to carry a policy, \
         got {attached:?}"
    );
}
