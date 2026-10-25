// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteIpamInternetRegistryAssociation`, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * Deleting an IPAM internet registry association DELETES the association the request
//!   names: it is answered in the transient `delete-in-progress` state with the clock owing
//!   it `delete-complete`, reported with the members the store holds for it, and no other
//!   association of the account is touched.
//! * An association something still depends on cannot be deleted — "before deleting, you
//!   must remove all routing policy registrations associated with the internet registry" —
//!   so one that still holds a registration is refused with EC2's dependency code, while one
//!   whose registrations have all been withdrawn is deletable.
//! * An id outside the form this service ISSUES for the resource a member names is refused
//!   with that resource's own `Invalid<Kind>.Malformed` before any lookup; an id of that form
//!   that names nothing the account holds is refused with its `Invalid<Kind>.NotFound`; an
//!   association whose deletion has already been asked for is refused for the state it is in;
//!   and `DryRun` answers the permission question and writes nothing.
//!
//! Each table row is a distinct input along what those rules generalise over: which Regional
//! Internet Registry the association is with, the IPAM and Region it belongs to, whether it
//! carries a description and how many tags, which state it sits in, how many registrations
//! it holds and in which state, and which part of the id is outside the issued form.

use std::time::Duration;

use ec2::ops::types::{
    IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES, IPAM_ROUTING_POLICY_REGISTRATION_STATE_VALUES,
    IpamInternetRegistryAssociation, IpamRoutingPolicyRegistration, RIR_VALUES, Tag,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_COMPLETE_MESSAGE,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_IN_PROGRESS_MESSAGE,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS,
    IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
    IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE,
    IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_IN_PROGRESS,
    IpamRoutingPolicyRegistrationRecord, RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The IPAM the seeded associations belong to unless a row names another.
const IPAM_ID: &str = "ipam-0123456789abcdef0";

/// The state a seeded association sits in unless a row names another: the
/// `IpamInternetRegistryAssociationState` catalog's `enable-complete` member, which
/// [`IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES`] is asserted to carry below.
const ASSOCIATION_STATE_ENABLE_COMPLETE: &str = "enable-complete";

/// That catalog's `disable-complete` member, another state an association can sit in with
/// nothing owed to it.
const ASSOCIATION_STATE_DISABLE_COMPLETE: &str = "disable-complete";

/// That catalog's `create-failed` member: an association whose setup never finished, which
/// is still an association a caller can delete.
const ASSOCIATION_STATE_CREATE_FAILED: &str = "create-failed";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// The `IpamInternetRegistryAssociationId=` field every request here carries, plus `extra`
/// form fields.
fn delete_params(association_id: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params = vec![(
        "IpamInternetRegistryAssociationId".to_string(),
        association_id.to_string(),
    )];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    params
}

/// What one seeded association is made of — the inputs the delete generalises over.
struct Seed {
    /// The IPAM it belongs to.
    ipam_id: &'static str,
    /// The Region of that IPAM.
    region: &'static str,
    /// The Regional Internet Registry it is with, a member of the modelled `Rir` catalog.
    rir: &'static str,
    /// Its organization handle at that registry.
    organization_handle: &'static str,
    /// Its description, when it carries one.
    description: Option<&'static str>,
    /// The state it sits in before the delete.
    state: &'static str,
    /// The tags assigned to it.
    tags: &'static [(&'static str, &'static str)],
}

/// The association a row does not vary: the control every row keeps beside the one it
/// deletes, so each delete is checked to touch only the association it names.
const CONTROL: Seed = Seed {
    ipam_id: IPAM_ID,
    region: REGION,
    rir: "arin",
    organization_handle: "EXAMPLE-CONTROL",
    description: None,
    state: ASSOCIATION_STATE_ENABLE_COMPLETE,
    tags: &[("Name", "control")],
};

/// Seed one internet registry association and answer the id this service minted for it.
fn seed_association(engine: &engine::InMemoryEngine, seed: &Seed) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let association_id = guard
        .insert_ipam_internet_registry_association(IpamInternetRegistryAssociation {
            ipam_id: Some(seed.ipam_id.to_string()),
            ipam_region: Some(seed.region.to_string()),
            rir: Some(seed.rir.to_string()),
            organization_handle: Some(seed.organization_handle.to_string()),
            description: seed.description.map(str::to_string),
            state: Some(seed.state.to_string()),
            tags: Some(
                seed.tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some((*key).to_string()),
                        value: Some((*value).to_string()),
                    })
                    .collect(),
            ),
            ..Default::default()
        })
        .expect("the store holds the seeded association");
    // Every id this service ISSUES carries this resource's own prefix, which is what makes
    // an id under any other prefix malformed rather than missing.
    assert!(
        association_id.starts_with(&format!("{IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX}-")),
        "the minted association id {association_id:?} must carry the issued prefix \
         {IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX:?}"
    );
    association_id
}

