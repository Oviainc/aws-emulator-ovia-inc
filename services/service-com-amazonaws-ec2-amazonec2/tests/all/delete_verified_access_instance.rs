// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteVerifiedAccessInstance`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the delete REMOVES THE INSTANCE THE REQUEST NAMES and answers the record the store
//!    held — member for member what a describe of that instance reported a moment earlier —
//!    the logging configuration kept beside the instance goes with it, the caller-chosen bytes
//!    both held go back to the store's budget, the instance beside it is left alone, and the
//!    operations that resolve an instance no longer find the deleted one, each refusing it in
//!    the wording that operation answers;
//! 2. a delete naming an id the account holds no instance for is refused at HTTP 400 with the
//!    instance's own `Invalid<Kind>.NotFound` code, naming the id the request gave, and
//!    REMOVES NOTHING;
//! 3. a resource that still names the instance — a group created on it, an endpoint that
//!    reports it — is a dependency, so the delete is refused at HTTP 400
//!    `DependencyViolation` while one is stored and succeeds once none is;
//! 4. `DryRun` is answered with the outcome the delete WOULD have had, never with a success
//!    status, and removes nothing either way;
//! 5. one delete `ClientToken` names the ONE delete it performed, so a retry under it answers
//!    what the first call answered and removes no second instance, the published client-token
//!    bound holds, and the deletes a store remembers for their tokens are bounded.
//!
//! Each table row is a distinct input along what the rule generalises over — which optional
//! members the deleted instance was created with, whether a logging configuration was set on
//! it, which form an unresolvable id takes, which kind of resource still names the instance,
//! how long a client token is — so no row is the specification.

use ec2::ops::types::{VerifiedAccessEndpoint, VerifiedAccessGroup, VerifiedAccessInstance};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, VerifiedAccessEndpointRecord,
    VerifiedAccessGroupRecord, MAX_CLIENT_TOKEN_CHARS,
    MAX_REMEMBERED_VERIFIED_ACCESS_INSTANCE_DELETIONS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, unescaped};

/// The Region every call here is signed for: a Region that is not the emulator's default is
/// the stronger case, and a Verified Access instance is a regional entity.
const REGION: &str = "eu-central-1";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// Stand up an emulator with nothing seeded: the account starts with no Verified Access
/// instance, so every instance a case deletes is one this suite created.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `CreateVerifiedAccessInstance` call.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateVerifiedAccessInstance", params).await
}

/// One `DeleteVerifiedAccessInstance` call.
async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DeleteVerifiedAccessInstance", params).await
}

/// One `DescribeVerifiedAccessInstances` call naming the one instance `instance_id`.
async fn describe(router: &MultiServiceRouter, instance_id: &str) -> (u16, String) {
    call(
        router,
        REGION,
        "DescribeVerifiedAccessInstances",
        &[p("VerifiedAccessInstanceId.1", instance_id.to_string())],
    )
    .await
}

/// The ids of the Verified Access instances the store holds, in the order it keeps them.
fn held_instance_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_instances
        .keys()
        .cloned()
        .collect()
}

/// The instances the store holds a logging configuration for, which is the fact of an instance
/// the modelled instance shape carries no member for.
fn configured_instance_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_instance_logging_configurations
        .keys()
        .cloned()
        .collect()
}

/// How many caller-chosen bytes the store has charged against its budget.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
}

/// Seed one instance through the insert seam `CreateVerifiedAccessInstance` lands on, and
/// answer the id it was given — the seam the group and logging suites seed instances through.
fn seed_instance(engine: &engine::InMemoryEngine, description: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_verified_access_instance(
            VerifiedAccessInstance {
                description: Some(description.to_string()),
                ..VerifiedAccessInstance::default()
            },
            None,
        )
}

/// Seed one group created on `instance_id` through the insert seam
/// `CreateVerifiedAccessGroup` lands on, and answer the id it was given.
fn seed_group(engine: &engine::InMemoryEngine, instance_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_verified_access_group(
            VerifiedAccessGroupRecord {
                view: VerifiedAccessGroup {
                    verified_access_instance_id: Some(instance_id.to_string()),
                    ..VerifiedAccessGroup::default()
                },
                ..VerifiedAccessGroupRecord::default()
            },
            REGION,
            None,
        )
}

