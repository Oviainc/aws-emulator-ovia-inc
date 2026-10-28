// SPDX-License-Identifier: CC0-1.0

//! How the MSK configuration family refuses a reference to a configuration
//! REVISION it cannot resolve, driven end to end over the wire through the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! The RULE this suite asserts, over a table rather than one recorded request:
//! an operation that names one revision of a configuration — by `Arn` plus
//! `Revision` — refuses a pair that resolves to no stored revision with the
//! family's ONE unresolvable-reference refusal: HTTP 400 `BadRequestException`,
//! naming `Arn` as the parameter at fault and carrying the service's single
//! fixed sentence for that condition, a sentence with no variable parts, so
//! neither the ARN nor the revision number nor the region nor the account id
//! appears in it. It is that one refusal whichever half of the pair failed to
//! resolve: the sibling suite `configuration_arn_refusal.rs` walks the ways an
//! `Arn` resolves to no configuration, and this one walks the ways a revision
//! number resolves to no revision OF a configuration that does exist.
//!
//! The rows span what the rule generalises over: how many revisions the
//! configuration holds (so the bound the numbers are judged against moves), the
//! kind of number that resolves to nothing (one past the history, far past it,
//! the `__long` maximum, zero, negative, the `__long` minimum), the signing
//! region, and which operation names the reference (the revision reader, and
//! `UpdateClusterKafkaVersion`'s `ConfigurationInfo`).

use serde_json::json;

use super::kafka_harness::{
    assert_configuration_arn_resolves_to_nothing, pct, sleep_past_activation, ClusterRequest,
    ConfigurationRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The `server.properties` body revision `index` of a history is stored with —
/// distinct per revision, so a read that resolves is checked against the body
/// its own number was stored with.
fn body_for(index: usize) -> Vec<u8> {
    format!("num.partitions={index}\n").into_bytes()
}

/// A configuration of `revisions` revisions in `region`, as (ARN, revision
/// count): revision 1 is the create's and each further one an update's.
async fn configuration_with(
    emulator: &Emulator,
    region: &str,
    name: &str,
    revisions: usize,
) -> (String, i64) {
    let request = ConfigurationRequest {
        server_properties: body_for(1),
        extra_revisions: (2..=revisions).map(body_for).collect(),
        ..ConfigurationRequest::named(name)
    };
    let arn = emulator
        .create_configuration_row_ok_in(region, &request)
        .await;
    (arn, revisions as i64)
}

/// Every revision number that resolves to nothing for a configuration holding
/// `latest` revisions: one past the end, far past it, the maximum a `__long`
/// carries, and the numbers below the 1 a create stores.
fn unissued_numbers(latest: i64) -> Vec<(String, i64)> {
    vec![
        ("one-past-the-history".to_string(), latest + 1),
        ("far-past-the-history".to_string(), latest + 1_000),
        ("long-maximum".to_string(), i64::MAX),
        ("zero".to_string(), 0),
        ("negative".to_string(), -1),
        ("long-minimum".to_string(), i64::MIN),
    ]
}

/// The shared refusal assertion plus the one value this suite adds to it: a
/// revision number is a request value too, so it may not be interpolated into
/// the fixed sentence either.
fn assert_revision_resolves_to_nothing(
    reply: &Reply,
    case: &str,
    arn: &str,
    region: &str,
    revision: i64,
) {
    assert_configuration_arn_resolves_to_nothing(reply, case, arn, region);
    assert!(
        !reply.error_message().contains(&revision.to_string()),
        "case {case}: the message must not interpolate the revision number ({revision}), got {:?}",
        reply.error_message()
    );
}

/// RULE: a revision number a configuration never issued resolves to no revision,
/// so the reader refuses it with the family's one unresolvable-reference
/// `BadRequestException` — whatever the number is and however long the stored
/// history is — while every number the history does hold is still served.
#[tokio::test]
async fn an_unissued_revision_number_is_refused_as_an_unresolvable_reference() {
    let emulator = Emulator::new();

    // Histories of three lengths, so the bound the numbers are judged against
    // moves between rows rather than being a single stored case.
    for revisions in [1usize, 2, 7] {
        let (arn, latest) =
            configuration_with(&emulator, REGION, &format!("history-of-{revisions}"), revisions)
                .await;

        // Every number the history holds is served, with its own stored body:
        // the refusal below is a reference that resolves to nothing, not a
        // reader that refuses everything.
        for revision in 1..=latest {
            let reply = emulator
                .describe_configuration_revision(&arn, revision)
                .await;
            assert_eq!(
                (reply.status, reply.at("/revision").as_i64()),
                (200, Some(revision)),
                "case history-of-{revisions} revision={revision}: expected the stored revision, \
                 got {} {}",
                reply.status,
                reply.body
            );
        }

        for (case, revision) in unissued_numbers(latest) {
            let reply = emulator
                .describe_configuration_revision(&arn, revision)
                .await;
            assert_revision_resolves_to_nothing(
                &reply,
                &format!("history-of-{revisions}/{case}"),
                &arn,
                REGION,
                revision,
            );
        }

        // The refusals changed nothing: the history is still the one the writes
        // left, and revision 1 still reports the body the create stored.
        let described = emulator.describe_configuration(&arn).await;
        assert_eq!(
            described.at("/latestRevision/revision").as_i64(),
            Some(latest),
            "case history-of-{revisions}: a refused read must not move the latest revision, got {}",
            described.body
        );
        let first = emulator.describe_configuration_revision(&arn, 1).await;
        assert_eq!(
            first.status, 200,
            "case history-of-{revisions}: revision 1 must still be readable, got {} {}",
            first.status, first.body
        );
    }
}

/// RULE: the same refusal, whichever region's endpoint answers — a configuration
/// lives in one region, so the bound its revision numbers are judged against is
/// the history that region holds.
#[tokio::test]
async fn an_unissued_revision_number_is_refused_alike_in_every_region() {
    let emulator = Emulator::new();

    for region in [REGION, "eu-central-1", "ap-southeast-2"] {
        let (arn, latest) = configuration_with(&emulator, region, "regional-history", 2).await;
        for revision in 1..=latest {
            let reply = emulator
                .describe_configuration_revision_in(region, &arn, revision)
                .await;
            assert_eq!(
                reply.status, 200,
                "case region={region} revision={revision}: expected the stored revision, got {} {}",
                reply.status, reply.body
            );
        }
        for (case, revision) in unissued_numbers(latest) {
            let reply = emulator
                .describe_configuration_revision_in(region, &arn, revision)
                .await;
            assert_revision_resolves_to_nothing(
                &reply,
                &format!("region={region}/{case}"),
                &arn,
                region,
                revision,
            );
        }
    }
}

/// RULE: a pair that resolves to nothing is ONE refusal, whichever half of it
/// failed — a configuration the endpoint holds no record of and a revision
/// number an existing configuration never issued answer byte for byte the same
/// error envelope.
#[tokio::test]
async fn the_refusal_is_the_same_whichever_half_of_the_reference_fails() {
    let emulator = Emulator::new();
    let (held, latest) = configuration_with(&emulator, REGION, "one-envelope", 3).await;
    let absent = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/never-minted/\
         11111111-2222-4333-8444-555555555555-1"
    );

    // The revision the existing configuration never issued, and a revision
    // number it does hold asked of a configuration nothing holds.
    let unissued = emulator
        .describe_configuration_revision(&held, latest + 1)
        .await;
    let unknown = emulator.describe_configuration_revision(&absent, 1).await;
    assert_revision_resolves_to_nothing(
        &unissued,
        "unissued-revision",
        &held,
        REGION,
        latest + 1,
    );
    assert_revision_resolves_to_nothing(&unknown, "unknown-configuration", &absent, REGION, 1);
    assert_eq!(
        unissued.body, unknown.body,
        "case one-envelope: the two halves of an unresolvable reference must answer the same \
         envelope, got {} vs {}",
        unissued.body, unknown.body
    );
    assert_eq!(
        unissued.status, unknown.status,
        "case one-envelope: expected the same status, got {} vs {}",
        unissued.status, unknown.status
    );
}

