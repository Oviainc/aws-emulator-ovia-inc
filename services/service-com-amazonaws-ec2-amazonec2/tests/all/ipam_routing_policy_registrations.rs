// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteIpamRoutingPolicyRegistration`, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * Deleting a routing policy registration WITHDRAWS the authorization the association
//!   holds for that CIDR prefix: the registration is answered in the transient
//!   `delete-in-progress` state with the clock owing it `delete-complete`, the change is
//!   recorded as a delta that starts `pending` and reaches `published`, and no other
//!   registration of that association is touched.
//! * An id outside the form this service ISSUES for the resource a member names is refused
//!   with that resource's own `Invalid<Kind>.Malformed` before any lookup; an id of that
//!   form that names nothing the account holds is refused with its `Invalid<Kind>.NotFound`;
//!   and a prefix the association holds no registration for is `InvalidCidr.NotFound`.
//! * `DryRun` answers the permission question and writes nothing; one `ClientToken` names
//!   one delta; and a prefix AWS is currently announcing needs `Force`.
//!
//! Each table row is a distinct input along what those rules generalise over: the address
//! family and prefix length of the registration, how many ASNs it authorizes, whether it
//! carries a maximum length and a description, whether the request names the prefix in
//! canonical form, which part of the id is outside the issued form, and which state the
//! account's address range for the prefix is in.

use std::time::Duration;

use ec2::ops::types::{
    IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES, IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_VALUES,
    IPAM_ROUTING_POLICY_REGISTRATION_STATE_VALUES, IpamInternetRegistryAssociation,
    IpamRoutingPolicyRegistration,
};
use ec2::state::{
    BYOIP_CIDR_STATE_ADVERTISED, BYOIP_CIDR_STATE_PROVISIONED, ByoipCidrRecord,
    ComAmazonawsEc2Amazonec2State as Ec2State,
    IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX,
    IPAM_ROUTING_POLICY_REGISTRATION_DELTA_ID_PREFIX,
    IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PENDING,
    IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PUBLISHED,
    IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
    IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE,
    IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_IN_PROGRESS,
    IpamRoutingPolicyRegistrationRecord, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, unescaped};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// The IPAM the seeded association belongs to.
const IPAM_ID: &str = "ipam-0123456789abcdef0";

/// The Regional Internet Registry the seeded association is with: the modelled `Rir`
/// catalog's `arin` member (`ec2::ops::types::RIR_VALUES`).
const RIR_ARIN: &str = "arin";

/// The state the seeded association sits in: the `IpamInternetRegistryAssociationState`
/// catalog's `enable-complete` member, which
/// [`IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES`] is asserted to carry below.
const ASSOCIATION_STATE_ENABLE_COMPLETE: &str = "enable-complete";

/// The registration every row keeps beside the one it deletes, so each delete is checked to
/// touch only the prefix it names.
const CONTROL_CIDR: &str = "192.0.2.0/24";

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

/// The `IpamInternetRegistryAssociationId=` / `Cidr=` pair every request here carries, plus
/// `extra` form fields.
fn delete_params(
    association_id: &str,
    cidr: &str,
    extra: &[(&str, &str)],
) -> Vec<(String, String)> {
    let mut params = vec![
        (
            "IpamInternetRegistryAssociationId".to_string(),
            association_id.to_string(),
        ),
        ("Cidr".to_string(), cidr.to_string()),
    ];
    params.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    );
    params
}

/// One routing policy registration, as the store holds one.
fn registration(
    cidr: &str,
    asns: &[&str],
    max_length: Option<i32>,
    description: Option<&str>,
) -> IpamRoutingPolicyRegistration {
    IpamRoutingPolicyRegistration {
        cidr: Some(cidr.to_string()),
        asns: Some(asns.iter().map(|asn| (*asn).to_string()).collect()),
        permit_more_specific_announcements: Some(max_length.is_some()),
        max_length,
        description: description.map(str::to_string),
        latest_delta_id: None,
        state: None,
    }
}