/// Seed one endpoint reporting `instance_id` through the insert seam
/// `CreateVerifiedAccessEndpoint` lands on, and answer the id it was given.
fn seed_endpoint(engine: &engine::InMemoryEngine, instance_id: &str, group_id: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_verified_access_endpoint(
            VerifiedAccessEndpointRecord {
                view: VerifiedAccessEndpoint {
                    verified_access_instance_id: Some(instance_id.to_string()),
                    verified_access_group_id: Some(group_id.to_string()),
                    ..VerifiedAccessEndpoint::default()
                },
                pending: None,
                policy_document: None,
            },
            None,
            REGION,
            None,
        )
}

/// Drop every group and endpoint that names `instance_id` straight out of the store: the
/// endpoint's own delete is still a scaffold stub, so a case that needs the instance to have
/// NO dependents left clears them through the store rather than through two operations one of
/// which does not exist yet.
fn clear_dependents(engine: &engine::InMemoryEngine, instance_id: &str) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let groups: Vec<String> = guard
        .verified_access_groups
        .iter()
        .filter(|(_, record)| {
            record.view.verified_access_instance_id.as_deref() == Some(instance_id)
        })
        .map(|(group_id, _)| group_id.clone())
        .collect();
    for group_id in groups {
        guard.verified_access_groups.remove(&group_id);
    }
    let endpoints: Vec<String> = guard
        .verified_access_endpoints
        .iter()
        .filter(|(_, record)| {
            record.view.verified_access_instance_id.as_deref() == Some(instance_id)
        })
        .map(|(endpoint_id, _)| endpoint_id.clone())
        .collect();
    for endpoint_id in endpoints {
        guard.verified_access_endpoints.remove(&endpoint_id);
    }
}

/// The members every reported instance carries besides its tags and its custom subdomain, so
/// two reports of the SAME instance can be compared member for member
/// (`smithy_query(query="shape",shape="VerifiedAccessInstance")`).
const INSTANCE_MEMBERS: [&str; 5] = [
    "verifiedAccessInstanceId",
    "description",
    "creationTime",
    "lastUpdatedTime",
    "fipsEnabled",
];

/// Assert that `right` reports the same instance `left` did, member for member — the custom
/// subdomain and the tags included, so a report that dropped one would fail the case.
fn assert_same_instance(case: &str, left_label: &str, left: &str, right_label: &str, right: &str) {
    for member in INSTANCE_MEMBERS.iter().chain(["subDomain"].iter()) {
        assert_eq!(
            element(left, member),
            element(right, member),
            "case {case:?}: expected the {right_label} to report the {left_label}'s \
             {member}={:?}, got {:?}",
            element(left, member),
            element(right, member)
        );
    }
    for member in ["key", "value"] {
        assert_eq!(
            elements(left, member),
            elements(right, member),
            "case {case:?}: expected the {right_label} to report the {left_label}'s tag \
             {member}s {:?}, got {:?}",
            elements(left, member),
            elements(right, member)
        );
    }
}

/// `count` tag parameters for the instance a create mints, with distinct keys.
fn tag_params(count: usize) -> Params {
    let mut params = vec![p("TagSpecification.1.ResourceType", "verified-access-instance")];
    for index in 1..=count {
        params.push(p(
            &format!("TagSpecification.1.Tag.{index}.Key"),
            format!("Key{index}"),
        ));
        params.push(p(
            &format!("TagSpecification.1.Tag.{index}.Value"),
            format!("value{index}"),
        ));
    }
    params
}

