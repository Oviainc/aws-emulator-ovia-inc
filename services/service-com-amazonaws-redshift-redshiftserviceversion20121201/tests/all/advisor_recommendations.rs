// SPDX-License-Identifier: CC0-1.0

//! `ListRecommendations`, end to end over the awsQuery wire.
//!
//! The RULE every case here asserts: the report is the Advisor observations the
//! store holds, scoped to the clusters the request names — the one cluster a
//! `ClusterIdentifier` or a `NamespaceArn` names, or every cluster of the
//! account when it names neither — in the order they were recorded, and paged
//! with this service's shared `Marker` / `MaxRecords` cursor. A scope naming no
//! cluster this account holds is refused with 404 `ClusterNotFound`, and a
//! cluster no observation names reports an empty page.
//!
//! The table spans what the rule generalises over: which scope member the
//! request carries (neither, each one, both agreeing, both disagreeing), how a
//! record identifies the cluster it observes (by identifier, by namespace ARN,
//! by a cluster the account does not hold), how many records a scope selects
//! (none, one, several, more than one page), and the documented `MaxRecords`
//! window on each published bound and one past it.
//!
//! The observations themselves are written straight into the shared store the
//! handler reads, because nothing in this control plane mints one: an Advisor
//! finding comes from analysing a cluster's workload — its query and table
//! history — which this emulator holds none of, and the service publishes no API
//! that records one. That is exactly why a cluster none has observed reports an
//! empty page. The request path is unchanged.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::{
    Recommendation, RecommendedAction, ReferenceLink,
};
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, element_blocks, Answer, Emulator, REGION};

/// One Advisor observation, with every member distinguishable from the record's
/// own id so a report cannot pass by rebuilding a record it did not store.
///
/// `ImpactRanking` and the action's `Type` are values of the enums the model
/// declares for them (`smithy_query(query="shape", shape="ImpactRankingType")` —
/// HIGH | MEDIUM | LOW — and `shape="RecommendedActionType"` — SQL | CLI).
fn observation(
    id: &str,
    cluster_identifier: Option<&str>,
    namespace_arn: Option<&str>,
) -> Recommendation {
    Recommendation {
        id: Some(id.to_string()),
        cluster_identifier: cluster_identifier.map(str::to_string),
        namespace_arn: namespace_arn.map(str::to_string),
        created_at: Some(aws_smithy_types::DateTime::from(std::time::SystemTime::now())),
        recommendation_type: Some(format!("{id}-type")),
        title: Some(format!("{id} title")),
        description: Some(format!("{id} description")),
        observation: Some(format!("{id} observation")),
        impact_ranking: Some("HIGH".to_string()),
        recommendation_text: Some(format!("{id} recommendation text")),
        recommended_actions: Some(vec![RecommendedAction {
            text: Some(format!("{id} action")),
            database: Some(state::DEFAULT_DB_NAME.to_string()),
            command: Some("ANALYZE".to_string()),
            r#type: Some("SQL".to_string()),
        }]),
        reference_links: Some(vec![ReferenceLink {
            text: Some(format!("{id} reference")),
            link: Some(format!("https://docs.aws.amazon.com/redshift/{id}")),
        }]),
    }
}

/// Record one observation against a cluster, straight into the same shared store
/// the handler reads.
fn record_observation(
    emulator: &Emulator,
    id: &str,
    cluster_identifier: Option<&str>,
    namespace_arn: Option<&str>,
) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    store
        .recommendations
        .push(observation(id, cluster_identifier, namespace_arn));
}

/// Create a cluster and answer the namespace ARN it publishes — the other way a
/// request, and a record, names the cluster an observation is about.
async fn cluster_with_namespace(emulator: &Emulator, identifier: &str) -> String {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
    created
        .text("ClusterNamespaceArn")
        .unwrap_or_else(|| panic!("case seed {identifier}: the cluster reports its namespace ARN"))
}

/// The `Id` of every recommendation a report carries, in the order the page
/// lists them.
fn listed(body: &str) -> Vec<String> {
    element_blocks(body, "Recommendation")
        .iter()
        .map(|block| {
            all_text(block, "Id")
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("every reported recommendation carries an Id: {block}"))
        })
        .collect()
}

/// Assert one refusal row: the answer's status, its `<Code>`, and that the
/// message names the value the request was refused for.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str, names: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
    assert!(
        answer.body.contains(names),
        "case {case}: expected the refusal to name {names:?}, got body {}",
        answer.body
    );
}

