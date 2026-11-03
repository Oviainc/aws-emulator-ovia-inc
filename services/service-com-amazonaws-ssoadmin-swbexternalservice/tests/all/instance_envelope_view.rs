// SPDX-License-Identifier: CC0-1.0

//! The `Instance` ENVELOPE `DescribeInstance` serves beside its flat members, over the
//! `awsJson1_1` wire.
//!
//! THE RULE: a reported Identity Center instance carries the service's instance VIEW — the
//! projection an enumeration serves for it, `State` included — and `DescribeInstance`
//! serves that view as an `Instance` member beside the flat members it reports, both read
//! from the one stored record, so the two can never disagree about the instance. Neither
//! the envelope nor the `State` structure on it is in the on-disk model
//! (`smithy_query(query="shape",shape="DescribeInstanceResponse,InstanceMetadata")` lists
//! neither), so the member set under test is the one the service serves: the enumerated
//! entry's own members.
//!
//! The rows span what the rule generalises over: the lifecycle PHASE the instance is in
//! (creation in flight, activation due, deletion in flight, and the `CREATE_FAILED` value
//! no call of this API reaches — the admissible values are not written out by hand but
//! taken from the model-generated `ops::types::INSTANCE_STATUS_VALUES`), the KIND of
//! instance (the account's ambient instance, an instance `CreateInstance` minted, an
//! organization instance), the REGION the instance is enabled in, HOW MANY instances the
//! account's store holds when one of them is described, and repeated reads of one
//! instance.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns. Where a phase lives on the far side of a due instant
//! the test moves the STORED instant rather than sleeping — the transition is still applied
//! by the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::INSTANCE_STATUS_VALUES;
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_create::hold_activation;
use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{
    call_in_region, hold_deletion, organization_engine, router_over, with_store, TEST_REGION,
};

/// The flat members `DescribeInstance` and the instance view BOTH report, so the envelope
/// can be checked against the response it travels in as well as against the enumeration.
///
/// These are exactly the members `InstanceMetadata` declares in the model
/// (`smithy_query(query="shape",shape="com.amazonaws.ssoadmin#InstanceMetadata")`), whose
/// values a `DescribeInstanceResponse` repeats.
const SHARED_MEMBERS: &[&str] = &[
    "InstanceArn",
    "IdentityStoreId",
    "IdentityStoreArn",
    "OwnerAccountId",
    "Name",
    "CreatedDate",
    "Status",
    "PrimaryRegion",
    "Regions",
];

/// A router over an account that holds NO instance yet, so `CreateInstance` is the call
/// that enables the account's one instance — and the engine behind it, for the stored due
/// instants a phase has to be moved past.
fn empty_account() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| {
        store.instances.clear();
        store.ambient_instance_enabled = true;
    });
    (router_over(Arc::clone(&engine)), engine)
}

/// The body `DescribeInstance` answers for `instance_arn` in `region`.
async fn described(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    instance_arn: &str,
) -> Value {
    let (status, body) = call_in_region(
        router,
        region,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstance expected 200, got {status} {body}"
    );
    body
}

/// THE RULE, asserted on one reported instance: the response carries an `Instance`
/// envelope which IS the view the enumeration serves for that same instance — member for
/// member, `State` included — and whose members agree with the response's own flat ones.
async fn assert_envelope_is_the_served_view(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    instance_arn: &str,
) -> Value {
    let body = described(case, router, region, instance_arn).await;
    let envelope = body.get("Instance").unwrap_or_else(|| {
        panic!("case {case:?}: the response carries no Instance envelope at all: {body}")
    });
    assert!(
        envelope.is_object(),
        "case {case:?}: Instance is a structure, got {envelope:?}"
    );

    // The envelope's lifecycle view: `State` is a structure whose `Name` is the envelope's
    // own `Status`, and that status is a value the model's enum declares.
    assert_state_mirrors_status(case, envelope);

    // One record, two projections: every member the flat response and the view share
    // carries the same value.
    for member in SHARED_MEMBERS {
        assert_eq!(
            envelope[*member], body[*member],
            "case {case:?}: envelope member {member} must repeat the reported value, got \
             {:?} vs {:?}",
            envelope[*member], body[*member]
        );
    }

    // And the view is the one an enumeration serves for this instance, so a client that
    // waits on `State` through either call sees the same structure.
    let entries = listed(case, router, region, None).await;
    let listed_entry = entries
        .iter()
        .find(|entry| entry["InstanceArn"] == body["InstanceArn"])
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the described instance {:?} is not enumerated: {entries:?}",
                body["InstanceArn"]
            )
        });
    assert_eq!(
        envelope, listed_entry,
        "case {case:?}: the envelope must be the view the enumeration serves, got \
         {envelope} vs {listed_entry}"
    );
    body
}