/// Seed one routing policy registration for `cidr` under `association_id`, in `state`.
fn seed_registration(
    engine: &engine::InMemoryEngine,
    association_id: &str,
    cidr: &str,
    state: &str,
) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_ipam_routing_policy_registration(
            association_id,
            IpamRoutingPolicyRegistrationRecord::in_state(
                IpamRoutingPolicyRegistration {
                    cidr: Some(cidr.to_string()),
                    asns: Some(vec!["64512".to_string()]),
                    ..Default::default()
                },
                state,
            ),
        )
        .expect("the store holds the seeded registration");
}

/// The state the stored association is in right now, read straight out of the store; `None`
/// when the account holds no association under that id.
fn stored_state(engine: &engine::InMemoryEngine, association_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_internet_registry_association(association_id)
        .map(|record| record.state().to_string())
}

/// The `StateMessage` the stored association carries right now.
fn stored_state_message(engine: &engine::InMemoryEngine, association_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_internet_registry_association(association_id)
        .and_then(|record| record.view.state_message.clone())
}

/// How many associations the store holds right now.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_internet_registry_associations
        .len()
}

/// The associations the delete is asserted over: every Regional Internet Registry the model
/// publishes, in three Regions and under two IPAMs, with and without a description, with no
/// tags, one and three, and from three states an association can sit in.
fn rows() -> Vec<(&'static str, Seed)> {
    vec![
        (
            "an arin association with no tags, in the Region the request is signed for",
            Seed {
                ipam_id: IPAM_ID,
                region: REGION,
                rir: "arin",
                organization_handle: "EXAMPLE-ORG",
                description: None,
                state: ASSOCIATION_STATE_ENABLE_COMPLETE,
                tags: &[],
            },
        ),
        (
            "a ripe association with a description and one tag, in another Region",
            Seed {
                ipam_id: IPAM_ID,
                region: "eu-west-2",
                rir: "ripe",
                organization_handle: "ORG-EX1-RIPE",
                description: Some("delegated rpki for the edge prefixes"),
                state: ASSOCIATION_STATE_ENABLE_COMPLETE,
                tags: &[("Name", "edge")],
            },
        ),
        (
            "an apnic association with three tags, under another IPAM",
            Seed {
                ipam_id: "ipam-00000000000000abc",
                region: "ap-southeast-1",
                rir: "apnic",
                organization_handle: "A12345678",
                description: Some("apac"),
                state: ASSOCIATION_STATE_DISABLE_COMPLETE,
                tags: &[("Name", "apac"), ("Team", "network"), ("Env", "prod")],
            },
        ),
        (
            "a lacnic association whose creation failed",
            Seed {
                ipam_id: IPAM_ID,
                region: "sa-east-1",
                rir: "lacnic",
                organization_handle: "LACNIC-EX",
                description: None,
                state: ASSOCIATION_STATE_CREATE_FAILED,
                tags: &[],
            },
        ),
        (
            "a nicbr association with a description and no tags",
            Seed {
                ipam_id: IPAM_ID,
                region: "sa-east-1",
                rir: "nicbr",
                organization_handle: "NICBR-EX",
                description: Some("brazil"),
                state: ASSOCIATION_STATE_ENABLE_COMPLETE,
                tags: &[],
            },
        ),
    ]
}

