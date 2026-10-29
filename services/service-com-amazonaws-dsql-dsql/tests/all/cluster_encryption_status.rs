// SPDX-License-Identifier: CC0-1.0

//! What `encryptionDetails.encryptionStatus` reports over the life of a cluster,
//! end to end over the wire.
//!
//! RULE: `encryptionStatus` reports how far the cluster's data is from being
//! protected by the encryption configuration the caller asked for, not that it
//! is in place: a configuration that still has to be put in place is reported as
//! arriving — `ENABLING` for a customer-managed key being established for a new
//! cluster, `UPDATING` for a change from the configuration the cluster already
//! had — and reaches `ENABLED` only when the clock settles it; a cluster created
//! on the key owned by Amazon Web Services has nothing to establish, so it
//! reports `ENABLED` from the create's own answer on, and a call that leaves the
//! configuration (the encryption type and its key) as it was leaves the status
//! alone, whatever else it changes about the cluster.
//!
//! The tables below span the class the rule generalises over: every form a
//! `kmsEncryptionKey` takes on the way IN (absent, the reserved Amazon-owned
//! value, a bare key id, an alias, a full ARN), and every kind of transition
//! between two configurations on the way THROUGH (onto a key, between two keys,
//! back to the Amazon-owned key, and the three calls that change no
//! configuration at all).
//!
//! Every call goes through the `Dsql` harness in `tests/all/harness.rs`, this
//! crate's one test client.

use serde_json::{json, Value};

use crate::harness::{wait_for_the_clock, Answer, Dsql};

/// The `EncryptionStatus` values a cluster of this emulator reports.
///
/// Source: `smithy_query(query="shape", shape="EncryptionStatus")`, whose values
/// are `ENABLED | UPDATING | KMS_KEY_INACCESSIBLE | ENABLING`.
const ENABLED: &str = "ENABLED";
/// See [`ENABLED`]: the cluster's first configuration is still being established.
const ENABLING: &str = "ENABLING";
/// See [`ENABLED`]: a change to another configuration is in flight.
const UPDATING: &str = "UPDATING";

/// The reserved `kmsEncryptionKey` value that selects the key owned by Amazon Web
/// Services. Source: `smithy_query(query="doc", shape="UpdateClusterInput",
/// member="kmsEncryptionKey")`.
const RESERVED_AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";

/// The `ClusterStatus` a cluster reports while a reconfiguration of it is in
/// flight. Source: `smithy_query(query="shape", shape="ClusterStatus")`.
const CLUSTER_UPDATING: &str = "UPDATING";
/// See [`CLUSTER_UPDATING`]: the status of a cluster being created.
const CLUSTER_CREATING: &str = "CREATING";

/// The `encryptionStatus` a response reports, or the empty string when the
/// response carries no `encryptionDetails` at all.
fn encryption_status(answer: &Answer) -> String {
    answer
        .get("encryptionDetails")
        .get("encryptionStatus")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `kmsKeyArn` a response reports, if it reports one.
fn key_arn(answer: &Answer) -> Option<String> {
    answer
        .get("encryptionDetails")
        .get("kmsKeyArn")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One form a `kmsEncryptionKey` arrives in, for the create table.
struct CreateCase {
    name: &'static str,
    /// The `kmsEncryptionKey` the request carries, or `None` for a request that
    /// carries no such member.
    key: Option<&'static str>,
    /// The `encryptionStatus` the cluster reports while it is being created: a
    /// key of the caller's own has to be established for the cluster, the
    /// Amazon-owned one is in place before the cluster exists.
    want_initial: &'static str,
}

/// Every form the member admits, including its absence and the reserved value,
/// which are the two spellings of the Amazon-owned key.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            name: "no kmsEncryptionKey at all",
            key: None,
            want_initial: ENABLED,
        },
        CreateCase {
            name: "the reserved Amazon-owned value",
            key: Some(RESERVED_AWS_OWNED),
            want_initial: ENABLED,
        },
        CreateCase {
            name: "a bare key id",
            key: Some("1234abcd-12ab-34cd-56ef"),
            want_initial: ENABLING,
        },
        CreateCase {
            name: "an alias",
            key: Some("alias/finance"),
            want_initial: ENABLING,
        },
        CreateCase {
            name: "a full key ARN",
            key: Some("arn:aws:kms:eu-west-1:000000000000:key/9f0abc"),
            want_initial: ENABLING,
        },
    ]
}