/// THE RULE over the instance lifecycle, for an instance `CreateInstance` minted: every
/// phase the instance passes through is reported in the envelope's `State` as well as by
/// `Status`, in every Region an instance can be enabled in.
#[tokio::test]
async fn the_described_envelope_follows_the_instance_through_its_lifecycle() {
    // The Region the instance is enabled in: the model's `RegionName` pattern
    // `^([a-z]+-){2,3}\d$` in two- and three-prefix spellings, across partitions. None of
    // them is the Region the recorded case used.
    let regions = ["us-east-1", "eu-west-3", "ap-southeast-3", "cn-northwest-1"];

    for region in regions {
        let (router, engine) = empty_account();
        let (status, created) = call_in_region(
            &router,
            region,
            "CreateInstance",
            &json!({"Name": "EnvelopeView"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: CreateInstance expected 200, got {status} {created}"
        );
        let instance_arn = created["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("region {region}: no InstanceArn in {created}"))
            .to_string();

        // PHASE 1 — the creation is in flight: CREATE_IN_PROGRESS.
        hold_activation(&engine, SystemTime::now() + Duration::from_secs(3_600));
        let case = format!("{region} / creation in flight");
        let body = assert_envelope_is_the_served_view(&case, &router, region, &instance_arn).await;
        assert_eq!(
            body["Instance"]["State"]["Name"].as_str(),
            Some("CREATE_IN_PROGRESS"),
            "case {case:?}: the creation is still in flight, got {body}"
        );

        // PHASE 2 — the activation has fallen due: ACTIVE, and the envelope says so on
        // every read, because a read reports rather than changes.
        hold_activation(&engine, SystemTime::now() - Duration::from_secs(1));
        for read in 0..2 {
            let case = format!("{region} / activation due / read {read}");
            let body =
                assert_envelope_is_the_served_view(&case, &router, region, &instance_arn).await;
            assert_eq!(
                body["Instance"]["State"]["Name"].as_str(),
                Some("ACTIVE"),
                "case {case:?}: the activation was due, got {body}"
            );
        }

        // PHASE 3 — a deletion is in flight: DELETE_IN_PROGRESS until the removal falls
        // due.
        let (status, deleted) = call_in_region(
            &router,
            region,
            "DeleteInstance",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: DeleteInstance expected 200, got {status} {deleted}"
        );
        hold_deletion(&engine, SystemTime::now() + Duration::from_secs(3_600));
        let case = format!("{region} / deletion in flight");
        let body = assert_envelope_is_the_served_view(&case, &router, region, &instance_arn).await;
        assert_eq!(
            body["Instance"]["State"]["Name"].as_str(),
            Some("DELETE_IN_PROGRESS"),
            "case {case:?}: the instance is being deleted, got {body}"
        );
    }
}

/// THE RULE on the account's AMBIENT instance — the one a client discovers with no create
/// first — over every value the model's `InstanceStatus` enum declares, `CREATE_FAILED`
/// included: the envelope's `State` is read off the stored status whatever put it there.
#[tokio::test]
async fn the_envelope_of_a_discovered_instance_reports_its_stored_status() {
    for status_value in INSTANCE_STATUS_VALUES {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(Arc::clone(&engine));
        let case = format!("ambient instance in {status_value}");

        // Discover the instance the way a client does, so the ambient instance is enabled
        // by the service rather than by this test.
        let discovered = listed(&case, &router, TEST_REGION, None).await;
        assert_eq!(
            discovered.len(),
            1,
            "case {case:?}: an account discovers exactly its ambient instance, got \
             {discovered:?}"
        );
        let instance_arn = discovered[0]["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no InstanceArn in {discovered:?}"))
            .to_string();
        assert_envelope_is_the_served_view(&case, &router, TEST_REGION, &instance_arn).await;

        with_store(&engine, |store| {
            for instance in store.instances.values_mut() {
                instance.status = (*status_value).to_string();
                // Nothing is owed: the status seeded here IS the instance's status, so no
                // transition the clock owes can move it out from under the read.
                instance.active_due = None;
                instance.deleted_due = None;
            }
        });

        let body =
            assert_envelope_is_the_served_view(&case, &router, TEST_REGION, &instance_arn).await;
        assert_eq!(
            body["Instance"]["State"]["Name"].as_str(),
            Some(*status_value),
            "case {case:?}: expected the stored status in the envelope, got {body}"
        );
    }
}

/// THE RULE where the account's instance is an ORGANIZATION instance, which this API does
/// not enable: the envelope is that instance's own view, and the members the envelope does
/// NOT carry are the ones the served view has no member for — the envelope is the instance
/// view, not a copy of the whole response.
#[tokio::test]
async fn the_envelope_of_an_organization_instance_is_its_own_view() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let case = "an organization instance";

    let body = assert_envelope_is_the_served_view(case, &router, TEST_REGION, &instance_arn).await;
    assert_eq!(
        body["Instance"]["Name"].as_str(),
        Some("Organization"),
        "case {case:?}: the envelope reports the instance's own name, got {body}"
    );
    // An organization instance serves permission sets, and the response says so — on the
    // flat member the model declares for it. The served instance view declares no member
    // for it, so the envelope carries none rather than inventing one.
    assert_eq!(
        body["PermissionSetsEnabled"],
        Value::Bool(true),
        "case {case:?}: an organization instance serves permission sets, got {body}"
    );
    for member in ["PermissionSetsEnabled", "EncryptionConfigurationDetails"] {
        assert_eq!(
            body["Instance"][member],
            Value::Null,
            "case {case:?}: the envelope is the served instance view, which has no \
             {member} member, got {:?}",
            body["Instance"][member]
        );
    }
}

/// THE RULE where the store holds MORE than one instance, each in a different phase: the
/// envelope is the view of the instance the request NAMED, so describing one instance never
/// reports the other's lifecycle.
#[tokio::test]
async fn each_described_instance_gets_its_own_envelope() {
    let region = "ap-northeast-1";
    let (router, engine) = empty_account();
    // Two instances, seeded through the service's own constructors (an account holds one
    // instance, so no call of this API enables a second), in two different phases.
    // Everything asserted then goes over the wire.
    let now = SystemTime::now();
    with_store(&engine, |store| {
        let active = state::Instance::active(
            "ssoins-3333333333333333",
            "d-3333333333",
            state::InstanceType::Organization,
            Some("Active".to_string()),
            aws_smithy_types::DateTime::from(now),
            region,
        );
        let creating = state::Instance::creating(
            "ssoins-4444444444444444",
            "d-4444444444",
            state::InstanceType::Account,
            Some("Creating".to_string()),
            // Far enough out that the activation is not owed while this test reads.
            now + Duration::from_secs(3_600),
            region,
        );
        store.instances.insert(active.instance_arn.clone(), active);
        store
            .instances
            .insert(creating.instance_arn.clone(), creating);
        store.ambient_instance_enabled = true;
    });

    // (the instance's name, the lifecycle its envelope must report)
    let rows = [
        ("Active", "ACTIVE"),
        ("Creating", "CREATE_IN_PROGRESS"),
    ];
    let entries = listed("two instances", &router, region, None).await;
    assert_eq!(
        entries.len(),
        2,
        "both instances are enumerated, got {entries:?}"
    );

    for (name, expected_state) in rows {
        let entry = entries
            .iter()
            .find(|entry| entry["Name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("no instance named {name} is enumerated: {entries:?}"));
        let instance_arn = entry["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("no InstanceArn on {entry}"))
            .to_string();
        let case = format!("two instances / describing {name}");
        let body = assert_envelope_is_the_served_view(&case, &router, region, &instance_arn).await;
        assert_eq!(
            body["Instance"]["Name"].as_str(),
            Some(name),
            "case {case:?}: the envelope must be the named instance's, got {body}"
        );
        assert_eq!(
            body["Instance"]["State"]["Name"].as_str(),
            Some(expected_state),
            "case {case:?}: each envelope reports its own lifecycle, got {body}"
        );
    }
}