/// The words this suite asserts on are the model's own: each state is a member of the
/// catalog the model publishes for its enum and each registry a member of `Rir`, so a row
/// cannot pin a word the API does not have.
#[test]
fn the_asserted_words_are_the_models_own() {
    for (case, word, catalog) in [
        (
            "an association in service",
            ASSOCIATION_STATE_ENABLE_COMPLETE,
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES,
        ),
        (
            "an association that has been disabled",
            ASSOCIATION_STATE_DISABLE_COMPLETE,
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES,
        ),
        (
            "an association whose creation failed",
            ASSOCIATION_STATE_CREATE_FAILED,
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES,
        ),
        (
            "an association being deleted",
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS,
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES,
        ),
        (
            "an association whose deletion is done",
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE,
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES,
        ),
        (
            "a registration in service",
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_VALUES,
        ),
        (
            "a registration being withdrawn",
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_IN_PROGRESS,
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_VALUES,
        ),
        (
            "a registration that has been withdrawn",
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE,
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_VALUES,
        ),
    ] {
        assert!(
            catalog.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled catalog, which \
             carries {catalog:?}"
        );
    }
    for (case, seed) in rows() {
        assert!(
            RIR_VALUES.contains(&seed.rir),
            "case {case:?}: expected the registry {rir:?} to be a member of the modelled Rir \
             catalog, which carries {RIR_VALUES:?}",
            rir = seed.rir
        );
    }
}

/// Every Regional Internet Registry the model publishes is covered by a row, so the delete
/// is asserted over the whole catalog rather than one registry.
#[test]
fn every_modelled_registry_is_covered_by_a_row() {
    for rir in RIR_VALUES {
        assert!(
            rows().iter().any(|(_, seed)| seed.rir == *rir),
            "expected a row for the registry {rir:?}, which the modelled Rir catalog carries"
        );
    }
}