/// RULE on the way in: a customer-managed key a new cluster is being given is
/// `ENABLING` while the cluster is being created and `ENABLED` once the clock has
/// settled it, while the Amazon-owned key — which nothing has to establish — is
/// `ENABLED` from the create's own answer on, even though the cluster itself is
/// still `CREATING`.
#[tokio::test]
async fn a_new_cluster_reports_a_key_it_still_has_to_establish_as_enabling() {
    for case in create_cases() {
        let dsql = Dsql::new();
        let mut body = json!({"deletionProtectionEnabled": false});
        if let Some(key) = case.key {
            body["kmsEncryptionKey"] = json!(key);
        }
        let created = dsql.create(body).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateCluster must answer 200, got {} {:?}",
            case.name, created.status, created.body
        );
        assert_eq!(
            created.str("status"),
            CLUSTER_CREATING,
            "case {:?}: the cluster must still be being created, got {:?}",
            case.name,
            created.body
        );
        assert_eq!(
            encryption_status(&created),
            case.want_initial,
            "case {:?}: encryptionStatus CreateCluster reports, got {:?}",
            case.name,
            created.get("encryptionDetails")
        );

        let identifier = created.str("identifier");
        let read = dsql.get(&identifier).await;
        assert_eq!(
            encryption_status(&read),
            case.want_initial,
            "case {:?}: GetCluster must report what the create answered, got {:?}",
            case.name,
            read.get("encryptionDetails")
        );
        // A read reports; it does not move the configuration on.
        let again = dsql.get(&identifier).await;
        assert_eq!(
            encryption_status(&again),
            case.want_initial,
            "case {:?}: a second read must report the same status, got {:?}",
            case.name,
            again.get("encryptionDetails")
        );

        wait_for_the_clock().await;
        let settled = dsql.get(&identifier).await;
        assert_eq!(
            encryption_status(&settled),
            ENABLED,
            "case {:?}: the settled cluster must report encryption as in place, got {:?}",
            case.name,
            settled.get("encryptionDetails")
        );
        assert_eq!(
            key_arn(&settled),
            key_arn(&created),
            "case {:?}: settling the status must not change the key, got {:?}",
            case.name,
            settled.get("encryptionDetails")
        );
    }
}

/// One transition between two encryption configurations.
struct UpdateCase {
    name: &'static str,
    /// The `kmsEncryptionKey` the cluster is created with, if any.
    from: Option<&'static str>,
    /// The body the `UpdateCluster` call carries.
    update: fn() -> Value,
    /// The `encryptionStatus` reported while the call's change is in flight.
    want_in_flight: &'static str,
    /// Whether the update must leave the reported `kmsKeyArn` where it was.
    key_arn_unchanged: bool,
}

/// Every kind of transition: onto a customer-managed key, between two of them,
/// back to the Amazon-owned key, and the three calls that ask for no change of
/// configuration (the same key again, the Amazon-owned key a cluster is already
/// on, and an update that carries no key at all).
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            name: "the Amazon-owned key onto a customer-managed one",
            from: None,
            update: || json!({"kmsEncryptionKey": "alias/finance"}),
            want_in_flight: UPDATING,
            key_arn_unchanged: false,
        },
        UpdateCase {
            name: "one customer-managed key onto another",
            from: Some("alias/finance"),
            update: || json!({"kmsEncryptionKey": "1234abcd-12ab-34cd-56ef"}),
            want_in_flight: UPDATING,
            key_arn_unchanged: false,
        },
        UpdateCase {
            name: "a customer-managed key onto a key ARN in another region",
            from: Some("alias/finance"),
            update: || json!({"kmsEncryptionKey": "arn:aws:kms:ap-south-1:000000000000:key/9f0abc"}),
            want_in_flight: UPDATING,
            key_arn_unchanged: false,
        },
        UpdateCase {
            name: "a customer-managed key back to the Amazon-owned one",
            from: Some("alias/finance"),
            update: || json!({"kmsEncryptionKey": RESERVED_AWS_OWNED}),
            want_in_flight: UPDATING,
            key_arn_unchanged: false,
        },
        UpdateCase {
            name: "the very key the cluster is already encrypted with",
            from: Some("alias/finance"),
            update: || json!({"kmsEncryptionKey": "alias/finance"}),
            want_in_flight: ENABLED,
            key_arn_unchanged: true,
        },
        UpdateCase {
            name: "the Amazon-owned key a cluster is already on",
            from: None,
            update: || json!({"kmsEncryptionKey": RESERVED_AWS_OWNED}),
            want_in_flight: ENABLED,
            key_arn_unchanged: true,
        },
        UpdateCase {
            name: "another member entirely, which touches no configuration",
            from: Some("alias/finance"),
            update: || json!({"deletionProtectionEnabled": true}),
            want_in_flight: ENABLED,
            key_arn_unchanged: true,
        },
    ]
}

