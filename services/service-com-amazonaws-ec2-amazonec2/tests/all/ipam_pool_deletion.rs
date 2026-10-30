// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteIpamPool`, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * Deleting an IPAM pool moves EXACTLY the pool the request names into the transient
//!   `delete-in-progress` state, with the clock owing it `delete-complete`, and answers that
//!   pool as the store holds it — its id, its ARN, the scope and IPAM it belongs to, its
//!   address family and its depth in its tree.
//! * A pool that still holds resources is not deletable: pools created within it, or CIDRs
//!   provisioned to it, are refused with `DependencyViolation` — unless the request carries
//!   `Cascade`, which deletes the pools within it and deprovisions the CIDRs provisioned to
//!   them; a `Cascade` the pool's scope does not admit is refused instead.
//! * An id outside the form this service ISSUES for an `IpamPoolId` is refused with
//!   `InvalidIpamPoolId.Malformed` before any lookup; an id of that form naming no pool of
//!   the account is refused with `InvalidIpamPoolId.NotFound`; and a pool whose deletion has
//!   already been asked for is refused with `IncorrectState`. None of them writes anything.
//! * `DryRun` answers the permission question with `DryRunOperation` and writes nothing,
//!   whether or not the pool it names exists.
//!
//! Each table row is a distinct input along what those rules generalise over: which address
//! family the pool holds, which scope it is created in, how deep in a tree of pools it sits,
//! how many resources are still inside it, how much of its life the clock has already run,
//! and which part of the id is outside the issued form.

use std::time::Duration;

use ec2::ops::types::{ByoipCidr, IPAM_POOL_STATE_VALUES, IpamScope};
use ec2::state::{
    ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6, BYOIP_CIDR_STATE_PROVISIONED, ByoipCidrRecord,
    ComAmazonawsEc2Amazonec2State as Ec2State, DEPENDENCY_VIOLATION, INCORRECT_STATE,
    INVALID_PARAMETER_COMBINATION, IPAM_POOL_ID_PREFIX, IPAM_POOL_STATE_CREATE_COMPLETE,
    IPAM_POOL_STATE_CREATE_IN_PROGRESS, IPAM_POOL_STATE_DELETE_COMPLETE,
    IPAM_POOL_STATE_DELETE_IN_PROGRESS, IPAM_SCOPE_TYPE_PRIVATE, IPAM_SCOPE_TYPE_PUBLIC,
    RESOURCE_TYPE_IPAM_POOL, TRANSITION_DELAY, canonical_byoip_cidr, ipam_resource_arn,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code};

/// The Region every call here is signed for, and the Region the seeded IPAM is in.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// A well-formed `IpamPoolId` in the long (17 hexadecimal digit) form that the account holds
/// no pool for.
///
/// Spelled from the top of the hexadecimal range rather than the bottom: this store mints ids
/// from a monotonic counter (`next_id`), so an id this high cannot collide with one a created
/// pool was given however many ids a run has minted.
const UNKNOWN_POOL_ID: &str = "ipam-pool-0ffffffffffffffff";

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