/// A delete answers the association it removed in the transient `delete-in-progress` state
/// with `delete-complete` owed by the clock, and touches no other association of the account.
#[tokio::test]
async fn deleting_an_association_reports_it_deleting_and_reaches_delete_complete() {
    for (case, seed) in rows() {
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine, &seed);
        let control_id = seed_association(&engine, &CONTROL);
        // A registration whose withdrawal is FINISHED is not a dependency any more, so an
        // association that held one is still deletable.
        seed_registration(
            &engine,
            &association_id,
            "203.0.113.0/24",
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE,
        );

        let (status, body) = invoke(
            &router,
            "DeleteIpamInternetRegistryAssociation",
            &delete_params(&association_id, &[]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // The association the response reports is the one the request named, as the store
        // holds it: its own id, the ARN this service gave it, the owning account, the IPAM it
        // belongs to and that IPAM's Region, the registry and the handle at it.
        assert_eq!(
            element(&body, "ipamInternetRegistryAssociationId").as_deref(),
            Some(association_id.as_str()),
            "case {case:?}: expected the deleted association's id, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a deletion is answered while it is still under way, got {body}"
        );
        assert_eq!(
            element(&body, "stateMessage").as_deref(),
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_IN_PROGRESS_MESSAGE),
            "case {case:?}: expected the message of the state it is in, got {body}"
        );
        assert_eq!(
            element(&body, "ipamId").as_deref(),
            Some(seed.ipam_id),
            "case {case:?}: expected the IPAM the association belongs to, got {body}"
        );
        assert_eq!(
            element(&body, "ipamRegion").as_deref(),
            Some(seed.region),
            "case {case:?}: expected the Region of that IPAM, got {body}"
        );
        assert_eq!(
            element(&body, "rir").as_deref(),
            Some(seed.rir),
            "case {case:?}: expected the registry the association is with, got {body}"
        );
        assert_eq!(
            element(&body, "organizationHandle").as_deref(),
            Some(seed.organization_handle),
            "case {case:?}: expected the handle at that registry, got {body}"
        );
        assert_eq!(
            element(&body, "description").as_deref(),
            seed.description,
            "case {case:?}: expected the description the association carries, got {body}"
        );
        let arn = element(&body, "ipamInternetRegistryAssociationArn").unwrap_or_default();
        assert!(
            arn.contains(&format!(
                "{RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION}/{association_id}"
            )) && arn.contains(seed.region),
            "case {case:?}: expected the association's ARN to name it and its Region, got \
             {arn:?}"
        );
        assert!(
            element(&body, "ownerId").is_some_and(|owner| !owner.is_empty()),
            "case {case:?}: expected the account that owns the association, got {body}"
        );
        let reported_keys = elements(&element(&body, "tagSet").unwrap_or_default(), "key");
        let expected_keys: Vec<String> =
            seed.tags.iter().map(|(key, _)| (*key).to_string()).collect();
        assert_eq!(
            reported_keys, expected_keys,
            "case {case:?}: expected the tags assigned to the association, got \
             {reported_keys:?}"
        );

        // The write the response reported is in the store, the record stays there with a
        // state to report, and the control association is untouched.
        assert_eq!(
            stored_state(&engine, &association_id).as_deref(),
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: the store must hold the deletion the response reported"
        );
        assert_eq!(
            stored_state(&engine, &control_id).as_deref(),
            Some(ASSOCIATION_STATE_ENABLE_COMPLETE),
            "case {case:?}: no other association of the account may change"
        );

        // The move the clock owed falls due on its own. A later request is what reads it
        // back: the control association's own delete still succeeds, which is what
        // materialises it.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) = invoke(
            &router,
            "DeleteIpamInternetRegistryAssociation",
            &delete_params(&control_id, &[]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the control association must still be deletable, got {status} with \
             {body}"
        );
        assert_eq!(
            stored_state(&engine, &association_id).as_deref(),
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE),
            "case {case:?}: the clock owed this association its terminal state"
        );
        assert_eq!(
            stored_state_message(&engine, &association_id).as_deref(),
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_COMPLETE_MESSAGE),
            "case {case:?}: the message must move with the state it describes"
        );
    }
}

/// An id outside the form this service issues is refused as malformed before any lookup, and
/// an id of that form naming nothing the account holds is refused as not found. Nothing is
/// written by either.
#[tokio::test]
async fn an_unresolvable_id_is_refused_with_this_resources_own_code() {
    // Every id below is read against the form this service issues: `<prefix>-<8 or 17
    // lowercase hex digits>` under this resource's own prefix.
    let rows = [
        (
            // The id the recorded corpus probes: a seventeen-hex-digit suffix under a prefix
            // this API does not issue.
            "a suffix of the long form under a prefix this API does not issue",
            "ipam-ira-0000000000000000f",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "an id of another resource kind",
            "ipam-pool-0000000000000000f",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "a suffix that is not hexadecimal",
            "ipam-ir-assoc-0000000000000000g",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "an uppercase suffix",
            "ipam-ir-assoc-0000000000000000F",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "a suffix of neither published length",
            "ipam-ir-assoc-000000000",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "not an id at all",
            "my-association",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "the prefix with no suffix at all",
            "ipam-ir-assoc",
            "InvalidIpamInternetRegistryAssociationId.Malformed",
        ),
        (
            "a well-formed long-form id the account holds no association for",
            "ipam-ir-assoc-0000000000000000f",
            "InvalidIpamInternetRegistryAssociationId.NotFound",
        ),
        (
            "a well-formed short-form id the account holds no association for",
            "ipam-ir-assoc-0000000f",
            "InvalidIpamInternetRegistryAssociationId.NotFound",
        ),
    ];

    for (case, requested_id, expected_code) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let held_id = seed_association(&engine, &CONTROL);

        let (status, body) = invoke(
            &router,
            "DeleteIpamInternetRegistryAssociation",
            &delete_params(requested_id, &[]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code:?}, got body {body}"
        );
        if expected_code.ends_with(".Malformed") {
            assert_eq!(
                error_message(&body),
                format!(
                    "The {RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION} ID \
                     '{requested_id}' is malformed"
                ),
                "case {case:?}: expected the resource kind and the value the request named, \
                 got body {body}"
            );
        }
        assert_eq!(
            stored_state(&engine, &held_id).as_deref(),
            Some(ASSOCIATION_STATE_ENABLE_COMPLETE),
            "case {case:?}: a refused request must leave the account's association as it was"
        );
        assert_eq!(
            stored_count(&engine),
            1,
            "case {case:?}: a refused request must store nothing"
        );
    }
}