/// RULE 1 over the request shapes an instance can have been created with: "Delete an Amazon
/// Web Services Verified Access instance" (`DeleteVerifiedAccessInstance`) — the instance the
/// request names LEAVES the store, the answer reports the record that was removed ("Details
/// about the Verified Access instance",
/// `DeleteVerifiedAccessInstanceResult$VerifiedAccessInstance`), the logging configuration
/// kept beside it goes with it, the caller-chosen bytes go back to the store's budget, and the
/// operations that resolve an instance no longer find it.
///
/// Each row is an instance created with a different set of the optional members an instance
/// carries — which is what the answer generalises over — so no one request's members are the
/// specification. A second instance nobody deletes is seeded beside each one, so a delete that
/// emptied the store would fail the case.
#[tokio::test]
async fn the_delete_removes_the_instance_and_answers_the_record_the_store_held() {
    // (case, the optional members the created instance carries, whether a logging
    // configuration is set on it, whether the record holds caller-chosen bytes the delete has
    // to give back).
    let rows: Vec<(&str, Params, bool, bool)> = vec![
        ("nothing but the defaults", Vec::new(), false, false),
        (
            "a description",
            vec![p("Description", "the sales applications")],
            false,
            true,
        ),
        (
            "FIPS support asked for",
            vec![p("FIPSEnabled", "true")],
            false,
            false,
        ),
        (
            "a custom subdomain",
            vec![p("CidrEndpointsCustomSubDomain", "sales.example.internal")],
            false,
            true,
        ),
        ("two tags", tag_params(2), false, true),
        (
            "a create client token",
            vec![p("ClientToken", "the-create-of-this-instance")],
            false,
            true,
        ),
        (
            "a logging configuration beside the instance",
            vec![p("Description", "every destination")],
            true,
            true,
        ),
    ];

    for (case, extra, configure_logging, frees_bytes) in rows {
        let (router, engine) = emulator();
        let (status, created) = create(&router, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must succeed, got {status} with body {created}"
        );
        let instance_id = element(&created, "verifiedAccessInstanceId").unwrap_or_default();
        if configure_logging {
            let (status, body) = call(
                &router,
                REGION,
                "ModifyVerifiedAccessInstanceLoggingConfiguration",
                &[
                    p("VerifiedAccessInstanceId", instance_id.clone()),
                    p("AccessLogs.S3.Enabled", "true"),
                    p("AccessLogs.S3.BucketName", "verified-access-records"),
                    p("AccessLogs.S3.Prefix", "instance/logs"),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the logging configuration must be set, got {status} with body \
                 {body}"
            );
            assert_eq!(
                configured_instance_ids(&engine),
                vec![instance_id.clone()],
                "case {case:?}: expected the configuration to be stored for the instance, got \
                 {:?}",
                configured_instance_ids(&engine)
            );
        }

        // What a reader of the instance reports while it is still stored: the delete's answer
        // is compared against THIS rather than against the create's, so the record the store
        // held is what the delete has to report.
        let (status, described) = describe(&router, &instance_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must report the stored instance, got {status} with \
             body {described}"
        );

        let bystander = seed_instance(&engine, "the instance nobody deletes");
        let bytes_before = stored_bytes(&engine);

        let (status, deleted) =
            delete(&router, &[p("VerifiedAccessInstanceId", instance_id.clone())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteVerifiedAccessInstance, got {status} \
             with body {deleted}"
        );
        assert_same_instance(case, "describe", &described, "delete", &deleted);

        // The store, not just the answer: the deleted instance is gone, the instance nobody
        // deleted is untouched, its logging configuration went with it, and the bytes the
        // record held are back.
        assert_eq!(
            held_instance_ids(&engine),
            vec![bystander.clone()],
            "case {case:?}: expected only the instance nobody deleted to be held, got {:?}",
            held_instance_ids(&engine)
        );
        assert!(
            configured_instance_ids(&engine).is_empty(),
            "case {case:?}: expected the logging configuration beside the instance to go with \
             it, still holding {:?}",
            configured_instance_ids(&engine)
        );
        if frees_bytes {
            assert!(
                stored_bytes(&engine) < bytes_before,
                "case {case:?}: expected the removed record's caller-chosen bytes back, still \
                 charged {} of {bytes_before}",
                stored_bytes(&engine)
            );
        } else {
            assert_eq!(
                stored_bytes(&engine),
                bytes_before,
                "case {case:?}: a record that held no caller-chosen bytes frees none, got {} \
                 of {bytes_before}",
                stored_bytes(&engine)
            );
        }

        // The operations that RESOLVE an instance answer the account as the delete left it:
        // the deleted id resolves to nothing, the instance beside it still does. Each one is
        // refused with the instance's own NotFound code, worded as THAT operation answers it —
        // a report of the instances words the sentence differently from a call that mutates
        // one or takes one as a reference.
        for (operation, params, expected_message) in [
            (
                "DescribeVerifiedAccessInstances",
                vec![p("VerifiedAccessInstanceId.1", instance_id.clone())],
                format!("VerifiedAccessInstance {instance_id} does not exist"),
            ),
            (
                "CreateVerifiedAccessGroup",
                vec![p("VerifiedAccessInstanceId", instance_id.clone())],
                format!("The verified-access-instance ID '{instance_id}' does not exist"),
            ),
            (
                "ModifyVerifiedAccessInstance",
                vec![
                    p("VerifiedAccessInstanceId", instance_id.clone()),
                    p("Description", "a modification of a deleted instance"),
                ],
                format!("The verified-access-instance ID '{instance_id}' does not exist"),
            ),
        ] {
            let (status, refused) = call(&router, REGION, operation, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} to refuse the deleted instance, got \
                 {status} with body {refused}"
            );
            assert_eq!(
                unescaped(&error_message(&refused)),
                expected_message,
                "case {case:?}: expected {operation} to word its refusal \
                 {expected_message:?}, got {}",
                unescaped(&error_message(&refused))
            );
            assert_eq!(
                error_code(&refused),
                "InvalidVerifiedAccessInstanceId.NotFound",
                "case {case:?}: expected the instance's own NotFound code from {operation}, \
                 got {refused}"
            );
        }
        let (status, described) = describe(&router, &bystander).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the instance nobody deleted to still resolve, got \
             {status} with body {described}"
        );
    }
}