/// One `(name, value)` request member.
fn p(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The `IpamPoolId=` field every delete here carries, plus `extra` form fields.
fn delete_params(pool_id: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params = vec![p("IpamPoolId", pool_id)];
    params.extend(extra.iter().map(|(name, value)| p(name, value)));
    params
}

/// The IPAM a suite seeds: the ids of its two default scopes, which is what a pool is created
/// in.
struct SeededIpam {
    /// The IPAM's default public scope.
    public_scope: String,
    /// The IPAM's default private scope.
    private_scope: String,
}

/// Seed one IPAM's two default scopes — "an IPAM contains two default scopes" (`IpamScope`) —
/// and answer what a request names them by.
///
/// `CreateIpam` and `CreateIpamScope` are still scaffolds, so the scopes are seeded through
/// the store's own writer the way the `CreateIpamPool` suite beside this one seeds them; that
/// writer mints the scope id and ARN, so the ids here are the ones a real create would have
/// handed out.
fn seed_ipam(engine: &engine::InMemoryEngine) -> SeededIpam {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let ipam_id = guard.next_id("ipam");
    let ipam_arn = engine::emulator_arn("ec2", REGION, &format!("ipam/{ipam_id}"));
    let mut scope = |scope_type: &str| -> String {
        guard
            .insert_ipam_scope(IpamScope {
                ipam_arn: Some(ipam_arn.clone()),
                ipam_region: Some(REGION.to_string()),
                ipam_scope_type: Some(scope_type.to_string()),
                is_default: Some(true),
                // The `IpamScopeState` enum's settled member.
                state: Some("create-complete".to_string()),
                ..Default::default()
            })
            .expect("the store holds the seeded IPAM scope")
    };
    let public_scope = scope(IPAM_SCOPE_TYPE_PUBLIC);
    let private_scope = scope(IPAM_SCOPE_TYPE_PRIVATE);
    SeededIpam {
        public_scope,
        private_scope,
    }
}

/// Create one pool through `CreateIpamPool` and answer the `IpamPoolId` the service minted.
async fn create_pool(
    router: &MultiServiceRouter,
    scope_id: &str,
    family: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut params = vec![p("IpamScopeId", scope_id), p("AddressFamily", family)];
    params.extend(extra.iter().map(|(name, value)| p(name, value)));
    let (status, body) = invoke(router, "CreateIpamPool", &params).await;
    assert_eq!(
        status, 200,
        "creating an {family} pool in {scope_id}: got {status} with body {body}"
    );
    let pool_id = element(&body, "ipamPoolId")
        .unwrap_or_else(|| panic!("the created pool reports an id, got {body}"));
    // Every id this service ISSUES carries this resource's own prefix, which is what makes an
    // id under any other prefix malformed rather than missing.
    assert!(
        pool_id.starts_with(&format!("{IPAM_POOL_ID_PREFIX}-")),
        "the minted pool id {pool_id:?} must carry the issued prefix {IPAM_POOL_ID_PREFIX:?}"
    );
    pool_id
}

/// The state the stored pool `pool_id` is in right now, read straight out of the store;
/// `None` when the account holds no pool under that id.
fn stored_state(engine: &engine::InMemoryEngine, pool_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_pools
        .get(pool_id)
        .and_then(|pool| pool.state.clone())
}

/// What the scope `scope_id` says it holds: "the number of pools in the scope"
/// (`IpamScope$PoolCount`).
fn reported_pool_count(engine: &engine::InMemoryEngine, scope_id: &str) -> i32 {
    engine
        .state::<Ec2State>()
        .lock()
        .ipam_scopes
        .get(scope_id)
        .and_then(|scope| scope.pool_count)
        .unwrap_or_else(|| panic!("the seeded scope {scope_id} reports a pool count"))
}

/// Seed one BYOIP address range the account has brought to AWS, and answer it as the store
/// spells it.
fn seed_byoip_cidr(engine: &engine::InMemoryEngine, cidr: &str) -> String {
    let stored = engine
        .state::<Ec2State>()
        .lock()
        .insert_byoip_cidr(ByoipCidrRecord {
            byoip_cidr: ByoipCidr {
                cidr: Some(cidr.to_string()),
                state: Some(BYOIP_CIDR_STATE_PROVISIONED.to_string()),
                network_border_group: Some(REGION.to_string()),
                ..Default::default()
            },
            released_at: None,
            pending: None,
        });
    assert!(stored, "the store has room for the seeded BYOIP range {cidr}");
    canonical_byoip_cidr(cidr).unwrap_or_else(|_| panic!("{cidr} is an address range"))
}

/// The IPAM pool the stored range `cidr` is managed by right now (`ByoipCidr$IpamPoolId`).
fn managed_by(engine: &engine::InMemoryEngine, cidr: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .byoip_cidrs
        .get(cidr)
        .and_then(|record| record.byoip_cidr.ipam_pool_id.clone())
}

/// Provision `cidr` to the public-scope pool `pool_id` through `MoveByoipCidrToIpam`, which is
/// what makes it a CIDR provisioned to that pool.
async fn provision_cidr(router: &MultiServiceRouter, pool_id: &str, cidr: &str) {
    let (status, body) = invoke(
        router,
        "MoveByoipCidrToIpam",
        &[
            p("Cidr", cidr),
            p("IpamPoolId", pool_id),
            p("IpamPoolOwner", engine::emulator_account_id()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "provisioning {cidr} to {pool_id}: got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// What a delete answers and writes
// ---------------------------------------------------------------------------

/// The state words this suite asserts on are the model's own: each is a member of the catalog
/// the model publishes for `IpamPoolState`, so a row cannot pin a word the API does not have.
#[test]
fn the_asserted_state_words_are_the_models_own() {
    for (case, word) in [
        ("a pool being created", IPAM_POOL_STATE_CREATE_IN_PROGRESS),
        ("a pool in service", IPAM_POOL_STATE_CREATE_COMPLETE),
        ("a pool being deleted", IPAM_POOL_STATE_DELETE_IN_PROGRESS),
        (
            "a pool whose deletion is done",
            IPAM_POOL_STATE_DELETE_COMPLETE,
        ),
    ] {
        assert!(
            IPAM_POOL_STATE_VALUES.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled catalog, which \
             carries {IPAM_POOL_STATE_VALUES:?}"
        );
    }
}

/// One pool to delete: a distinct input along what the delete generalises over.
struct Row {
    /// What this row is called in a failure message.
    case: &'static str,
    /// Whether the pool is created in the IPAM's public scope rather than its private one.
    public: bool,
    /// The address family the pool holds.
    family: &'static str,
    /// The request members the create carries beyond the scope and the family.
    extra: &'static [(&'static str, &'static str)],
    /// Whether the pool is created WITHIN another pool of the same scope and family, which is
    /// what puts it one level deeper in its tree.
    nested: bool,
    /// Whether the clock has already finished the pool's creation when the delete arrives.
    settled: bool,
}

/// The pools the delete is asserted over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a top-level IPv4 pool in the private scope, deleted while it is still \
                   being created",
            public: false,
            family: ADDRESS_FAMILY_IPV4,
            extra: &[],
            nested: false,
            settled: false,
        },
        Row {
            case: "a described and tagged IPv6 pool in the private scope, deleted once its \
                   creation has settled",
            public: false,
            family: ADDRESS_FAMILY_IPV6,
            extra: &[
                ("Description", "space being retired"),
                ("TagSpecification.1.ResourceType", "ipam-pool"),
                ("TagSpecification.1.Tag.1.Key", "Name"),
                ("TagSpecification.1.Tag.1.Value", "retired"),
            ],
            nested: false,
            settled: true,
        },
        Row {
            case: "a top-level IPv6 pool in the public scope",
            public: true,
            family: ADDRESS_FAMILY_IPV6,
            extra: &[],
            nested: false,
            settled: false,
        },
        Row {
            case: "an IPv4 pool nested inside another pool of the private scope",
            public: false,
            family: ADDRESS_FAMILY_IPV4,
            extra: &[],
            nested: true,
            settled: false,
        },
    ]
}

/// A delete answers the pool it removed in the transient `delete-in-progress` state with
/// `delete-complete` owed by the clock, reports it as the store holds it, and touches no other
/// pool of the account.
#[tokio::test]
async fn deleting_a_pool_reports_it_deleting_and_reaches_delete_complete() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let ipam = seed_ipam(&engine);
        let scope_id = if row.public {
            ipam.public_scope.clone()
        } else {
            ipam.private_scope.clone()
        };

        let mut extra: Vec<(&str, &str)> = row.extra.to_vec();
        let source_pool = if row.nested {
            Some(create_pool(&router, &scope_id, row.family, &[]).await)
        } else {
            None
        };
        if let Some(source) = source_pool.as_deref() {
            extra.push(("SourceIpamPoolId", source));
        }
        let pool_id = create_pool(&router, &scope_id, row.family, &extra).await;
        // The control pool every row keeps beside the one it deletes, in the OTHER scope so
        // it is never a pool within the deleted one: each delete is checked to touch only the
        // pool it names.
        let control_scope = if row.public {
            ipam.private_scope.clone()
        } else {
            ipam.public_scope.clone()
        };
        let control_id = create_pool(&router, &control_scope, row.family, &[]).await;

        if row.settled {
            tokio::time::sleep(past_the_transition()).await;
        }

        let (status, body) = invoke(&router, "DeleteIpamPool", &delete_params(&pool_id, &[])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // The pool the response reports is the one the request named, as the store holds it:
        // its own id, the ARNs this service gave it, the scope it lives in, the family it
        // holds and its depth in its tree.
        assert_eq!(
            element(&body, "ipamPoolId").as_deref(),
            Some(pool_id.as_str()),
            "case {case:?}: expected the deleted pool's id, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(IPAM_POOL_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: a deletion is answered while it is still under way, got {body}"
        );
        assert_eq!(
            element(&body, "addressFamily").as_deref(),
            Some(row.family),
            "case {case:?}: expected the family the pool holds, got {body}"
        );
        let expected_scope_type = if row.public {
            IPAM_SCOPE_TYPE_PUBLIC
        } else {
            IPAM_SCOPE_TYPE_PRIVATE
        };
        assert_eq!(
            element(&body, "ipamScopeType").as_deref(),
            Some(expected_scope_type),
            "case {case:?}: expected the type of the scope the pool lives in, got {body}"
        );
        let expected_depth = if row.nested { "2" } else { "1" };
        assert_eq!(
            element(&body, "poolDepth").as_deref(),
            Some(expected_depth),
            "case {case:?}: expected the pool's depth in its tree, got {body}"
        );
        // An IPAM-family ARN carries an EMPTY Region segment — the pool's Region is reported
        // in `IpamPool$IpamRegion` — so the expected ARN is built by the one writer of them.
        assert_eq!(
            element(&body, "ipamPoolArn").as_deref(),
            Some(ipam_resource_arn(RESOURCE_TYPE_IPAM_POOL, &pool_id).as_str()),
            "case {case:?}: expected the pool's ARN to name it, got {body}"
        );
        // The same ARN spelled out, so the empty Region segment is pinned independently of
        // the writer that builds it; the account segment is the emulator's own rather than a
        // literal.
        let spelled_out = format!(
            "arn:aws:ec2::{account}:ipam-pool/{pool_id}",
            account = engine::emulator_account_id(),
        );
        assert_eq!(
            element(&body, "ipamPoolArn").as_deref(),
            Some(spelled_out.as_str()),
            "case {case:?}: expected an IPAM-family ARN with no Region segment, got {body}"
        );
        assert!(
            element(&body, "ipamArn").is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected the IPAM the pool belongs to, got {body}"
        );
        assert!(
            element(&body, "ownerId").is_some_and(|owner| !owner.is_empty()),
            "case {case:?}: expected the account that owns the pool, got {body}"
        );

        // The write the response reported is in the store, and the control pool is untouched.
        assert_eq!(
            stored_state(&engine, &pool_id).as_deref(),
            Some(IPAM_POOL_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: the store must hold the deletion the response reported"
        );
        // The scope that held it stops counting it: "the number of pools in the scope"
        // (`IpamScope$PoolCount`) is derived from the pools the scope HOLDS, and a pool a
        // delete has started on is not one of them.
        let expected_count = i32::from(row.nested);
        let reported = reported_pool_count(&engine, &scope_id);
        assert_eq!(
            reported, expected_count,
            "case {case:?}: expected the scope to report {expected_count} pools after the \
             delete, reports {reported}"
        );
        let control_before = if row.settled {
            IPAM_POOL_STATE_CREATE_COMPLETE
        } else {
            IPAM_POOL_STATE_CREATE_IN_PROGRESS
        };
        assert_eq!(
            stored_state(&engine, &control_id).as_deref(),
            Some(control_before),
            "case {case:?}: no other pool of the account may change"
        );
        if let Some(source) = source_pool.as_deref() {
            assert_eq!(
                stored_state(&engine, source).as_deref(),
                Some(control_before),
                "case {case:?}: the pool this one was created within may not change either"
            );
        }

        // The move the clock owed falls due on its own. A later request is what reads it
        // back: the control pool's own delete still succeeds, which is what materialises it.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) =
            invoke(&router, "DeleteIpamPool", &delete_params(&control_id, &[])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the control pool must still be deletable, got {status} with {body}"
        );
        assert_eq!(
            stored_state(&engine, &pool_id).as_deref(),
            Some(IPAM_POOL_STATE_DELETE_COMPLETE),
            "case {case:?}: the clock owed this pool its terminal state"
        );
    }
}

// ---------------------------------------------------------------------------
// What is still inside the pool
// ---------------------------------------------------------------------------

/// A pool that still holds resources — pools created within it, or CIDRs provisioned to it —
/// is refused, and nothing it holds is touched.
#[tokio::test]
async fn a_pool_that_still_holds_resources_is_refused() {
    // Rows: how many resources of which kind are still inside the pool, which is what the
    // dependency rule generalises over.
    for (case, children, provisioned_cidrs) in [
        ("a pool holding one pool", 1usize, 0usize),
        ("a pool holding three pools", 3, 0),
        ("a pool holding one provisioned CIDR", 0, 1),
        ("a pool holding two provisioned CIDRs", 0, 2),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let ipam = seed_ipam(&engine);
        // A CIDR is provisioned to a pool by moving a BYOIP range into it, which only a pool
        // in the public scope can hold; a pool within a pool is asserted in the private one.
        let scope_id = if provisioned_cidrs > 0 {
            ipam.public_scope.clone()
        } else {
            ipam.private_scope.clone()
        };
        let pool_id = create_pool(&router, &scope_id, ADDRESS_FAMILY_IPV4, &[]).await;

        let mut inner: Vec<String> = Vec::new();
        for _ in 0..children {
            inner.push(
                create_pool(
                    &router,
                    &scope_id,
                    ADDRESS_FAMILY_IPV4,
                    &[("SourceIpamPoolId", pool_id.as_str())],
                )
                .await,
            );
        }
        let mut cidrs: Vec<String> = Vec::new();
        for index in 0..provisioned_cidrs {
            let cidr = seed_byoip_cidr(&engine, &format!("198.51.{index}.0/24"));
            provision_cidr(&router, &pool_id, &cidr).await;
            cidrs.push(cidr);
        }

        let (status, body) = invoke(&router, "DeleteIpamPool", &delete_params(&pool_id, &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DEPENDENCY_VIOLATION,
            "case {case:?}: expected {DEPENDENCY_VIOLATION}, got body {body}"
        );
        assert_eq!(
            stored_state(&engine, &pool_id).as_deref(),
            Some(IPAM_POOL_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: a refused delete must leave the pool as it was"
        );
        for child in &inner {
            assert_eq!(
                stored_state(&engine, child).as_deref(),
                Some(IPAM_POOL_STATE_CREATE_IN_PROGRESS),
                "case {case:?}: a refused delete must leave the pools within it as they were"
            );
        }
        for cidr in &cidrs {
            assert_eq!(
                managed_by(&engine, cidr).as_deref(),
                Some(pool_id.as_str()),
                "case {case:?}: a refused delete must leave {cidr} provisioned to the pool"
            );
        }
    }
}

/// `Cascade` deletes the pool and every pool within it, however deep, and deprovisions the
/// CIDRs provisioned to each of them.
#[tokio::test]
async fn cascade_deletes_the_pools_within_the_pool() {
    // Rows: how deep the tree inside the pool goes, which is what "all resources within that
    // pool, including … other pools" generalises over.
    for (case, depth) in [
        ("a pool with one pool within it", 1usize),
        ("a pool with a chain of three pools within it", 3),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let ipam = seed_ipam(&engine);
        let pool_id = create_pool(&router, &ipam.private_scope, ADDRESS_FAMILY_IPV4, &[]).await;

        // One chain of pools, each created within the one before it.
        let mut chain: Vec<String> = Vec::new();
        let mut parent = pool_id.clone();
        for _ in 0..depth {
            let child = create_pool(
                &router,
                &ipam.private_scope,
                ADDRESS_FAMILY_IPV4,
                &[("SourceIpamPoolId", parent.as_str())],
            )
            .await;
            chain.push(child.clone());
            parent = child;
        }
        // A pool of the same scope that is in NO tree of the deleted one, which a cascade may
        // not touch.
        let bystander = create_pool(&router, &ipam.private_scope, ADDRESS_FAMILY_IPV4, &[]).await;

        let (status, body) = invoke(
            &router,
            "DeleteIpamPool",
            &delete_params(&pool_id, &[("Cascade", "true")]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(IPAM_POOL_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: the named pool is answered while its deletion is under way, got \
             {body}"
        );
        for deleted in std::iter::once(&pool_id).chain(chain.iter()) {
            assert_eq!(
                stored_state(&engine, deleted).as_deref(),
                Some(IPAM_POOL_STATE_DELETE_IN_PROGRESS),
                "case {case:?}: the pool {deleted} is within the deleted pool, so the cascade \
                 must delete it too"
            );
        }
        assert_eq!(
            stored_state(&engine, &bystander).as_deref(),
            Some(IPAM_POOL_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: a pool outside the deleted pool's tree must not change"
        );
        // Only the bystander is still a pool the scope holds.
        let reported = reported_pool_count(&engine, &ipam.private_scope);
        assert_eq!(
            reported, 1,
            "case {case:?}: expected the scope to report only the pool outside the deleted \
             tree, reports {reported}"
        );
    }
}

/// A `Cascade` the pool's own scope does not admit is refused, and the CIDRs provisioned to a
/// pool a cascade MAY delete are deprovisioned with it.
#[tokio::test]
async fn cascade_is_only_admitted_where_the_pools_scope_allows_it() {
    let (router, engine) = super::ec2_query::emulator();
    let ipam = seed_ipam(&engine);

    // "You can only use this option to delete pools in the private scope or pools in the
    // public scope with a source resource" (`Cascade`).
    let no_source = create_pool(&router, &ipam.public_scope, ADDRESS_FAMILY_IPV4, &[]).await;
    let cidr = seed_byoip_cidr(&engine, "203.0.113.0/24");
    provision_cidr(&router, &no_source, &cidr).await;

    let (status, body) = invoke(
        &router,
        "DeleteIpamPool",
        &delete_params(&no_source, &[("Cascade", "true")]),
    )
    .await;
    assert_eq!(
        status, 400,
        "a public pool with no source resource: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_COMBINATION,
        "a public pool with no source resource: expected {INVALID_PARAMETER_COMBINATION}, got \
         {body}"
    );
    assert_eq!(
        managed_by(&engine, &cidr).as_deref(),
        Some(no_source.as_str()),
        "a refused cascade must leave the CIDR provisioned to the pool"
    );

    // The same pool WITH a source resource is one the option may be used on, and the CIDR
    // provisioned to it is deprovisioned with it.
    let with_source = create_pool(
        &router,
        &ipam.public_scope,
        ADDRESS_FAMILY_IPV4,
        &[
            ("SourceResource.ResourceId", "vpc-0123456789abcdef0"),
            // The one member of the modelled `IpamPoolSourceResourceType` catalog
            // (`smithy_query(query="shape",shape="IpamPoolSourceResourceType")`).
            ("SourceResource.ResourceType", "vpc"),
            ("SourceResource.ResourceRegion", REGION),
        ],
    )
    .await;
    let planned = seed_byoip_cidr(&engine, "203.0.113.0/25");
    provision_cidr(&router, &with_source, &planned).await;

    let (status, body) = invoke(
        &router,
        "DeleteIpamPool",
        &delete_params(&with_source, &[("Cascade", "true")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "a public pool with a source resource: expected HTTP 200, got {status} with {body}"
    );
    assert_eq!(
        stored_state(&engine, &with_source).as_deref(),
        Some(IPAM_POOL_STATE_DELETE_IN_PROGRESS),
        "a cascaded delete must move the pool it names into its transient delete state"
    );
    assert_eq!(
        managed_by(&engine, &planned),
        None,
        "a cascaded delete must deprovision the CIDRs provisioned to the pool"
    );
}

// ---------------------------------------------------------------------------
// What a delete refuses
// ---------------------------------------------------------------------------

/// An id outside the form this service issues is refused as malformed before any lookup, and
/// an id of that form naming no pool of the account is refused as not found. Nothing is
/// written by either.
#[tokio::test]
async fn an_unresolvable_id_is_refused_with_this_resources_own_code() {
    // Every id below is read against the form this service issues: `<prefix>-<8 or 17
    // lowercase hex digits>` under this resource's own prefix.
    for (case, requested, code) in [
        (
            "an id of another resource kind",
            "ipam-scope-0000000000000000f",
            "InvalidIpamPoolId.Malformed",
        ),
        (
            "a suffix that is not hexadecimal",
            "ipam-pool-0000000000000000g",
            "InvalidIpamPoolId.Malformed",
        ),
        (
            "an uppercase suffix",
            "ipam-pool-000000000000000FF",
            "InvalidIpamPoolId.Malformed",
        ),
        (
            "a suffix of neither published length",
            "ipam-pool-000000000",
            "InvalidIpamPoolId.Malformed",
        ),
        ("not an id at all", "my-pool", "InvalidIpamPoolId.Malformed"),
        (
            "a well-formed long-form id the account holds no pool for",
            UNKNOWN_POOL_ID,
            "InvalidIpamPoolId.NotFound",
        ),
        (
            "a well-formed short-form id the account holds no pool for",
            "ipam-pool-0fffffff",
            "InvalidIpamPoolId.NotFound",
        ),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let ipam = seed_ipam(&engine);
        let kept = create_pool(&router, &ipam.private_scope, ADDRESS_FAMILY_IPV4, &[]).await;

        let (status, body) = invoke(&router, "DeleteIpamPool", &delete_params(requested, &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the code {code:?}, got body {body}"
        );
        assert_eq!(
            stored_state(&engine, &kept).as_deref(),
            Some(IPAM_POOL_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: a refused request must leave the account's pool as it was"
        );
    }
}

/// A pool whose deletion has already been asked for has nothing left to delete, so a second
/// delete is refused — while the deletion is under way and once the clock has finished it.
#[tokio::test]
async fn a_second_delete_is_refused_as_an_incorrect_state() {
    for (case, wait) in [
        ("while the deletion is still under way", false),
        ("once the clock has finished the deletion", true),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let ipam = seed_ipam(&engine);
        let pool_id = create_pool(&router, &ipam.private_scope, ADDRESS_FAMILY_IPV4, &[]).await;

        let (status, body) = invoke(&router, "DeleteIpamPool", &delete_params(&pool_id, &[])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        if wait {
            tokio::time::sleep(past_the_transition()).await;
        }

        let (status, body) = invoke(&router, "DeleteIpamPool", &delete_params(&pool_id, &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INCORRECT_STATE,
            "case {case:?}: expected {INCORRECT_STATE}, got body {body}"
        );
        let expected_state = if wait {
            IPAM_POOL_STATE_DELETE_COMPLETE
        } else {
            IPAM_POOL_STATE_DELETE_IN_PROGRESS
        };
        assert_eq!(
            stored_state(&engine, &pool_id).as_deref(),
            Some(expected_state),
            "case {case:?}: the refused repeat must not have restarted the deletion"
        );
    }
}

/// A `DryRun` request answers the permission question with `DryRunOperation` and writes
/// nothing — whether or not the pool it names exists; an id that is not of this kind at all is
/// still refused as malformed, because that is a question about the request's own form.
#[tokio::test]
async fn a_dry_run_answers_the_permission_question_and_writes_nothing() {
    for (case, requested, expected_status, expected_code) in [
        (
            "a pool the account holds",
            None,
            DRY_RUN_STATUS,
            "DryRunOperation",
        ),
        (
            "a well-formed id the account holds no pool for",
            Some(UNKNOWN_POOL_ID),
            DRY_RUN_STATUS,
            "DryRunOperation",
        ),
        (
            "an id outside the issued form",
            Some("ipam-pool-000000000"),
            400,
            "InvalidIpamPoolId.Malformed",
        ),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let ipam = seed_ipam(&engine);
        let pool_id = create_pool(&router, &ipam.private_scope, ADDRESS_FAMILY_IPV4, &[]).await;

        let (status, body) = invoke(
            &router,
            "DeleteIpamPool",
            &delete_params(
                requested.unwrap_or(pool_id.as_str()),
                &[("DryRun", "true")],
            ),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code:?}, got body {body}"
        );
        assert_eq!(
            stored_state(&engine, &pool_id).as_deref(),
            Some(IPAM_POOL_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: a dry run must write nothing"
        );
    }
}