/// What one dependency row names: the registrations the association holds and the state each
/// is in, and what the delete must answer.
struct DependencyRow {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The registrations to seed under the association, each with the state it sits in.
    registrations: &'static [(&'static str, &'static str)],
    /// The HTTP status the delete must answer.
    expected_status: u16,
    /// The state the association must end in on a 200, or the error code on a refusal.
    expected: &'static str,
}

/// "Before deleting, you must remove all routing policy registrations associated with the
/// internet registry": an association something still depends on is refused with EC2's
/// dependency code, and one whose registrations have all been withdrawn is deletable.
#[tokio::test]
async fn an_association_with_routing_policy_registrations_cannot_be_deleted() {
    let rows = [
        DependencyRow {
            case: "no registrations at all",
            registrations: &[],
            expected_status: 200,
            expected: IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS,
        },
        DependencyRow {
            case: "one registration in service",
            registrations: &[(
                "203.0.113.0/24",
                IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
            )],
            expected_status: 400,
            expected: "DependencyViolation",
        },
        DependencyRow {
            case: "three registrations in service",
            registrations: &[
                (
                    "203.0.113.0/24",
                    IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
                ),
                (
                    "198.51.100.0/22",
                    IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
                ),
                (
                    "2001:db8::/48",
                    IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
                ),
            ],
            expected_status: 400,
            expected: "DependencyViolation",
        },
        DependencyRow {
            case: "one registration whose withdrawal has only been asked for",
            registrations: &[(
                "203.0.113.0/24",
                IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_IN_PROGRESS,
            )],
            expected_status: 400,
            expected: "DependencyViolation",
        },
        DependencyRow {
            case: "one withdrawn registration",
            registrations: &[(
                "203.0.113.0/24",
                IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE,
            )],
            expected_status: 200,
            expected: IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS,
        },
        DependencyRow {
            case: "one withdrawn registration beside one in service",
            registrations: &[
                (
                    "203.0.113.0/24",
                    IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE,
                ),
                (
                    "198.51.100.0/22",
                    IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
                ),
            ],
            expected_status: 400,
            expected: "DependencyViolation",
        },
    ];

    for DependencyRow {
        case,
        registrations,
        expected_status,
        expected,
    } in rows
    {
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine, &CONTROL);
        for (cidr, state) in registrations {
            seed_registration(&engine, &association_id, cidr, state);
        }

        let (status, body) = invoke(
            &router,
            "DeleteIpamInternetRegistryAssociation",
            &delete_params(&association_id, &[]),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            assert_eq!(
                stored_state(&engine, &association_id).as_deref(),
                Some(expected),
                "case {case:?}: the delete must have been applied"
            );
        } else {
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?}: expected the code {expected:?}, got body {body}"
            );
            // The refusal names what still has to be removed, so the caller knows what to
            // delete first: every prefix whose authorization has not been withdrawn.
            let message = error_message(&body);
            for (cidr, state) in registrations {
                assert_eq!(
                    message.contains(cidr),
                    *state != IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE,
                    "case {case:?}: the refusal must name exactly the registrations that are \
                     not withdrawn, got {message:?}"
                );
            }
            assert_eq!(
                stored_state(&engine, &association_id).as_deref(),
                Some(ASSOCIATION_STATE_ENABLE_COMPLETE),
                "case {case:?}: a refused request must leave the association as it was"
            );
        }
    }
}

