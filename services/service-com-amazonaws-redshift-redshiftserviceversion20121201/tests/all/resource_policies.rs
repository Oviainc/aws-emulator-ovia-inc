// SPDX-License-Identifier: CC0-1.0

//! The resource-policy lifecycle, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: a policy is held against the ARN it was attached to and reported
//! back VERBATIM, whatever the shape of the document and whichever Redshift
//! resource the ARN names; a second write to one resource REPLACING the document
//! it held and leaving every other resource's where it was; a delete making the
//! policy absent from a later read and freeing nothing else; a document that is
//! not a JSON policy document, or larger than this service holds under one
//! resource, being refused with nothing written; and an ARN that names no
//! resource, a resource of another service, or a Redshift resource that does not
//! exist, being refused by the write, the read and the delete alike.
//!
//! Tables rather than a generated corpus, for the reason the crate's
//! `Cargo.toml` records: a property-based generator would add an entry to the
//! workspace `Cargo.lock`, which the squash merge cannot resolve against a
//! mainline that rewrites it.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::Snapshot;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator, REGION};

/// Create one provisioned cluster, so the ARNs a row carries for it identify a
/// resource this account holds: a resource policy is attached TO a resource,
/// and an ARN that identifies none is refused by all three operations.
async fn seed_cluster(emulator: &Emulator, identifier: &str) {
    let created = emulator.create_minimal(identifier).await;
    assert_eq!(
        created.status, 200,
        "case seed the cluster {identifier}: expected 200, got {} with body {}",
        created.status, created.body
    );
}

/// Seed one snapshot of `cluster` straight into the shared store the handlers
/// read.
///
/// `CreateClusterSnapshot` takes a cluster that has finished provisioning, and
/// what these rows need is only that the snapshot ARN they carry identifies a
/// snapshot this account holds.
fn seed_snapshot(emulator: &Emulator, cluster: &str, snapshot: &str) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    store.snapshots.push(state::StoredSnapshot {
        snapshot: Snapshot {
            snapshot_identifier: Some(snapshot.to_string()),
            cluster_identifier: Some(cluster.to_string()),
            snapshot_arn: Some(state::snapshot_arn(cluster, snapshot, REGION)),
            status: Some("available".to_string()),
            snapshot_type: Some("manual".to_string()),
            ..Default::default()
        },
        // Already past, so the record needs no clock-owed transition.
        available_at: std::time::Instant::now(),
        source: None,
    });
}

/// `PutResourcePolicy` with one ARN and one document.
async fn put(emulator: &Emulator, resource_arn: &str, policy: &str) -> Answer {
    emulator
        .call(
            "PutResourcePolicy",
            &[("ResourceArn", resource_arn), ("Policy", policy)],
        )
        .await
}

/// `GetResourcePolicy` for one ARN.
async fn get(emulator: &Emulator, resource_arn: &str) -> Answer {
    emulator
        .call("GetResourcePolicy", &[("ResourceArn", resource_arn)])
        .await
}

/// `DeleteResourcePolicy` for one ARN.
async fn delete(emulator: &Emulator, resource_arn: &str) -> Answer {
    emulator
        .call("DeleteResourcePolicy", &[("ResourceArn", resource_arn)])
        .await
}

/// The document the store holds against one ARN, read straight from this
/// service's state rather than from a response.
fn stored_policy(emulator: &Emulator, resource_arn: &str) -> Option<String> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store.resource_policy(resource_arn).map(str::to_string)
}

/// Every ARN the store holds a policy against.
fn stored_arns(emulator: &Emulator) -> Vec<String> {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let store = store.lock();
    store.resource_policies.keys().cloned().collect()
}

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

/// The ARN of a Redshift Serverless namespace, the resource the published
/// zero-ETL authorization workflow attaches a policy to.
fn serverless_namespace_arn(uuid: &str) -> String {
    format!("arn:aws:redshift-serverless:{REGION}:000000000000:namespace/{uuid}")
}

/// A JSON policy document whose `Sid` is `sid`, so two rows of a table differ in
/// the document they carry without differing in its form.
fn policy_with_sid(sid: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"{sid}","Effect":"Allow","Principal":{{"Service":"redshift.amazonaws.com"}},"Action":"redshift:CreateInboundIntegration","Resource":"*"}}]}}"#
    )
}

