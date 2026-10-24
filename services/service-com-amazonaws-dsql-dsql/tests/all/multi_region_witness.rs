// SPDX-License-Identifier: CC0-1.0

//! The witness Region of a multi-Region cluster configuration, over the wire.
//!
//! RULE under test: a `multiRegionProperties.witnessRegion` must name a Region
//! OTHER than the cluster's own Region — a witness holds a quorum vote outside the
//! cluster's Region — so every operation that accepts the member (`CreateCluster`
//! and `UpdateCluster`) refuses a witness equal to the cluster's Region with HTTP
//! 400 `ValidationException` naming that member, and leaves the store exactly as it
//! was; a witness naming any other Region is accepted and persisted.
//!
//! Source of the constraint: this service's own model — the sentence both
//! operations' documentation carries, "The witness Region specified in
//! multiRegionProperties.witnessRegion cannot be the same as the cluster's Region"
//! (`smithy_query(query="operation", operation="CreateCluster")` and
//! `operation="UpdateCluster"`).
//!
//! The table spans the class rather than one recorded request: several cluster
//! Regions (us, eu and ap partitions' Regions plus a GovCloud-style name), the
//! spellings an equal Region takes (as sent, upper-cased, mixed case, padded with
//! whitespace), Regions that merely share a prefix with the cluster's, and a
//! `multiRegionProperties` carrying peer clusters with and without a witness.

use serde_json::{json, Value};

use crate::harness::{wait_for_the_clock, Answer, Dsql};

/// The `ValidationExceptionReason` a member-value refusal carries. Source: the
/// service's own model, `smithy_query(query="shape",
/// shape="ValidationExceptionReason")` — one of `unknownOperation`, `cannotParse`,
/// `fieldValidationFailed`, `deletionProtectionEnabled`, `other`.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The member name the refusal must name in `fieldList`, spelled with its path
/// from the request root the way both operations' documentation spells it.
const WITNESS_MEMBER: &str = "multiRegionProperties.witnessRegion";

/// One `witnessRegion` sent against a cluster in `region`, and whether the service
/// may accept it.
struct WitnessCase {
    name: &'static str,
    /// The Region the cluster belongs to (the Region its request is signed for).
    region: &'static str,
    /// The `witnessRegion` the request carries.
    witness: &'static str,
    /// True when the witness names a Region other than the cluster's own.
    accepted: bool,
}

/// The whole class: equal-Region spellings (refused) beside Regions that genuinely
/// differ (accepted), across cluster Regions in several Region families.
fn witness_cases() -> Vec<WitnessCase> {
    vec![
        WitnessCase {
            name: "the cluster's own Region, exactly as signed",
            region: "us-east-1",
            witness: "us-east-1",
            accepted: false,
        },
        WitnessCase {
            name: "the cluster's own Region, upper-cased",
            region: "eu-west-2",
            witness: "EU-WEST-2",
            accepted: false,
        },
        WitnessCase {
            name: "the cluster's own Region, mixed case",
            region: "ap-southeast-2",
            witness: "Ap-SouthEast-2",
            accepted: false,
        },
        WitnessCase {
            name: "the cluster's own Region, padded with whitespace",
            region: "eu-central-1",
            witness: "  eu-central-1 ",
            accepted: false,
        },
        WitnessCase {
            name: "the cluster's own Region, a GovCloud-style name",
            region: "us-gov-west-1",
            witness: "us-gov-west-1",
            accepted: false,
        },
        WitnessCase {
            name: "another Region in the same family",
            region: "us-east-1",
            witness: "us-east-2",
            accepted: true,
        },
        WitnessCase {
            name: "another Region on another continent",
            region: "eu-west-2",
            witness: "ap-northeast-1",
            accepted: true,
        },
        WitnessCase {
            name: "a Region whose name extends the cluster's own",
            region: "us-west-2",
            witness: "us-west-20",
            accepted: true,
        },
        WitnessCase {
            name: "a Region whose name is a prefix of the cluster's own",
            region: "us-west-20",
            witness: "us-west-2",
            accepted: true,
        },
    ]
}