/// RULE 2 over the forms an unresolvable id takes: whatever the id looks like, a delete that
/// names no instance of the account is refused — HTTP 400
/// `InvalidVerifiedAccessInstanceId.NotFound` naming the id the request gave — and the store
/// is left alone, so a caller that deletes twice can tell the deletion it performed from one
/// it did not.
#[tokio::test]
async fn a_delete_of_an_id_the_account_does_not_hold_is_refused() {
    let (router, engine) = emulator();
    let kept = seed_instance(&engine, "the instance nobody deletes");
    let spent = seed_instance(&engine, "the instance this caller deletes first");
    let (status, body) = delete(&router, &[p("VerifiedAccessInstanceId", spent.clone())]).await;
    assert_eq!(
        status, 200,
        "the first delete must succeed, got {status}: {body}"
    );

    // (case, the id the request names) — the long and the short EC2 id form of an instance
    // that was never minted, another Verified Access resource kind's id, a value that is no
    // EC2 id at all, and the id of an instance this caller already deleted.
    let rows: Vec<(&str, String)> = vec![
        (
            "an unminted long-form instance id",
            "vai-0123456789abcdef0".to_string(),
        ),
        (
            "an unminted short-form instance id",
            "vai-01234567".to_string(),
        ),
        ("a group id", "vagr-0123456789abcdef0".to_string()),
        (
            "a trust provider id",
            "vatp-0123456789abcdef0".to_string(),
        ),
        ("an id of no EC2 resource", "not-an-instance".to_string()),
        ("an instance this caller already deleted", spent.clone()),
    ];

    for (case, instance_id) in rows {
        let (status, body) =
            delete(&router, &[p("VerifiedAccessInstanceId", instance_id.clone())]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an unresolvable instance, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVerifiedAccessInstanceId.NotFound",
            "case {case:?}: expected the instance's own NotFound code, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The verified-access-instance ID '{instance_id}' does not exist"),
            "case {case:?}: expected the refusal to name the id the request gave, got {}",
            error_message(&body)
        );
        assert_eq!(
            held_instance_ids(&engine),
            vec![kept.clone()],
            "case {case:?}: a refused delete removes nothing, got {:?}",
            held_instance_ids(&engine)
        );
    }
}

/// RULE 3 over the kinds of resource that still name an instance: a group is created ON an
/// instance and an endpoint reports the instance its group belongs to, so either one still
/// stored is a reference the delete refuses to leave dangling — HTTP 400 `DependencyViolation`
/// naming the instance the request named — and the same delete succeeds once nothing names it.
#[tokio::test]
async fn an_instance_another_resource_still_names_is_not_deleted() {
    // (case, how many groups name the instance, whether an endpoint names it too).
    let rows: Vec<(&str, usize, bool)> = vec![
        ("one group created on the instance", 1, false),
        ("two groups created on the instance", 2, false),
        ("an endpoint that reports the instance", 1, true),
    ];

    for (case, group_count, with_endpoint) in rows {
        let (router, engine) = emulator();
        let (status, created) = create(&router, &[p("Description", "the applications")]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must succeed, got {status} with body {created}"
        );
        let instance_id = element(&created, "verifiedAccessInstanceId").unwrap_or_default();
        let mut group_ids = Vec::new();
        for _ in 0..group_count {
            group_ids.push(seed_group(&engine, &instance_id));
        }
        if with_endpoint {
            seed_endpoint(&engine, &instance_id, &group_ids[0]);
        }

        let (status, body) =
            delete(&router, &[p("VerifiedAccessInstanceId", instance_id.clone())]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 while a resource still names the instance, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DependencyViolation",
            "case {case:?}: expected the dependency refusal, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "The verifiedAccessInstance '{instance_id}' has dependencies and cannot be \
                 deleted."
            ),
            "case {case:?}: expected the refusal to name the instance the request named, got {}",
            error_message(&body)
        );
        assert_eq!(
            held_instance_ids(&engine),
            vec![instance_id.clone()],
            "case {case:?}: a refused delete removes nothing, got {:?}",
            held_instance_ids(&engine)
        );

        // Once nothing names the instance any more, the SAME delete succeeds.
        clear_dependents(&engine, &instance_id);
        let (status, body) =
            delete(&router, &[p("VerifiedAccessInstanceId", instance_id.clone())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to succeed once nothing names the instance, \
             got {status} with body {body}"
        );
        assert!(
            held_instance_ids(&engine).is_empty(),
            "case {case:?}: expected the instance to be gone, still holding {:?}",
            held_instance_ids(&engine)
        );
    }
}

