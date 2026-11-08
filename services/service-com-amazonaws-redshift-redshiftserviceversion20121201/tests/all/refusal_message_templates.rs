// SPDX-License-Identifier: CC0-1.0

//! Two refusal MESSAGES whose text a client is written against, end to end over
//! the awsQuery wire.
//!
//! The RULE every case here asserts: a refusal's message is Amazon Redshift's own
//! wording for that error class — fixed text plus the variable parts taken from
//! the request — so a caller matching on the sentence recognises it. Concretely:
//!
//! * re-creating a cluster subnet group name the account already has is refused
//!   `ClusterSubnetGroupAlreadyExists` with `The Cluster subnet group '<name>'
//!   already exists.`, the one variable part being the name the REQUEST spelled;
//! * a `GetClusterCredentials` whose `DurationSeconds` is outside the documented
//!   900..3600 window is refused with `Token duration must be between 900 and 3600
//!   seconds` — fixed text whose only values are the two documented bounds, so the
//!   sentence does NOT echo the value the request asked for and is the same for
//!   every rejected value.
//!
//! The rows differ along what each rule generalises over: the case, length and
//! shape of the group name for the first, and — for the second — which side of the
//! window the value falls on, how far outside it is, and whether it is zero or
//! negative. The expected sentences are written out literally here, so a change
//! to the fixed text fails a test rather than passing one computed from the same
//! code under test.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// The admin user every credential request below asks for, which is the cluster's
/// own so that nothing but the lifetime can be what a refusal is about.
const MASTER_USERNAME: &str = "admin";

/// `CreateClusterSubnetGroup` over `subnet_ids`, under the name as spelled.
async fn create_group(emulator: &Emulator, name: &str, subnet_ids: &[String]) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("ClusterSubnetGroupName".to_string(), name.to_string()),
        ("Description".to_string(), "a group".to_string()),
    ];
    for (index, subnet_id) in subnet_ids.iter().enumerate() {
        params.push((
            format!("SubnetIds.SubnetIdentifier.{}", index + 1),
            subnet_id.clone(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("CreateClusterSubnetGroup", &borrowed)
        .await
}

#[tokio::test]
async fn a_repeat_subnet_group_create_names_the_group_that_already_exists() {
    let emulator = Emulator::new();

    // (case, the name the first create used, the name the repeat create uses)
    let rows: [(&str, &str, &str); 4] = [
        (
            "the same lowercase name twice",
            "analytics-subnets",
            "analytics-subnets",
        ),
        (
            "a mixed-case name repeated as written",
            "Analytics-Subnets-Two",
            "Analytics-Subnets-Two",
        ),
        // Amazon Redshift "stores the value as a lowercase string", so a repeat
        // under another case names the SAME group — and the refusal quotes what
        // the repeat asked for.
        (
            "a stored name repeated in another case",
            "Warehouse-Subnets",
            "warehouse-subnets",
        ),
        (
            "a single-character name",
            "g",
            "g",
        ),
    ];

    for (case, first, repeat) in rows {
        let subnets = emulator.subnet_ids(case, 2).await;
        let created = create_group(&emulator, first, &subnets).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the first create, got {} with body {}",
            created.status, created.body
        );

        let answered = create_group(&emulator, repeat, &subnets).await;
        assert_eq!(
            (answered.status, answered.error_code().as_deref()),
            (400, Some("ClusterSubnetGroupAlreadyExists")),
            "case {case:?}: expected 400 ClusterSubnetGroupAlreadyExists, got {} / {:?} with \
             body {}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        let expected = format!("The Cluster subnet group '{repeat}' already exists.");
        assert_eq!(
            answered.text("Message").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the refusal {expected:?}, got {:?}",
            answered.text("Message")
        );
    }
}

#[tokio::test]
async fn a_token_lifetime_outside_the_window_is_refused_in_the_documented_words() {
    let emulator = Emulator::new();
    let cluster = "token-duration-cluster";
    let created = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", cluster),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", MASTER_USERNAME),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateCluster, got {} with body {}",
        created.status, created.body
    );

    let min = state::CREDENTIAL_DURATION_MIN_SECONDS;
    let max = state::CREDENTIAL_DURATION_MAX_SECONDS;
    // The sentence, with the two documented bounds as its only values. Written out
    // rather than built from the code under test.
    let expected = "Token duration must be between 900 and 3600 seconds";
    assert_eq!(
        (min, max),
        (900, 3600),
        "the documented window is 900..3600 seconds, got {min}..{max}"
    );

    // Each published bound is accepted; one past each, zero, a negative lifetime
    // and a value far outside are refused — in the same words.
    let rows: [(&str, i32, bool); 7] = [
        ("the published minimum", min, true),
        ("the published maximum", max, true),
        ("one second under the minimum", min - 1, false),
        ("one second over the maximum", max + 1, false),
        ("no lifetime at all", 0, false),
        ("a negative lifetime", -1, false),
        ("a whole day", 86_400, false),
    ];

    for (case, duration, accepted) in rows {
        let seconds = duration.to_string();
        let answered = emulator
            .call(
                "GetClusterCredentials",
                &[
                    ("ClusterIdentifier", cluster),
                    ("DbUser", MASTER_USERNAME),
                    ("DurationSeconds", seconds.as_str()),
                ],
            )
            .await;
        if accepted {
            assert_eq!(
                answered.status, 200,
                "case {case:?} ({duration}s): expected HTTP 200, got {} with body {}",
                answered.status, answered.body
            );
            continue;
        }
        assert_eq!(
            (answered.status, answered.error_code().as_deref()),
            (400, Some("InvalidParameterValue")),
            "case {case:?} ({duration}s): expected 400 InvalidParameterValue, got {} / {:?} \
             with body {}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        let message = answered.text("Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?} ({duration}s): expected the refusal {expected:?}, got {message:?}"
        );
    }
}