/// A JSON policy document of exactly `len` characters, for the service's
/// document size bound.
fn policy_of_length(len: usize) -> String {
    let wrapper = r#"{"Sid":""}"#.chars().count();
    let document = format!(
        r#"{{"Sid":"{}"}}"#,
        std::iter::repeat_n('x', len - wrapper).collect::<String>()
    );
    assert_eq!(
        document.chars().count(),
        len,
        "the test's own document builder must produce exactly {len} characters"
    );
    document
}

/// One row of the round-trip table: the resource the policy is attached to and
/// the document attached to it.
struct PolicyCase {
    case: &'static str,
    resource_arn: String,
    policy: String,
}

/// The round-trip table: rows differing along everything the rule generalises
/// over — the Redshift resource the ARN names (a serverless namespace, a
/// provisioned cluster's namespace, a cluster, a snapshot, a resource portion
/// using `/` and one using `:`) and the shape of the document (an empty object,
/// one statement, several statements, a nested condition, text needing XML
/// escaping, non-ASCII text, a pretty-printed document whose whitespace is part
/// of its bytes, and a document at this service's size bound).
fn round_trip_table() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "an empty JSON object on a serverless namespace",
            resource_arn: serverless_namespace_arn("5533bed2-5c89-4049-91c3-77252462b84d"),
            policy: "{}".to_string(),
        },
        PolicyCase {
            case: "one statement on a serverless namespace",
            resource_arn: serverless_namespace_arn("949ecae8-a4ad-4824-98b3-67f95ac004e4"),
            policy: policy_with_sid("AllowIntegration"),
        },
        PolicyCase {
            case: "two statements on a provisioned cluster's namespace",
            resource_arn: state::namespace_arn("policy-holder", REGION),
            policy: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"000000000000"},"Action":"redshift:AuthorizeDataShare","Resource":"*"},{"Effect":"Deny","Principal":"*","Action":"redshift:DeauthorizeDataShare","Resource":"*"}]}"#.to_string(),
        },
        PolicyCase {
            case: "a nested condition on a cluster",
            resource_arn: state::cluster_arn("policy-holder", REGION),
            policy: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"redshift:GetClusterCredentials","Resource":"*","Condition":{"StringEquals":{"aws:SourceAccount":"000000000000"},"Null":{"aws:SourceArn":"false"}}}]}"#.to_string(),
        },
        PolicyCase {
            case: "text needing XML escaping on a snapshot",
            resource_arn: state::snapshot_arn("policy-holder", "snap-1", REGION),
            policy: r#"{"Sid":"a <b> & 'c' \"d\"","Statement":[]}"#.to_string(),
        },
        PolicyCase {
            case: "non-ASCII text",
            resource_arn: serverless_namespace_arn("bd0b0579-3e39-4169-8bc3-155c196c4ae8"),
            policy: r#"{"Sid":"政策 — naïve ✓ 🙂","Statement":[]}"#.to_string(),
        },
        PolicyCase {
            case: "a pretty-printed document whose whitespace is part of its bytes",
            resource_arn: serverless_namespace_arn("c5823534-ce1d-4cca-90bd-d34934091515"),
            policy: "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \"Effect\": \"Allow\",\n      \"Principal\": \"*\",\n      \"Action\": \"redshift:DescribeClusters\"\n    }\n  ]\n}"
                .to_string(),
        },
        PolicyCase {
            case: "a document of exactly the size this service holds",
            resource_arn: serverless_namespace_arn("cc5eb723-844c-4749-8f11-ee554437b488"),
            policy: policy_of_length(state::MAX_RESOURCE_POLICY_LEN),
        },
    ]
}