/// RULE: an operation that names a configuration revision as a request MEMBER
/// judges it by the same rule the reader does, so `UpdateClusterKafkaVersion`
/// refuses a `ConfigurationInfo` whose revision was never issued with the
/// family's one unresolvable-reference refusal and applies nothing.
#[tokio::test]
async fn a_cluster_update_refuses_an_unissued_revision_the_same_way() {
    let emulator = Emulator::new();
    let (configuration, latest) =
        configuration_with(&emulator, REGION, "applied-to-a-cluster", 2).await;

    let cluster = emulator
        .create_ok(&ClusterRequest {
            kafka_version: "3.4.0",
            ..ClusterRequest::named("revision-reference")
        })
        .await;
    sleep_past_activation();
    let described = emulator.describe(&cluster).await;
    let current_version = described
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("the cluster reported no currentVersion: {}", described.body))
        .to_string();

    for (case, revision) in unissued_numbers(latest) {
        let reply = emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}/version", pct(&cluster)),
                Some(json!({
                    "currentVersion": current_version,
                    "targetKafkaVersion": "3.5.1",
                    "configurationInfo": { "arn": configuration, "revision": revision },
                })),
            )
            .await;
        assert_revision_resolves_to_nothing(
            &reply,
            &format!("cluster-update/{case}"),
            &configuration,
            REGION,
            revision,
        );
        let after = emulator.describe(&cluster).await;
        assert_eq!(
            after
                .at("/clusterInfo/currentBrokerSoftwareInfo/kafkaVersion")
                .as_str(),
            Some("3.4.0"),
            "case cluster-update/{case}: a refused update must install nothing, got {}",
            after.at("/clusterInfo/currentBrokerSoftwareInfo")
        );
    }
}