/// A peer cluster ARN in `region`, as `ClusterArn`'s pattern requires it.
fn peer_arn(region: &str) -> String {
    format!(
        "arn:aws:dsql:{region}:{}:cluster/aaaaaaaaaaaaaaaaaaaaaaaaaa",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The Region a peer cluster ARN names where the case is about the witness alone.
const PEER_REGION: &str = "ap-south-1";

/// Assert that `answer` is the modelled 400 `ValidationException` naming the
/// witness member.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a witness equal to the cluster's Region must be refused with 400, \
         got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer
            .error
            .as_deref()
            .is_some_and(|code| code.contains("ValidationException")),
        "case {case:?}: the wire code must be ValidationException, got {:?}",
        answer.error
    );
    assert!(
        !answer.message().is_empty(),
        "case {case:?}: the modelled @required message must be filled, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str("reason"),
        REASON_FIELD_VALIDATION_FAILED,
        "case {case:?}: a member-value refusal reports {REASON_FIELD_VALIDATION_FAILED}, got {:?}",
        answer.body
    );
    let field_list = answer.get("fieldList");
    let fields = field_list.as_array().map(Vec::as_slice).unwrap_or_default();
    assert!(
        fields.iter().any(|field| {
            field.get("name").and_then(Value::as_str) == Some(WITNESS_MEMBER)
                && field
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty())
        }),
        "case {case:?}: fieldList must name {WITNESS_MEMBER} with a message saying why, \
         got {field_list:?}"
    );
}

/// RULE: `CreateCluster` refuses a witness Region equal to the cluster's Region with
/// the modelled 400 and creates NOTHING; any other Region is accepted and the
/// created cluster reports it back.
#[tokio::test]
async fn create_cluster_refuses_a_witness_region_equal_to_the_cluster_region() {
    for case in witness_cases() {
        let dsql = Dsql::in_region(case.region);
        // The peer ARN's own Region is fixed and lowercase: `ClusterArn`'s pattern
        // (`...:dsql:[a-z0-9-]{1,20}:...`) is checked at the boundary, so a peer
        // built from a case's upper-cased witness would be refused there instead of
        // by the rule under test.
        let properties = json!({
            "witnessRegion": case.witness,
            "clusters": [peer_arn(PEER_REGION)],
        });
        let created = dsql
            .create(json!({
                "deletionProtectionEnabled": false,
                "multiRegionProperties": properties.clone(),
            }))
            .await;

        if !case.accepted {
            assert_refused(case.name, &created);
            // Nothing was created: the store is still empty.
            let listed = dsql.list("").await;
            let clusters = listed.get("clusters");
            assert_eq!(
                clusters.as_array().map(Vec::len).unwrap_or_default(),
                0,
                "case {:?}: a refused CreateCluster must create nothing, got {clusters:?}",
                case.name
            );
            continue;
        }

        assert_eq!(
            created.status, 200,
            "case {:?}: a witness naming another Region must be accepted, got {} {:?}",
            case.name, created.status, created.body
        );
        assert_eq!(
            created.get("multiRegionProperties"),
            properties,
            "case {:?}: CreateCluster must report the accepted multiRegionProperties, got {:?}",
            case.name,
            created.get("multiRegionProperties")
        );
        let fetched = dsql.get(&created.str("identifier")).await;
        assert_eq!(
            fetched.get("multiRegionProperties"),
            properties,
            "case {:?}: GetCluster must report the persisted multiRegionProperties, got {:?}",
            case.name,
            fetched.get("multiRegionProperties")
        );
    }
}

/// RULE: `UpdateCluster` refuses a witness Region equal to the updated cluster's
/// Region with the modelled 400 and leaves the cluster unchanged — not even moved to
/// `UPDATING`; any other Region is accepted and `GetCluster` reports it once the
/// clock settles the update.
#[tokio::test]
async fn update_cluster_refuses_a_witness_region_equal_to_the_cluster_region() {
    for case in witness_cases() {
        let dsql = Dsql::in_region(case.region);
        let identifier = dsql.create_settled().await;
        let properties = json!({"witnessRegion": case.witness});
        let updated = dsql
            .update(&identifier, json!({"multiRegionProperties": properties.clone()}))
            .await;

        if !case.accepted {
            assert_refused(case.name, &updated);
            let fetched = dsql.get(&identifier).await;
            assert_eq!(
                fetched.get("multiRegionProperties"),
                Value::Null,
                "case {:?}: a refused UpdateCluster must persist no multiRegionProperties, \
                 got {:?}",
                case.name,
                fetched.get("multiRegionProperties")
            );
            assert_eq!(
                fetched.str("status"),
                "ACTIVE",
                "case {:?}: a refused UpdateCluster must not move the cluster's status, got {:?}",
                case.name,
                fetched.str("status")
            );
            continue;
        }

        assert_eq!(
            updated.status, 200,
            "case {:?}: a witness naming another Region must be accepted, got {} {:?}",
            case.name, updated.status, updated.body
        );
        wait_for_the_clock().await;
        let fetched = dsql.get(&identifier).await;
        assert_eq!(
            fetched.get("multiRegionProperties"),
            properties,
            "case {:?}: GetCluster must report the persisted multiRegionProperties, got {:?}",
            case.name,
            fetched.get("multiRegionProperties")
        );
    }
}