/// The document a write attaches is reported back verbatim, by the write itself
/// and by a later read, and is held against the ARN the write named.
#[tokio::test]
async fn a_put_policy_is_reported_back_verbatim_by_the_put_and_by_a_later_get() {
    let emulator = Emulator::new();
    seed_cluster(&emulator, "policy-holder").await;
    seed_snapshot(&emulator, "policy-holder", "snap-1");

    for row in round_trip_table() {
        let written = put(&emulator, &row.resource_arn, &row.policy).await;
        assert_eq!(
            written.status, 200,
            "case {}: expected status 200, got {} with body {}",
            row.case, written.status, written.body
        );
        assert_eq!(
            written.text("ResourceArn").as_deref(),
            Some(row.resource_arn.as_str()),
            "case {}: expected the written policy to report the ARN it was attached to, got {:?}",
            row.case,
            written.text("ResourceArn")
        );
        assert_eq!(
            written.text("Policy").as_deref(),
            Some(row.policy.as_str()),
            "case {}: expected the write to report the document verbatim, got {:?}",
            row.case,
            written.text("Policy")
        );

        let read = get(&emulator, &row.resource_arn).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected the read to answer 200, got {} with body {}",
            row.case, read.status, read.body
        );
        assert_eq!(
            read.text("Policy").as_deref(),
            Some(row.policy.as_str()),
            "case {}: expected the read to report the document verbatim, got {:?}",
            row.case,
            read.text("Policy")
        );
        assert_eq!(
            read.text("ResourceArn").as_deref(),
            Some(row.resource_arn.as_str()),
            "case {}: expected the read to report the ARN the policy is attached to, got {:?}",
            row.case,
            read.text("ResourceArn")
        );
        assert_eq!(
            stored_policy(&emulator, &row.resource_arn).as_deref(),
            Some(row.policy.as_str()),
            "case {}: expected the store to hold the document verbatim, got {:?}",
            row.case,
            stored_policy(&emulator, &row.resource_arn)
        );
    }

    // Every row attached its policy to its OWN resource: one write never moved
    // another resource's document.
    let attached = stored_arns(&emulator);
    for row in round_trip_table() {
        assert!(
            attached.contains(&row.resource_arn),
            "case {}: expected the store to hold a policy against {}, holding {attached:?}",
            row.case,
            row.resource_arn
        );
    }
    assert_eq!(
        attached.len(),
        round_trip_table().len(),
        "case \"one policy per resource\": expected {} attached policies, got {attached:?}",
        round_trip_table().len()
    );
}

/// A second write to one resource REPLACES the document it held: a resource
/// carries at most one policy, and the resources around it keep theirs.
#[tokio::test]
async fn a_second_put_replaces_the_document_of_that_resource_alone() {
    let emulator = Emulator::new();
    seed_cluster(&emulator, "untouched").await;
    let holder = serverless_namespace_arn("575d91fc-611c-44e9-9107-a5afb4a50b3c");
    let neighbour = state::namespace_arn("untouched", REGION);
    let neighbour_policy = policy_with_sid("Neighbour");

    let sequence = [
        ("an empty object first", "{}".to_string()),
        ("then one statement", policy_with_sid("First")),
        ("then another statement", policy_with_sid("Second")),
        (
            "then a document of another shape",
            r#"{"Version":"2012-10-17","Statement":[]}"#.to_string(),
        ),
        ("then an empty object again", "{}".to_string()),
    ];

    assert_eq!(
        put(&emulator, &neighbour, &neighbour_policy).await.status,
        200,
        "case \"seed the neighbouring resource\": expected status 200"
    );

    for (case, policy) in &sequence {
        let written = put(&emulator, &holder, policy).await;
        assert_eq!(
            written.status, 200,
            "case {case}: expected status 200, got {} with body {}",
            written.status, written.body
        );
        let read = get(&emulator, &holder).await;
        assert_eq!(
            read.text("Policy").as_deref(),
            Some(policy.as_str()),
            "case {case}: expected the read to report the document the last write attached, got {:?}",
            read.text("Policy")
        );
        assert_eq!(
            stored_policy(&emulator, &holder).as_deref(),
            Some(policy.as_str()),
            "case {case}: expected the store to hold one document for the resource, got {:?}",
            stored_policy(&emulator, &holder)
        );
        assert_eq!(
            stored_arns(&emulator),
            vec![neighbour.clone(), holder.clone()]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>(),
            "case {case}: expected a replacement to attach no second policy"
        );
    }

    assert_eq!(
        stored_policy(&emulator, &neighbour).as_deref(),
        Some(neighbour_policy.as_str()),
        "case \"the neighbouring resource\": expected its own document to be untouched, got {:?}",
        stored_policy(&emulator, &neighbour)
    );
}