/// RULE 4: "Checks whether you have the required permissions for the action, without actually
/// making the request" (`DeleteVerifiedAccessInstanceRequest$DryRun`) — the dry run is
/// answered with the outcome the delete WOULD have had, never with a success status, and
/// removes nothing either way.
#[tokio::test]
async fn a_dry_run_of_the_delete_reports_the_outcome_and_removes_nothing() {
    // (case, whether the request names an instance the account holds, whether a group names
    // that instance, the HTTP status, the error code).
    let rows: Vec<(&str, bool, bool, u16, &str)> = vec![
        (
            "a delete that would have succeeded",
            true,
            false,
            412,
            "DryRunOperation",
        ),
        (
            "a delete that would have been refused as unresolvable",
            false,
            false,
            400,
            "InvalidVerifiedAccessInstanceId.NotFound",
        ),
        (
            "a delete that would have been refused for a dependency",
            true,
            true,
            400,
            "DependencyViolation",
        ),
    ];

    for (case, resolvable, with_group, expected_status, expected_code) in rows {
        let (router, engine) = emulator();
        let held = seed_instance(&engine, "the instance under a dry run");
        if with_group {
            seed_group(&engine, &held);
        }
        let instance_id = if resolvable {
            held.clone()
        } else {
            "vai-0123456789abcdef0".to_string()
        };
        let (status, body) = delete(
            &router,
            &[
                p("VerifiedAccessInstanceId", instance_id),
                p("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the {expected_code} refusal, got {body}"
        );
        assert_eq!(
            held_instance_ids(&engine),
            vec![held.clone()],
            "case {case:?}: a dry run removes nothing, got {:?}",
            held_instance_ids(&engine)
        );
    }
}

/// RULE 5: "A unique, case-sensitive token that you provide to ensure idempotency of your
/// modification request" (`DeleteVerifiedAccessInstanceRequest$ClientToken`) — one token names
/// the ONE delete it performed, so a retry under it answers what that delete answered rather
/// than refusing an id its own call removed, and removes no second instance.
#[tokio::test]
async fn one_client_token_names_one_delete() {
    let (router, engine) = emulator();
    let first = seed_instance(&engine, "the first instance");
    let second = seed_instance(&engine, "the second instance");
    let third = seed_instance(&engine, "the third instance");
    let survivor = seed_instance(&engine, "the instance nobody deletes");

    let (status, answered) = delete(
        &router,
        &[
            p("VerifiedAccessInstanceId", first.clone()),
            p("ClientToken", "the-first-delete"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the first delete must succeed, got {status}: {answered}"
    );

    // (case, the parameters of the next call, the instance it must report, the description
    // that instance was seeded with, the instances the store must hold afterwards).
    let rows: Vec<(&str, Params, String, &str, Vec<String>)> = vec![
        (
            "the same token naming the same instance",
            vec![
                p("VerifiedAccessInstanceId", first.clone()),
                p("ClientToken", "the-first-delete"),
            ],
            first.clone(),
            "the first instance",
            vec![second.clone(), third.clone(), survivor.clone()],
        ),
        (
            "the same token naming another instance, which is a request of its own",
            vec![
                p("VerifiedAccessInstanceId", second.clone()),
                p("ClientToken", "the-first-delete"),
            ],
            second.clone(),
            "the second instance",
            vec![third.clone(), survivor.clone()],
        ),
        (
            "a token the store has not seen",
            vec![
                p("VerifiedAccessInstanceId", third.clone()),
                p("ClientToken", "the-third-delete"),
            ],
            third.clone(),
            "the third instance",
            vec![survivor.clone()],
        ),
    ];

    for (case, params, reported, description, mut expected_held) in rows {
        let (status, body) = delete(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "verifiedAccessInstanceId").as_deref(),
            Some(reported.as_str()),
            "case {case:?}: expected the answer to report {reported}, got {body}"
        );
        assert_eq!(
            element(&body, "description").as_deref(),
            Some(description),
            "case {case:?}: expected the record the store held for {reported}, got {body}"
        );
        expected_held.sort();
        let mut held = held_instance_ids(&engine);
        held.sort();
        assert_eq!(
            held, expected_held,
            "case {case:?}: expected the store to hold {expected_held:?}, got {held:?}"
        );
    }
}

/// RULE 5's bound on the token itself: a delete token is retained for the process's lifetime,
/// so its length is held to the bound every client token of this service is — on the bound and
/// one past it.
#[tokio::test]
async fn the_published_client_token_bound_holds_on_the_delete() {
    // (case, how many characters the token has, the HTTP status, the error code).
    let rows: Vec<(&str, usize, u16, &str)> = vec![
        ("a token on the bound", MAX_CLIENT_TOKEN_CHARS, 200, ""),
        (
            "a token one past the bound",
            MAX_CLIENT_TOKEN_CHARS + 1,
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, token_chars, expected_status, expected_code) in rows {
        let (router, engine) = emulator();
        let held = seed_instance(&engine, "the instance under a bounded token");
        let (status, body) = delete(
            &router,
            &[
                p("VerifiedAccessInstanceId", held.clone()),
                p("ClientToken", "t".repeat(token_chars)),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the {expected_code:?} code, got {body}"
        );
        let expected_held: Vec<String> = if expected_status == 200 {
            Vec::new()
        } else {
            vec![held.clone()]
        };
        assert_eq!(
            held_instance_ids(&engine),
            expected_held,
            "case {case:?}: expected the store to hold {expected_held:?}, got {:?}",
            held_instance_ids(&engine)
        );
    }
}

/// RULE 5's bound on the HISTORY: the deletes this store remembers for their idempotency
/// tokens are BOUNDED. A delete frees the live record's slot, so nothing else caps a history
/// that outlives the resource: the store keeps the most recent
/// [`MAX_REMEMBERED_VERIFIED_ACCESS_INSTANCE_DELETIONS`] of them and drops the oldest, so a
/// retry of a delete still remembered is answered while one of a delete long since displaced
/// sees the id as gone — and a delete is never refused because the history is full.
#[tokio::test]
async fn the_remembered_deletes_are_bounded_and_drop_the_oldest() {
    let (router, engine) = emulator();

    // One delete past the bound, each under its own token, so the FIRST one is the entry the
    // last delete displaces.
    let mut deleted: Vec<(String, String)> = Vec::new();
    for nth in 0..=MAX_REMEMBERED_VERIFIED_ACCESS_INSTANCE_DELETIONS {
        let instance_id = seed_instance(&engine, "one of many");
        let token = format!("delete-token-{nth}");
        let (status, body) = delete(
            &router,
            &[
                p("VerifiedAccessInstanceId", instance_id.clone()),
                p("ClientToken", token.clone()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"delete {nth} of {MAX_REMEMBERED_VERIFIED_ACCESS_INSTANCE_DELETIONS}\": a \
             full history must not refuse a delete, got {status} with body {body}"
        );
        deleted.push((token, instance_id));
    }
    assert!(
        held_instance_ids(&engine).is_empty(),
        "expected every seeded instance to have been deleted, still holding {:?}",
        held_instance_ids(&engine)
    );

    // (case, the delete to retry, whether the history still answers it).
    for (case, (token, instance_id), remembered) in [
        (
            "the oldest delete, displaced by the newest",
            deleted.first().cloned().unwrap_or_default(),
            false,
        ),
        (
            "the newest delete",
            deleted.last().cloned().unwrap_or_default(),
            true,
        ),
    ] {
        let (status, body) = delete(
            &router,
            &[
                p("VerifiedAccessInstanceId", instance_id.clone()),
                p("ClientToken", token.clone()),
            ],
        )
        .await;
        if remembered {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the retry to be answered, got {status} with body {body}"
            );
            assert_eq!(
                element(&body, "verifiedAccessInstanceId").as_deref(),
                Some(instance_id.as_str()),
                "case {case:?}: expected the retry to report {instance_id}, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a displaced delete's token to see the id as gone, got \
                 {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidVerifiedAccessInstanceId.NotFound",
                "case {case:?}: expected the instance's own NotFound code, got {body}"
            );
        }
    }
}