/// RULE on the way through: a change of configuration reports `UPDATING` for the
/// window it is in flight — the same window the cluster's own status is
/// `UPDATING` — and `ENABLED` after it; a call that changes no configuration
/// reports `ENABLED` throughout, even while the cluster itself is `UPDATING`.
#[tokio::test]
async fn a_key_change_reports_updating_until_the_re_encryption_completes() {
    for case in update_cases() {
        let dsql = Dsql::new();
        let mut body = json!({"deletionProtectionEnabled": false});
        if let Some(key) = case.from {
            body["kmsEncryptionKey"] = json!(key);
        }
        let created = dsql.create(body).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateCluster must answer 200, got {} {:?}",
            case.name, created.status, created.body
        );
        let identifier = created.str("identifier");
        wait_for_the_clock().await;
        let before = dsql.get(&identifier).await;
        assert_eq!(
            encryption_status(&before),
            ENABLED,
            "case {:?}: the cluster must start from a configuration in place, got {:?}",
            case.name,
            before.get("encryptionDetails")
        );

        let updated = dsql.update(&identifier, (case.update)()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: UpdateCluster must answer 200, got {} {:?}",
            case.name, updated.status, updated.body
        );
        assert_eq!(
            updated.str("status"),
            CLUSTER_UPDATING,
            "case {:?}: the reconfigured cluster must report UPDATING, got {:?}",
            case.name,
            updated.body
        );

        let in_flight = dsql.get(&identifier).await;
        assert_eq!(
            in_flight.str("status"),
            CLUSTER_UPDATING,
            "case {:?}: the cluster must still be reconfiguring, got {:?}",
            case.name,
            in_flight.body
        );
        assert_eq!(
            encryption_status(&in_flight),
            case.want_in_flight,
            "case {:?}: encryptionStatus while the call's change is in flight, got {:?}",
            case.name,
            in_flight.get("encryptionDetails")
        );
        // Reporting it does not finish it: a second read says the same.
        let again = dsql.get(&identifier).await;
        assert_eq!(
            encryption_status(&again),
            case.want_in_flight,
            "case {:?}: a second read must report the same status, got {:?}",
            case.name,
            again.get("encryptionDetails")
        );
        if case.key_arn_unchanged {
            assert_eq!(
                key_arn(&in_flight),
                key_arn(&before),
                "case {:?}: a call that changes no configuration must leave the key, got {:?}",
                case.name,
                in_flight.get("encryptionDetails")
            );
        } else {
            assert_ne!(
                key_arn(&in_flight),
                key_arn(&before),
                "case {:?}: the change must report the configuration it asked for, got {:?}",
                case.name,
                in_flight.get("encryptionDetails")
            );
        }

        wait_for_the_clock().await;
        let settled = dsql.get(&identifier).await;
        assert_eq!(
            encryption_status(&settled),
            ENABLED,
            "case {:?}: the completed change must report encryption as in place, got {:?}",
            case.name,
            settled.get("encryptionDetails")
        );
        assert_eq!(
            key_arn(&settled),
            key_arn(&in_flight),
            "case {:?}: settling the status must not change the key, got {:?}",
            case.name,
            settled.get("encryptionDetails")
        );
    }
}

/// RULE as an invariant over a sequence: however many configurations a cluster is
/// driven through, every change is reported as `UPDATING` first and `ENABLED`
/// only once it has completed, so a client that polls always sees the change it
/// asked for finish.
#[tokio::test]
async fn every_configuration_change_in_a_sequence_is_polled_to_completion() {
    // The sequence crosses between the two encryption types in both directions
    // and repeats a value, so a change and a no-change alternate.
    let steps: &[(&str, &str)] = &[
        ("alias/one", UPDATING),
        ("alias/one", ENABLED),
        ("two", UPDATING),
        (RESERVED_AWS_OWNED, UPDATING),
        (RESERVED_AWS_OWNED, ENABLED),
        ("arn:aws:kms:us-east-1:000000000000:key/three", UPDATING),
    ];

    let dsql = Dsql::new();
    let identifier = dsql.create_settled().await;
    assert_eq!(
        encryption_status(&dsql.get(&identifier).await),
        ENABLED,
        "a settled cluster starts from a configuration in place"
    );

    for (step, (key, want_in_flight)) in steps.iter().enumerate() {
        let updated = dsql
            .update(&identifier, json!({"kmsEncryptionKey": key}))
            .await;
        assert_eq!(
            updated.status, 200,
            "step {step} ({key:?}): UpdateCluster must answer 200, got {} {:?}",
            updated.status, updated.body
        );
        let in_flight = dsql.get(&identifier).await;
        assert_eq!(
            encryption_status(&in_flight),
            *want_in_flight,
            "step {step} ({key:?}): encryptionStatus while the change is in flight, got {:?}",
            in_flight.get("encryptionDetails")
        );

        wait_for_the_clock().await;
        let settled = dsql.get(&identifier).await;
        assert_eq!(
            encryption_status(&settled),
            ENABLED,
            "step {step} ({key:?}): the completed change must report ENABLED, got {:?}",
            settled.get("encryptionDetails")
        );
    }
}