/// The dependency refusal names what still has to be removed, but a one-line request cannot
/// make the emulator build a refusal the size of the whole registration store: past a bound
/// the text counts the rest instead of naming them.
#[tokio::test]
async fn the_dependency_refusal_names_a_bounded_number_of_registrations() {
    // Each row: how many registrations the association holds, and how many of them the
    // refusal may name.
    for (case, held, named) in [
        ("as many registrations as the refusal names", 10, 10),
        ("one more than the refusal names", 11, 10),
        ("far more than the refusal names", 40, 10),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine, &CONTROL);
        // A /24 per registration out of one block, so each row holds a distinct prefix.
        let prefixes: Vec<String> = (0..held)
            .map(|index| format!("198.18.{index}.0/24"))
            .collect();
        for cidr in &prefixes {
            seed_registration(
                &engine,
                &association_id,
                cidr,
                IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
            );
        }

        let (status, body) = invoke(
            &router,
            "DeleteIpamInternetRegistryAssociation",
            &delete_params(&association_id, &[]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DependencyViolation",
            "case {case:?}: expected DependencyViolation, got body {body}"
        );
        let message = error_message(&body);
        let reported = prefixes
            .iter()
            .filter(|cidr| message.contains(cidr.as_str()))
            .count();
        assert_eq!(
            reported, named,
            "case {case:?}: expected {named} of the {held} dependent prefixes to be named, got \
             {message:?}"
        );
        let unnamed = held - named;
        assert_eq!(
            message.contains(&format!("and {unnamed} more")),
            unnamed > 0,
            "case {case:?}: expected the refusal to count the prefixes it does not name, got \
             {message:?}"
        );
    }
}

/// An association whose deletion has already been asked for is refused for the state it is
/// in — while the deletion is still under way and once it is finished alike.
#[tokio::test]
async fn a_second_delete_is_refused_for_the_state_the_association_is_in() {
    for (case, wait) in [
        ("while the deletion is still under way", false),
        ("once the deletion has finished", true),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine, &CONTROL);

        let params = delete_params(&association_id, &[]);
        let (status, body) =
            invoke(&router, "DeleteIpamInternetRegistryAssociation", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        if wait {
            tokio::time::sleep(past_the_transition()).await;
        }

        let (status, body) =
            invoke(&router, "DeleteIpamInternetRegistryAssociation", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got body {body}"
        );
        let expected_state = if wait {
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE
        } else {
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS
        };
        assert_eq!(
            error_message(&body),
            format!(
                "{RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION} '{association_id}' is in \
                 the '{expected_state}' state."
            ),
            "case {case:?}: expected the state the association is in, got body {body}"
        );
        assert_eq!(
            stored_state(&engine, &association_id).as_deref(),
            Some(expected_state),
            "case {case:?}: a refused request must leave the association as it was"
        );
    }
}

/// A `DryRun` request answers the permission question with `DryRunOperation` and writes
/// nothing — whether or not the association it names exists, and whatever still depends on
/// it.
#[tokio::test]
async fn a_dry_run_answers_the_permission_question_and_writes_nothing() {
    for (case, requested_id, registrations) in [
        ("an association the account holds", None, &[][..]),
        (
            "an association something still depends on",
            None,
            &[(
                "203.0.113.0/24",
                IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
            )][..],
        ),
        (
            "a well-formed id the account holds no association for",
            Some("ipam-ir-assoc-0000000000000000f"),
            &[][..],
        ),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine, &CONTROL);
        for (cidr, state) in registrations {
            seed_registration(&engine, &association_id, cidr, state);
        }

        let (status, body) = invoke(
            &router,
            "DeleteIpamInternetRegistryAssociation",
            &delete_params(
                requested_id.unwrap_or(association_id.as_str()),
                &[("DryRun", "true")],
            ),
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got body {body}"
        );
        assert_eq!(
            stored_state(&engine, &association_id).as_deref(),
            Some(ASSOCIATION_STATE_ENABLE_COMPLETE),
            "case {case:?}: a dry run must write nothing"
        );
    }
}