/// Seed one internet registry association and answer its id.
fn seed_association(engine: &engine::InMemoryEngine) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let association_id = guard
        .insert_ipam_internet_registry_association(IpamInternetRegistryAssociation {
            ipam_id: Some(IPAM_ID.to_string()),
            ipam_region: Some(REGION.to_string()),
            rir: Some(RIR_ARIN.to_string()),
            organization_handle: Some("EXAMPLE-ORG".to_string()),
            state: Some(ASSOCIATION_STATE_ENABLE_COMPLETE.to_string()),
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

/// Seed one registration under `association_id`, in service.
fn seed_registration(
    engine: &engine::InMemoryEngine,
    association_id: &str,
    view: IpamRoutingPolicyRegistration,
) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_ipam_routing_policy_registration(
            association_id,
            IpamRoutingPolicyRegistrationRecord::in_state(
                view,
                IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
            ),
        )
        .expect("the store holds the seeded registration")
}

/// Seed one of the account's BYOIP address ranges for `cidr`, in `state`.
fn seed_byoip_range(engine: &engine::InMemoryEngine, cidr: &str, state: &str) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    assert!(
        guard.insert_byoip_cidr(ByoipCidrRecord::in_state(cidr, state)),
        "the store holds the seeded address range {cidr}"
    );
}

/// The state the stored registration for `cidr` is in right now, read straight out of the
/// store; `None` when the association holds no registration for it.
fn stored_state(
    engine: &engine::InMemoryEngine,
    association_id: &str,
    cidr: &str,
) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_routing_policy_registration(association_id, cidr)
        .map(|record| record.state().to_string())
}

/// The `LatestDeltaId` the stored registration for `cidr` carries right now.
fn stored_latest_delta_id(
    engine: &engine::InMemoryEngine,
    association_id: &str,
    cidr: &str,
) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_routing_policy_registration(association_id, cidr)
        .and_then(|record| record.view.latest_delta_id.clone())
}

/// The state the stored delta `delta_id` is in right now.
fn stored_delta_state(engine: &engine::InMemoryEngine, delta_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_routing_policy_registration_delta(delta_id)
        .and_then(|record| record.view.state.clone())
}

/// How many deltas the store holds right now.
fn stored_delta_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_routing_policy_registration_deltas
        .len()
}

/// One registration to delete: a distinct input along what the delete generalises over.
struct Row {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The prefix the registration authorizes, in canonical form.
    cidr: &'static str,
    /// The prefix as the DELETE request names it — the same prefix, written the way a
    /// caller might.
    requested_cidr: &'static str,
    /// The ASNs authorized to originate it.
    asns: &'static [&'static str],
    /// Its `MaxLength`, when it carries one.
    max_length: Option<i32>,
    /// Its description, when it carries one.
    description: Option<&'static str>,
}

/// The registrations the delete is asserted over: both address families, one and several
/// ASNs, with and without a maximum length and a description, and a prefix the request
/// names with its host bits set.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one asn, an ipv4 /24, a maximum length and a description",
            cidr: "203.0.113.0/24",
            requested_cidr: "203.0.113.0/24",
            asns: &["64512"],
            max_length: Some(24),
            description: Some("primary allocation"),
        },
        Row {
            case: "three asns, an ipv4 /22 named with host bits set, no maximum length",
            cidr: "198.51.100.0/22",
            requested_cidr: "198.51.100.37/22",
            asns: &["64512", "64513", "65000"],
            max_length: None,
            description: None,
        },
        Row {
            case: "one asn, an ipv6 /48",
            cidr: "2001:db8::/48",
            requested_cidr: "2001:db8::/48",
            asns: &["64520"],
            max_length: Some(48),
            description: Some("ipv6 roa"),
        },
    ]
}

/// The state words this suite asserts on are the model's own: each is a member of the
/// catalog the model publishes for its enum, so a row cannot pin a word the API does not
/// have.
#[test]
fn the_asserted_state_words_are_the_models_own() {
    for (case, word, catalog) in [
        (
            "the association state a seeded association sits in",
            ASSOCIATION_STATE_ENABLE_COMPLETE,
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_VALUES,
        ),
        (
            "a registration in service",
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE,
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_VALUES,
        ),
        (
            "a registration being deleted",
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_IN_PROGRESS,
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_VALUES,
        ),
        (
            "a registration whose deletion is done",
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE,
            IPAM_ROUTING_POLICY_REGISTRATION_STATE_VALUES,
        ),
        (
            "a delta that has not been published",
            IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PENDING,
            IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_VALUES,
        ),
        (
            "a published delta",
            IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PUBLISHED,
            IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_VALUES,
        ),
    ] {
        assert!(
            catalog.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled catalog, \
             which carries {catalog:?}"
        );
    }
}