/// RULE: the constraint is about the witness ALONE — a `multiRegionProperties` that
/// carries no `witnessRegion` names no witness, so it is accepted whatever Region
/// its peer clusters live in, including the cluster's own.
#[tokio::test]
async fn multi_region_properties_without_a_witness_region_are_accepted() {
    for region in ["us-east-1", "eu-west-1"] {
        for peer_region in [region, "us-west-2"] {
            let case = format!("cluster in {region}, peer in {peer_region}");
            let dsql = Dsql::in_region(region);
            let properties = json!({"clusters": [peer_arn(peer_region)]});
            let created = dsql
                .create(json!({
                    "deletionProtectionEnabled": false,
                    "multiRegionProperties": properties.clone(),
                }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: peers without a witness must be accepted, got {} {:?}",
                created.status, created.body
            );
            let fetched = dsql.get(&created.str("identifier")).await;
            assert_eq!(
                fetched.get("multiRegionProperties"),
                properties,
                "case {case:?}: GetCluster must report the persisted peers, got {:?}",
                fetched.get("multiRegionProperties")
            );
        }
    }
}

/// RULE: a refused `UpdateCluster` leaves the configuration that was already stored
/// in place — the refusal neither replaces nor clears it.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_configuration_alone() {
    let dsql = Dsql::in_region("us-east-1");
    let accepted = json!({"witnessRegion": "us-west-2", "clusters": [peer_arn("us-west-2")]});
    let created = dsql
        .create(json!({
            "deletionProtectionEnabled": false,
            "multiRegionProperties": accepted.clone(),
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture cluster must be created, got {} {:?}",
        created.status, created.body
    );
    let identifier = created.str("identifier");
    wait_for_the_clock().await;

    let refused = dsql
        .update(
            &identifier,
            json!({
                "deletionProtectionEnabled": true,
                "multiRegionProperties": {"witnessRegion": "us-east-1"},
            }),
        )
        .await;
    assert_refused("a self-referential witness over a stored configuration", &refused);

    let fetched = dsql.get(&identifier).await;
    assert_eq!(
        fetched.get("multiRegionProperties"),
        accepted,
        "a refused update must leave the stored multiRegionProperties alone, got {:?}",
        fetched.get("multiRegionProperties")
    );
    assert_eq!(
        fetched.get("deletionProtectionEnabled"),
        json!(false),
        "a refused update must apply none of its other members either, got {:?}",
        fetched.get("deletionProtectionEnabled")
    );
}

/// RULE: a `clientToken` does not launder the refusal — the same invalid request
/// sent twice is refused twice, and the token names no cluster afterwards, so a
/// LATER valid request may use it.
#[tokio::test]
async fn an_invalid_witness_is_refused_however_often_the_same_token_sends_it() {
    let dsql = Dsql::in_region("us-east-1");
    let identifier = dsql.create_settled().await;

    for attempt in 1..=2 {
        let refused = dsql
            .update(
                &identifier,
                json!({
                    "clientToken": "witness-token",
                    "multiRegionProperties": {"witnessRegion": "us-east-1"},
                }),
            )
            .await;
        assert_refused(&format!("attempt {attempt} with the same clientToken"), &refused);
    }

    let accepted = dsql
        .update(
            &identifier,
            json!({
                "clientToken": "witness-token",
                "multiRegionProperties": {"witnessRegion": "us-west-2"},
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "a token a refused request carried must still be usable, got {} {:?}",
        accepted.status, accepted.body
    );
}