/// One row of the scope table: the case name, the request's scope parameters,
/// and the record ids the report must carry, in order.
struct ScopeCase<'a> {
    case: &'a str,
    params: Vec<(&'a str, &'a str)>,
    expected: Vec<&'a str>,
}

#[tokio::test]
async fn the_report_is_the_recorded_observations_of_the_clusters_the_scope_names() {
    let emulator = Emulator::new();
    let alpha_namespace = cluster_with_namespace(&emulator, "advisor-alpha").await;
    let beta_namespace = cluster_with_namespace(&emulator, "advisor-beta").await;

    // Recorded in this order, so the report's order is observable: two
    // observations of one cluster, one of the other, one identified only by the
    // namespace holding a cluster's data, and one naming a cluster this account
    // does not hold at all.
    record_observation(
        &emulator,
        "obs-alpha-1",
        Some("advisor-alpha"),
        Some(&alpha_namespace),
    );
    record_observation(
        &emulator,
        "obs-beta-1",
        Some("advisor-beta"),
        Some(&beta_namespace),
    );
    record_observation(
        &emulator,
        "obs-alpha-2",
        Some("advisor-alpha"),
        Some(&alpha_namespace),
    );
    record_observation(&emulator, "obs-beta-namespace", None, Some(&beta_namespace));
    record_observation(&emulator, "obs-departed", Some("advisor-departed"), None);

    let cases = vec![
        ScopeCase {
            // "If the neither the cluster identifier and the cluster namespace
            // ARN parameters are specified, then recommendations for all
            // clusters in the account are returned" — all but the one observing
            // a cluster the account does not hold.
            case: "no scope at all",
            params: Vec::new(),
            expected: vec![
                "obs-alpha-1",
                "obs-beta-1",
                "obs-alpha-2",
                "obs-beta-namespace",
            ],
        },
        ScopeCase {
            case: "scoped by the identifier of the cluster with two observations",
            params: vec![("ClusterIdentifier", "advisor-alpha")],
            expected: vec!["obs-alpha-1", "obs-alpha-2"],
        },
        ScopeCase {
            // Both scope members name a CLUSTER, so a record naming its cluster
            // only by the namespace holding that cluster's data is in the
            // cluster's identifier scope too.
            case: "scoped by the identifier of the cluster a record names by namespace",
            params: vec![("ClusterIdentifier", "advisor-beta")],
            expected: vec!["obs-beta-1", "obs-beta-namespace"],
        },
        ScopeCase {
            case: "scoped by namespace ARN",
            params: vec![("NamespaceArn", alpha_namespace.as_str())],
            expected: vec!["obs-alpha-1", "obs-alpha-2"],
        },
        ScopeCase {
            // The same cluster, named the other way round: the namespace scope
            // selects the same records its identifier scope does.
            case: "scoped by the namespace ARN of that same cluster",
            params: vec![("NamespaceArn", beta_namespace.as_str())],
            expected: vec!["obs-beta-1", "obs-beta-namespace"],
        },
        ScopeCase {
            case: "both scope members, naming the same cluster",
            params: vec![
                ("ClusterIdentifier", "advisor-alpha"),
                ("NamespaceArn", alpha_namespace.as_str()),
            ],
            expected: vec!["obs-alpha-1", "obs-alpha-2"],
        },
        ScopeCase {
            // Each member narrows, so a pair naming two different clusters
            // selects the records belonging to both — of which there are none.
            case: "both scope members, naming different clusters",
            params: vec![
                ("ClusterIdentifier", "advisor-alpha"),
                ("NamespaceArn", beta_namespace.as_str()),
            ],
            expected: Vec::new(),
        },
    ];

    for case in cases {
        let answer = emulator.call("ListRecommendations", &case.params).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected 200, got {} with body {}",
            case.case, answer.status, answer.body
        );
        let reported = listed(&answer.body);
        assert_eq!(
            reported, case.expected,
            "case {}: expected the recommendations {:?}, got {reported:?} with body {}",
            case.case, case.expected, answer.body
        );
        assert_eq!(
            answer.text("Marker"),
            None,
            "case {}: every selected record fits one default page, so expected no cursor, got {:?}",
            case.case,
            answer.text("Marker")
        );
    }
}