/// A delete records the change as a `pending` delta, leaves the registration in the
/// transient `delete-in-progress` state with `delete-complete` owed by the clock, and
/// touches no other registration of the association.
#[tokio::test]
async fn deleting_a_registration_records_a_delta_and_reaches_delete_complete() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine);
        seed_registration(
            &engine,
            &association_id,
            registration(row.cidr, row.asns, row.max_length, row.description),
        );
        seed_registration(
            &engine,
            &association_id,
            registration(CONTROL_CIDR, &["64999"], None, None),
        );

        let (status, body) = invoke(
            &router,
            "DeleteIpamRoutingPolicyRegistration",
            &delete_params(&association_id, row.requested_cidr, &[]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // The delta the deletion created, as the response reports it.
        let delta_id = element(&body, "deltaId").unwrap_or_default();
        assert!(
            delta_id.starts_with(&format!("{IPAM_ROUTING_POLICY_REGISTRATION_DELTA_ID_PREFIX}-")),
            "case {case:?}: expected a delta id this service minted, got {delta_id:?} in {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PENDING),
            "case {case:?}: a delta is answered before it is published, got {body}"
        );
        // "The JSON specification describing the changes applied in this delta": the change
        // is the deletion of the prefix the request named, described by the registration
        // that was stored — so it carries the CANONICAL prefix and the ASNs the store held,
        // never the request's spelling of them.
        let delta_json = unescaped(&element(&body, "deltaJson").unwrap_or_default());
        assert!(
            delta_json.contains(&format!("\"cidr\":\"{cidr}\"", cidr = row.cidr)),
            "case {case:?}: expected the stored prefix in the delta, got {delta_json:?}"
        );
        for asn in row.asns {
            assert!(
                delta_json.contains(&format!("\"{asn}\"")),
                "case {case:?}: expected the authorized ASN {asn} in the delta, \
                 got {delta_json:?}"
            );
        }
        assert_eq!(
            delta_json.contains("\"maxLength\""),
            row.max_length.is_some(),
            "case {case:?}: the delta reports a maximum length only where the registration \
             carried one, got {delta_json:?}"
        );

        // The write the response reported is in the store: the registration the request
        // named is transient, carries the delta's id, and the control registration is
        // untouched.
        assert_eq!(
            stored_state(&engine, &association_id, row.cidr).as_deref(),
            Some(IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: the store must hold the deletion the response reported"
        );
        assert_eq!(
            stored_latest_delta_id(&engine, &association_id, row.cidr).as_deref(),
            Some(delta_id.as_str()),
            "case {case:?}: the registration must name the delta that changed it"
        );
        assert_eq!(
            stored_state(&engine, &association_id, CONTROL_CIDR).as_deref(),
            Some(IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE),
            "case {case:?}: no other registration of the association may change"
        );
        assert_eq!(
            stored_delta_count(&engine),
            1,
            "case {case:?}: one deletion records exactly one delta"
        );

        // The moves the clock owed fall due on their own. A later request is what reads
        // them back: the control registration's own delete still succeeds, which is what
        // materialises them.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) = invoke(
            &router,
            "DeleteIpamRoutingPolicyRegistration",
            &delete_params(&association_id, CONTROL_CIDR, &[]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the control registration must still be deletable, got {status} \
             with {body}"
        );
        assert_eq!(
            stored_state(&engine, &association_id, row.cidr).as_deref(),
            Some(IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_COMPLETE),
            "case {case:?}: the clock owed this registration its terminal state"
        );
        assert_eq!(
            stored_delta_state(&engine, &delta_id).as_deref(),
            Some(IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PUBLISHED),
            "case {case:?}: the clock owed this delta its publication"
        );
    }
}

/// What one refusal row names, and the code it must be refused with.
struct Refusal {
    /// What this row is called in a failure message.
    case: &'static str,
    /// The `IpamInternetRegistryAssociationId` to send; `None` sends the seeded
    /// association's id.
    association_id: Option<&'static str>,
    /// The `Cidr` to send; `None` sends the seeded registration's prefix.
    cidr: Option<&'static str>,
    /// The EC2 error code the request must be refused with.
    code: &'static str,
}

/// An id outside the form this service issues is refused as malformed before any lookup, an
/// id of that form naming nothing the account holds is refused as not found, and a prefix
/// the association holds no registration for is refused with the CIDR's own code. Nothing is
/// written by any of them.
#[tokio::test]
async fn an_unresolvable_id_or_prefix_is_refused_with_that_resources_own_code() {
    // Every id below is read against the form this service issues: `<prefix>-<8 or 17
    // lowercase hex digits>` under this resource's own prefix.
    let rows = [
        Refusal {
            // The id the recorded corpus probes: a seventeen-hex-digit suffix under a prefix
            // this API does not issue.
            case: "a suffix of the long form under a prefix this API does not issue",
            association_id: Some("ipam-ira-0000000000000000f"),
            cidr: None,
            code: "InvalidIpamInternetRegistryAssociationId.Malformed",
        },
        Refusal {
            case: "an id of another resource kind",
            association_id: Some("ipam-pool-0000000000000000f"),
            cidr: None,
            code: "InvalidIpamInternetRegistryAssociationId.Malformed",
        },
        Refusal {
            case: "a suffix that is not hexadecimal",
            association_id: Some("ipam-ir-assoc-0000000000000000g"),
            cidr: None,
            code: "InvalidIpamInternetRegistryAssociationId.Malformed",
        },
        Refusal {
            case: "an uppercase suffix",
            association_id: Some("ipam-ir-assoc-0000000000000000F"),
            cidr: None,
            code: "InvalidIpamInternetRegistryAssociationId.Malformed",
        },
        Refusal {
            case: "a suffix of neither published length",
            association_id: Some("ipam-ir-assoc-000000000"),
            cidr: None,
            code: "InvalidIpamInternetRegistryAssociationId.Malformed",
        },
        Refusal {
            case: "not an id at all",
            association_id: Some("my-association"),
            cidr: None,
            code: "InvalidIpamInternetRegistryAssociationId.Malformed",
        },
        Refusal {
            case: "a well-formed long-form id the account holds no association for",
            association_id: Some("ipam-ir-assoc-0000000000000000f"),
            cidr: None,
            code: "InvalidIpamInternetRegistryAssociationId.NotFound",
        },
        Refusal {
            case: "a well-formed short-form id the account holds no association for",
            association_id: Some("ipam-ir-assoc-0000000f"),
            cidr: None,
            code: "InvalidIpamInternetRegistryAssociationId.NotFound",
        },
        Refusal {
            case: "a prefix the association holds no registration for",
            association_id: None,
            cidr: Some("10.0.0.0/25"),
            code: "InvalidCidr.NotFound",
        },
        Refusal {
            case: "a prefix inside a registered one, which is a prefix of its own",
            association_id: None,
            cidr: Some("203.0.113.128/25"),
            code: "InvalidCidr.NotFound",
        },
        Refusal {
            case: "a value that is not an address prefix",
            association_id: None,
            cidr: Some("not-a-prefix"),
            code: "InvalidParameterValue",
        },
        Refusal {
            // The id's form is read BEFORE the prefix's, so the malformed id is what is
            // reported when both are wrong.
            case: "a malformed id beside a value that is not an address prefix",
            association_id: Some("ipam-ira-0000000000000000f"),
            cidr: Some("not-a-prefix"),
            code: "InvalidIpamInternetRegistryAssociationId.Malformed",
        },
    ];

    const REGISTERED_CIDR: &str = "203.0.113.0/24";
    for row in rows {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine);
        seed_registration(
            &engine,
            &association_id,
            registration(REGISTERED_CIDR, &["64512"], Some(24), None),
        );

        let (status, body) = invoke(
            &router,
            "DeleteIpamRoutingPolicyRegistration",
            &delete_params(
                row.association_id.unwrap_or(association_id.as_str()),
                row.cidr.unwrap_or(REGISTERED_CIDR),
                &[],
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected the code {expected:?}, got body {body}",
            expected = row.code,
        );
        assert_eq!(
            stored_state(&engine, &association_id, REGISTERED_CIDR).as_deref(),
            Some(IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE),
            "case {case:?}: a refused request must leave the registration as it was"
        );
        assert_eq!(
            stored_delta_count(&engine),
            0,
            "case {case:?}: a refused request must record no delta"
        );
    }
}

/// A `DryRun` request answers the permission question with `DryRunOperation` and writes
/// nothing — whether or not the registration it names exists.
#[tokio::test]
async fn a_dry_run_answers_the_permission_question_and_writes_nothing() {
    const REGISTERED_CIDR: &str = "203.0.113.0/24";
    for (case, cidr) in [
        ("a registration the association holds", REGISTERED_CIDR),
        ("a prefix it holds no registration for", "10.0.0.0/24"),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine);
        seed_registration(
            &engine,
            &association_id,
            registration(REGISTERED_CIDR, &["64512"], Some(24), None),
        );

        let (status, body) = invoke(
            &router,
            "DeleteIpamRoutingPolicyRegistration",
            &delete_params(&association_id, cidr, &[("DryRun", "true")]),
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
            stored_state(&engine, &association_id, REGISTERED_CIDR).as_deref(),
            Some(IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE),
            "case {case:?}: a dry run must write nothing"
        );
        assert_eq!(
            stored_delta_count(&engine),
            0,
            "case {case:?}: a dry run must record no delta"
        );
    }
}