/// A delete detaches the policy of the resource it names: a later read reports
/// no policy, a second delete has none to detach, and every other resource keeps
/// its own.
#[tokio::test]
async fn a_delete_detaches_the_policy_of_that_resource_alone() {
    let emulator = Emulator::new();
    seed_cluster(&emulator, "second-holder").await;
    seed_cluster(&emulator, "third-holder").await;
    let holders = [
        serverless_namespace_arn("5533bed2-5c89-4049-91c3-77252462b84d"),
        state::namespace_arn("second-holder", REGION),
        state::cluster_arn("third-holder", REGION),
    ];

    for holder in &holders {
        assert_eq!(
            put(&emulator, holder, &policy_with_sid("Attached")).await.status,
            200,
            "case \"seed {holder}\": expected status 200"
        );
    }

    for (position, holder) in holders.iter().enumerate() {
        let detached = delete(&emulator, holder).await;
        assert_eq!(
            detached.status, 200,
            "case \"delete {holder}\": expected status 200, got {} with body {}",
            detached.status, detached.body
        );
        assert_eq!(
            stored_policy(&emulator, holder),
            None,
            "case \"delete {holder}\": expected the store to hold no document for it, got {:?}",
            stored_policy(&emulator, holder)
        );

        let read = get(&emulator, holder).await;
        assert_refused(
            &format!("read {holder} after the delete"),
            &read,
            404,
            "ResourceNotFoundFault",
        );
        assert_eq!(
            read.text("Message").as_deref(),
            Some(state::RESOURCE_POLICY_NOT_FOUND),
            "case \"read {holder} after the delete\": expected the missing-policy message, got {:?}",
            read.text("Message")
        );

        let again = delete(&emulator, holder).await;
        assert_refused(
            &format!("delete {holder} twice"),
            &again,
            404,
            "ResourceNotFoundFault",
        );
        assert_eq!(
            again.text("Message").as_deref(),
            Some(state::RESOURCE_POLICY_NOT_FOUND),
            "case \"delete {holder} twice\": expected the missing-policy message, got {:?}",
            again.text("Message")
        );

        // Only the resources already deleted lost their policy.
        for still_attached in holders.iter().skip(position + 1) {
            assert!(
                stored_policy(&emulator, still_attached).is_some(),
                "case \"delete {holder}\": expected {still_attached} to keep its own document"
            );
        }
    }
}