/// The record a report carries is the one the store holds, member for member.
#[tokio::test]
async fn a_reported_recommendation_carries_the_record_the_store_holds() {
    let emulator = Emulator::new();
    let namespace = cluster_with_namespace(&emulator, "advisor-members").await;
    record_observation(
        &emulator,
        "obs-members",
        Some("advisor-members"),
        Some(&namespace),
    );
    let stored = observation("obs-members", Some("advisor-members"), Some(&namespace));

    let answer = emulator.call("ListRecommendations", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case the whole record: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    for (member, value) in [
        ("Id", stored.id.clone()),
        ("ClusterIdentifier", stored.cluster_identifier.clone()),
        ("NamespaceArn", stored.namespace_arn.clone()),
        ("RecommendationType", stored.recommendation_type.clone()),
        ("Title", stored.title.clone()),
        ("Description", stored.description.clone()),
        ("Observation", stored.observation.clone()),
        ("ImpactRanking", stored.impact_ranking.clone()),
        ("RecommendationText", stored.recommendation_text.clone()),
    ] {
        assert_eq!(
            answer.text(member),
            value,
            "case the whole record: expected {member} {value:?}, got {:?} with body {}",
            answer.text(member),
            answer.body
        );
    }
    let action = &stored.recommended_actions.as_deref().expect("one action")[0];
    for (member, value) in [
        ("Text", action.text.clone()),
        ("Database", action.database.clone()),
        ("Command", action.command.clone()),
        ("Type", action.r#type.clone()),
    ] {
        assert_eq!(
            answer.text(member),
            value,
            "case the recommended action: expected {member} {value:?}, got {:?} with body {}",
            answer.text(member),
            answer.body
        );
    }
    let reference = &stored.reference_links.as_deref().expect("one link")[0];
    assert_eq!(
        answer.text("Link"),
        reference.link.clone(),
        "case the reference link: expected Link {:?}, got {:?} with body {}",
        reference.link,
        answer.text("Link"),
        answer.body
    );
}

/// A cluster no analysis has observed reports an EMPTY page — the answer the
/// real service gives a cluster Advisor has no finding for — never a refusal and
/// never a finding derived from the cluster's configuration.
#[tokio::test]
async fn a_cluster_no_observation_names_reports_an_empty_page() {
    for (case, clusters, params) in [
        ("an account holding nothing at all", Vec::new(), Vec::new()),
        (
            "an account whose cluster carries no observation",
            vec!["advisor-unobserved"],
            Vec::new(),
        ),
        (
            "that same cluster named as the scope",
            vec!["advisor-unobserved"],
            vec![("ClusterIdentifier", "advisor-unobserved")],
        ),
    ] {
        let emulator = Emulator::new();
        for identifier in &clusters {
            cluster_with_namespace(&emulator, identifier).await;
        }
        let answer = emulator.call("ListRecommendations", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            listed(&answer.body),
            Vec::<String>::new(),
            "case {case}: expected no recommendation, got body {}",
            answer.body
        );
        assert_eq!(
            answer.text("Marker"),
            None,
            "case {case}: expected no cursor, got {:?}",
            answer.text("Marker")
        );
    }
}

/// A scope naming no cluster this account holds is refused as a cluster that is
/// not there, whichever member names it.
#[tokio::test]
async fn a_scope_naming_no_cluster_of_this_account_is_refused() {
    let emulator = Emulator::new();
    let namespace = cluster_with_namespace(&emulator, "advisor-held").await;
    record_observation(&emulator, "obs-held", Some("advisor-held"), Some(&namespace));

    // A namespace ARN of exactly the form this service publishes, for a cluster
    // that was never created.
    let unheld_namespace = state::namespace_arn("advisor-never-created", REGION);

    for (case, member, value) in [
        (
            "an identifier naming no cluster",
            "ClusterIdentifier",
            "advisor-never-created".to_string(),
        ),
        (
            "a well-formed namespace ARN naming no cluster",
            "NamespaceArn",
            unheld_namespace.clone(),
        ),
        (
            "a namespace value that is no ARN at all",
            "NamespaceArn",
            "advisor-never-created".to_string(),
        ),
    ] {
        let answer = emulator
            .call("ListRecommendations", &[(member, value.as_str())])
            .await;
        assert_refused(case, &answer, 404, "ClusterNotFound", &value);
        assert_eq!(
            listed(&answer.body),
            Vec::<String>::new(),
            "case {case}: a refusal reports no recommendation, got body {}",
            answer.body
        );
    }

    // The same identifier, once the cluster has left the account: what the scope
    // resolves against is the store, not the request.
    let deleted = emulator
        .call(
            "DeleteCluster",
            &[
                ("ClusterIdentifier", "advisor-held"),
                ("SkipFinalClusterSnapshot", "true"),
            ],
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete advisor-held: expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let after = emulator
        .call(
            "ListRecommendations",
            &[("ClusterIdentifier", "advisor-held")],
        )
        .await;
    assert_refused(
        "the identifier of a cluster the account no longer holds",
        &after,
        404,
        "ClusterNotFound",
        "advisor-held",
    );
    // And the observation of that cluster is an observation of no cluster this
    // account holds, so an unscoped report no longer carries it.
    let unscoped = emulator.call("ListRecommendations", &[]).await;
    assert_eq!(
        listed(&unscoped.body),
        Vec::<String>::new(),
        "case the departed cluster's observation: expected it reported for no cluster, got body {}",
        unscoped.body
    );
}

/// The report is a pure READ: two calls either side of nothing answer the same
/// page.
#[tokio::test]
async fn repeating_the_report_changes_nothing() {
    let emulator = Emulator::new();
    let namespace = cluster_with_namespace(&emulator, "advisor-stable").await;
    for id in ["obs-stable-1", "obs-stable-2"] {
        record_observation(&emulator, id, Some("advisor-stable"), Some(&namespace));
    }
    // Past the provisioning span, so the clock owes the cluster no transition
    // that could change a later answer.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;

    let first = emulator.call("ListRecommendations", &[]).await;
    let second = emulator.call("ListRecommendations", &[]).await;
    assert_eq!(
        listed(&first.body),
        vec!["obs-stable-1".to_string(), "obs-stable-2".to_string()],
        "case the first read: expected both observations, got body {}",
        first.body
    );
    assert_eq!(
        listed(&second.body),
        listed(&first.body),
        "case the repeated read: expected the same page, got bodies {} and {}",
        first.body,
        second.body
    );
}

/// The page follows the `MaxRecords` window this service publishes, and the
/// `Marker` it hands back resumes the same sequence where the page left it.
#[tokio::test]
async fn the_page_follows_the_published_max_records_window() {
    let emulator = Emulator::new();
    let namespace = cluster_with_namespace(&emulator, "advisor-paged").await;
    // One more than a page of 20, so the first page leaves a cursor with
    // records behind it.
    let total = 25usize;
    let ids: Vec<String> = (0..total).map(|index| format!("obs-paged-{index:02}")).collect();
    for id in &ids {
        record_observation(&emulator, id, Some("advisor-paged"), Some(&namespace));
    }

    for (case, max_records, accepted) in [
        ("one below the published minimum", "19", false),
        ("at the published minimum", "20", true),
        ("inside the window", "50", true),
        ("at the published maximum", "100", true),
        ("one past the published maximum", "101", true),
        ("far past the published maximum", "1000", true),
        ("zero", "0", false),
    ] {
        let answer = emulator
            .call("ListRecommendations", &[("MaxRecords", max_records)])
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} served with 200, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(case, &answer, 400, "InvalidParameterValue", "MaxRecords");
        }
    }

    // The default window takes every record in one page, with no cursor left.
    let unpaged = emulator.call("ListRecommendations", &[]).await;
    assert_eq!(
        listed(&unpaged.body),
        ids,
        "case the default window: expected all {total} observations in one page, got {:?}",
        listed(&unpaged.body)
    );
    assert_eq!(
        unpaged.text("Marker"),
        None,
        "case the default window: expected no cursor, got {:?}",
        unpaged.text("Marker")
    );

    // A page of 20 leaves a cursor the next call resumes from, and the two pages
    // are that one sequence split in order.
    let first_page = emulator
        .call("ListRecommendations", &[("MaxRecords", "20")])
        .await;
    let first_ids = listed(&first_page.body);
    assert_eq!(
        first_ids,
        ids[..20].to_vec(),
        "case the first page of 20: expected the first 20 observations, got {first_ids:?}"
    );
    let marker = first_page
        .text("Marker")
        .expect("case the first page of 20: records remain, so the page carries a cursor");
    let second_page = emulator
        .call(
            "ListRecommendations",
            &[("MaxRecords", "20"), ("Marker", marker.as_str())],
        )
        .await;
    let second_ids = listed(&second_page.body);
    assert_eq!(
        second_ids,
        ids[20..].to_vec(),
        "case the second page: expected the remaining {} observations, got {second_ids:?}",
        total - 20
    );
    assert_eq!(
        second_page.text("Marker"),
        None,
        "case the second page: nothing remains, so expected no cursor, got {:?}",
        second_page.text("Marker")
    );

    // A cursor this service never handed out is refused, not silently served as
    // the first page.
    let forged = emulator
        .call("ListRecommendations", &[("Marker", "not-a-cursor")])
        .await;
    assert_refused(
        "a cursor nothing produced",
        &forged,
        400,
        "InvalidParameterValue",
        "Marker",
    );
}