/// One `ClientToken` names one delta: a repeat of the same request answers that delta and
/// changes nothing, while a repeat WITHOUT the token is refused, the registration already
/// being deleted.
#[tokio::test]
async fn one_client_token_names_one_delta() {
    const REGISTERED_CIDR: &str = "203.0.113.0/24";
    for (case, token) in [
        ("a short token", "token-1"),
        ("a token of the longest accepted length", &"t".repeat(128)),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine);
        seed_registration(
            &engine,
            &association_id,
            registration(REGISTERED_CIDR, &["64512"], Some(24), None),
        );

        let params = delete_params(
            &association_id,
            REGISTERED_CIDR,
            &[("ClientToken", token)],
        );
        let (status, body) = invoke(&router, "DeleteIpamRoutingPolicyRegistration", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let delta_id = element(&body, "deltaId").unwrap_or_default();

        // The replay: same token, same delta, still one delta stored.
        let (status, body) = invoke(&router, "DeleteIpamRoutingPolicyRegistration", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replay of the same token must succeed, got {status} with {body}"
        );
        assert_eq!(
            element(&body, "deltaId").as_deref(),
            Some(delta_id.as_str()),
            "case {case:?}: a replay must answer the delta its first call minted, got {body}"
        );
        assert_eq!(
            stored_delta_count(&engine),
            1,
            "case {case:?}: a replay must record no second delta"
        );

        // Asking again WITHOUT the token is a fresh request for a registration that is
        // already being deleted, which its state does not admit.
        let (status, body) = invoke(
            &router,
            "DeleteIpamRoutingPolicyRegistration",
            &delete_params(&association_id, REGISTERED_CIDR, &[]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got body {body}"
        );
        assert_eq!(
            stored_delta_count(&engine),
            1,
            "case {case:?}: a refused request must record no delta"
        );
    }
}

/// Deleting the authorization for a prefix that is being announced conflicts with that
/// announcement, so it needs `Force`; a prefix nothing announces needs none.
#[tokio::test]
async fn a_prefix_under_an_announced_route_needs_force() {
    const REGISTERED_CIDR: &str = "203.0.113.0/24";
    // Each row: the state the account's address range for the prefix is in, whether the
    // request sets `Force`, and what the delete must answer.
    let rows = [
        (
            "an advertised range without Force",
            Some(BYOIP_CIDR_STATE_ADVERTISED),
            false,
            400,
            "OperationNotPermitted",
        ),
        (
            "an advertised range with Force",
            Some(BYOIP_CIDR_STATE_ADVERTISED),
            true,
            200,
            "",
        ),
        (
            "a provisioned, unannounced range without Force",
            Some(BYOIP_CIDR_STATE_PROVISIONED),
            false,
            200,
            "",
        ),
        ("no range of the account at all", None, false, 200, ""),
    ];

    for (case, range_state, force, expected_status, expected_code) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let association_id = seed_association(&engine);
        seed_registration(
            &engine,
            &association_id,
            registration(REGISTERED_CIDR, &["64512"], Some(24), None),
        );
        if let Some(range_state) = range_state {
            seed_byoip_range(&engine, REGISTERED_CIDR, range_state);
        }

        let extra: Vec<(&str, &str)> = if force {
            vec![("Force", "true")]
        } else {
            Vec::new()
        };
        let (status, body) = invoke(
            &router,
            "DeleteIpamRoutingPolicyRegistration",
            &delete_params(&association_id, REGISTERED_CIDR, &extra),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            assert_eq!(
                stored_state(&engine, &association_id, REGISTERED_CIDR).as_deref(),
                Some(IPAM_ROUTING_POLICY_REGISTRATION_STATE_DELETE_IN_PROGRESS),
                "case {case:?}: the delete must have been applied"
            );
            // The forced delete records the force on the delta it wrote, so a later reader
            // can tell a forced withdrawal from an ordinary one.
            let delta_json = unescaped(&element(&body, "deltaJson").unwrap_or_default());
            assert!(
                delta_json.contains(&format!("\"force\":{force}")),
                "case {case:?}: expected the force flag in the delta, got {delta_json:?}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected the code {expected_code:?}, got body {body}"
            );
            assert_eq!(
                stored_state(&engine, &association_id, REGISTERED_CIDR).as_deref(),
                Some(IPAM_ROUTING_POLICY_REGISTRATION_STATE_CREATE_COMPLETE),
                "case {case:?}: a refused request must leave the registration as it was"
            );
            assert_eq!(
                stored_delta_count(&engine),
                0,
                "case {case:?}: a refused request must record no delta"
            );
        }
    }
}