/// A document that is not a JSON policy document, or one larger than this
/// service holds under a resource, is refused and nothing is written.
#[tokio::test]
async fn a_document_that_is_no_policy_document_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let holder = serverless_namespace_arn("5533bed2-5c89-4049-91c3-77252462b84d");
    let kept = policy_with_sid("Kept");

    assert_eq!(
        put(&emulator, &holder, &kept).await.status,
        200,
        "case \"seed the resource\": expected status 200"
    );

    let table = [
        ("plain text", "not a policy".to_string()),
        ("an unterminated object", r#"{"Statement":"#.to_string()),
        ("a JSON array", r#"[{"Effect":"Allow"}]"#.to_string()),
        ("a JSON string", r#""a policy""#.to_string()),
        ("a JSON number", "12".to_string()),
        ("JSON null", "null".to_string()),
        ("two documents in one value", "{}{}".to_string()),
        (
            "one character past the size this service holds",
            policy_of_length(state::MAX_RESOURCE_POLICY_LEN + 1),
        ),
    ];

    for (case, policy) in &table {
        let refused = put(&emulator, &holder, policy).await;
        assert_refused(case, &refused, 400, "InvalidPolicyFault");
        if *case != "one character past the size this service holds" {
            assert_eq!(
                refused.text("Message").as_deref(),
                Some(state::RESOURCE_POLICY_NOT_A_POLICY_DOCUMENT),
                "case {case}: expected the parse-failure sentence, got {:?}",
                refused.text("Message")
            );
        }
        assert_eq!(
            stored_policy(&emulator, &holder).as_deref(),
            Some(kept.as_str()),
            "case {case}: expected the refusal to leave the attached document alone, got {:?}",
            stored_policy(&emulator, &holder)
        );
    }
}

/// An ARN that names no resource, a resource of another service, or a Redshift
/// resource that does not exist is refused by the write, the read and the delete
/// alike — and nothing is attached.
#[tokio::test]
async fn an_arn_naming_no_policy_holder_is_refused_by_every_resource_policy_operation() {
    let emulator = Emulator::new();
    seed_cluster(&emulator, "policy-holder").await;

    // A value that is no ARN at all names no resource: `ResourceNotFoundFault`.
    let not_resources = [
        ("a bare name", "policy-holder".to_string()),
        (
            "a resource portion without the ARN prefix",
            format!("redshift:{REGION}:000000000000:namespace/one"),
        ),
        (
            "an ARN missing its resource portion",
            format!("arn:aws:redshift:{REGION}:000000000000:"),
        ),
        (
            "an ARN with no service",
            format!("arn:aws::{REGION}:000000000000:namespace/one"),
        ),
        (
            "a value longer than an ARN can be",
            format!(
                "arn:aws:redshift:{REGION}:000000000000:namespace/{}",
                std::iter::repeat_n('n', state::MAX_ARN_LENGTH).collect::<String>()
            ),
        ),
    ];
    for (case, resource_arn) in &not_resources {
        assert_refused(
            case,
            &put(&emulator, resource_arn, &policy_with_sid("Refused")).await,
            404,
            "ResourceNotFoundFault",
        );
        assert_refused(
            &format!("{case}, read"),
            &get(&emulator, resource_arn).await,
            404,
            "ResourceNotFoundFault",
        );
        assert_refused(
            &format!("{case}, delete"),
            &delete(&emulator, resource_arn).await,
            404,
            "ResourceNotFoundFault",
        );
    }

    // An ARN of another service names a resource that carries no Amazon Redshift
    // resource policy: `UnsupportedOperationFault`, whose awsQuery code is
    // `UnsupportedOperation`.
    let other_services = [
        (
            "an S3 bucket",
            "arn:aws:s3:::a-bucket".to_string(),
        ),
        (
            "a DynamoDB table",
            format!("arn:aws:dynamodb:{REGION}:000000000000:table/a-table"),
        ),
        (
            "an IAM role",
            "arn:aws:iam::000000000000:role/a-role".to_string(),
        ),
        (
            "a Glue catalog",
            format!("arn:aws:glue:{REGION}:000000000000:catalog"),
        ),
    ];
    for (case, resource_arn) in &other_services {
        assert_refused(
            case,
            &put(&emulator, resource_arn, &policy_with_sid("Refused")).await,
            400,
            "UnsupportedOperation",
        );
        assert_refused(
            &format!("{case}, read"),
            &get(&emulator, resource_arn).await,
            400,
            "UnsupportedOperation",
        );
        assert_refused(
            &format!("{case}, delete"),
            &delete(&emulator, resource_arn).await,
            400,
            "UnsupportedOperation",
        );
    }

    // A well-formed Redshift ARN whose resource does not exist identifies no
    // policy holder either: the up-front parameter refusal,
    // `InvalidParameterValue`, decided before any policy state is touched. The
    // rows span the resource types an account's ARNs name — a datashare of a
    // namespace nothing created, a cluster, a cluster's namespace, a snapshot of
    // a cluster that exists and one of a cluster that does not.
    let unresolvable = [
        (
            "a datashare of a namespace nothing created",
            format!(
                "arn:aws:redshift:{REGION}:000000000000:datashare:3fd3f0cf-11bd-4fd0-9c4b-1c0f2c4b1a4f/sales"
            ),
        ),
        (
            "a cluster nothing created",
            state::cluster_arn("no-such-cluster", REGION),
        ),
        (
            "the namespace of a cluster nothing created",
            state::namespace_arn("no-such-cluster", REGION),
        ),
        (
            "a snapshot nothing took of a cluster that exists",
            state::snapshot_arn("policy-holder", "no-such-snapshot", REGION),
        ),
        (
            "a snapshot of a cluster nothing created",
            state::snapshot_arn("no-such-cluster", "snap-1", REGION),
        ),
        (
            "a cluster of this account in another Region",
            state::cluster_arn("policy-holder", "eu-west-1"),
        ),
    ];
    for (case, resource_arn) in &unresolvable {
        assert_refused(
            case,
            &put(&emulator, resource_arn, &policy_with_sid("Refused")).await,
            400,
            "InvalidParameterValue",
        );
        assert_refused(
            &format!("{case}, read"),
            &get(&emulator, resource_arn).await,
            400,
            "InvalidParameterValue",
        );
        assert_refused(
            &format!("{case}, delete"),
            &delete(&emulator, resource_arn).await,
            400,
            "InvalidParameterValue",
        );
    }

    assert_eq!(
        stored_arns(&emulator),
        Vec::<String>::new(),
        "case \"a refused write\": expected the store to hold no policy at all"
    );
}

/// A resource that was never written to carries no policy: the read and the
/// delete say so rather than reporting an empty document.
#[tokio::test]
async fn a_resource_that_carries_no_policy_has_none_to_report_or_detach() {
    let emulator = Emulator::new();
    seed_cluster(&emulator, "never-written").await;
    seed_snapshot(&emulator, "never-written", "snap-1");
    let unwritten = [
        serverless_namespace_arn("00000000-1111-2222-3333-444444444444"),
        state::namespace_arn("never-written", REGION),
        state::cluster_arn("never-written", REGION),
        state::snapshot_arn("never-written", "snap-1", REGION),
    ];

    for resource_arn in &unwritten {
        let read = get(&emulator, resource_arn).await;
        assert_refused(
            &format!("read {resource_arn}"),
            &read,
            404,
            "ResourceNotFoundFault",
        );
        assert_eq!(
            read.text("Message").as_deref(),
            Some(state::RESOURCE_POLICY_NOT_FOUND),
            "case \"read {resource_arn}\": expected the missing-policy message, got {:?}",
            read.text("Message")
        );
        assert_refused(
            &format!("delete {resource_arn}"),
            &delete(&emulator, resource_arn).await,
            404,
            "ResourceNotFoundFault",
        );
    }
}

/// A store already holding as many policies as this emulator keeps takes no
/// further HOLDER, while a resource already holding one can still have its
/// document replaced: the bound is on how many resources carry a policy, not on
/// how often one is written.
#[tokio::test]
async fn a_write_past_the_policies_this_emulator_holds_is_refused() {
    let emulator = Emulator::new();
    let holder = serverless_namespace_arn("5533bed2-5c89-4049-91c3-77252462b84d");

    // Seeded straight into the shared store the handlers read: reaching the
    // bound through the request path would be the same assertion with thousands
    // of requests in front of it.
    {
        let store = state::service_state(&*emulator.engine).expect("the Redshift store");
        let mut store = store.lock();
        store.set_resource_policy(&holder, policy_with_sid("Seeded"));
        for filler in 1..state::RESOURCE_POLICY_QUOTA {
            store.set_resource_policy(
                &format!("arn:aws:redshift:{REGION}:000000000000:namespace:seed-{filler}"),
                "{}".to_string(),
            );
        }
    }

    let refused = put(
        &emulator,
        &serverless_namespace_arn("949ecae8-a4ad-4824-98b3-67f95ac004e4"),
        &policy_with_sid("OneTooMany"),
    )
    .await;
    assert_refused(
        "one policy holder past the bound",
        &refused,
        400,
        "UnsupportedOperation",
    );
    assert_eq!(
        refused.text("Message").as_deref(),
        Some(state::resource_policy_quota_message().as_str()),
        "case \"one policy holder past the bound\": expected the emulator-limit message, got {:?}",
        refused.text("Message")
    );
    assert_eq!(
        stored_arns(&emulator).len(),
        state::RESOURCE_POLICY_QUOTA,
        "case \"one policy holder past the bound\": expected the refusal to attach nothing"
    );

    // A resource that already carries a policy is no new holder, so replacing
    // its document is served at the bound.
    let replaced = put(&emulator, &holder, &policy_with_sid("Replaced")).await;
    assert_eq!(
        replaced.status, 200,
        "case \"replace a document at the bound\": expected status 200, got {} with body {}",
        replaced.status, replaced.body
    );
    assert_eq!(
        stored_policy(&emulator, &holder).as_deref(),
        Some(policy_with_sid("Replaced").as_str()),
        "case \"replace a document at the bound\": expected the new document to be held, got {:?}",
        stored_policy(&emulator, &holder)
    );
}
