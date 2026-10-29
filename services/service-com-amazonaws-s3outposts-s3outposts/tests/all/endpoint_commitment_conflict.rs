// SPDX-License-Identifier: CC0-1.0

//! What `CreateEndpoint` refuses as an already-committed (Outpost, subnet, security
//! group), end to end over HTTP.
//!
//! The rule under test: an endpoint commits a range of the subnet it names, reached
//! through the security group it names, to the Outpost it is created on, so a second
//! endpoint repeating all three of those resources is the conflict this operation
//! models — and those resources belong to the Outpost and to EC2 rather than to any one
//! account, so the endpoint already holding them conflicts whatever account owns it. A
//! create differing in any one of the three commits resources nothing holds and is
//! accepted.
//!
//! Which account owns an endpoint decides who may report or remove it (`ListEndpoints`,
//! `DeleteEndpoint`); it never makes the resources a stored endpoint holds available to
//! another endpoint. Each row below therefore varies the owner of the record already in
//! the store (this account, the account that shared the Outpost, a third account), which
//! Outpost it sits on (one this account owns, one shared with it) and which of the three
//! members the create differs in.
//!
//! The oracle is the persisted records: every row ends by checking that no Outpost holds
//! two endpoints on one (subnet, security group), whoever owns them — a fact about the
//! store, since no one listing of this API reports every account's endpoints on an Outpost
//! this account owns.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::json;
use service_com_amazonaws_s3outposts_s3outposts::state::{
    EndpointRecord, ACCESS_TYPE_PRIVATE, ENDPOINT_STATUS_AVAILABLE,
};
use service_com_amazonaws_s3outposts_s3outposts::ComAmazonawsS3outpostsS3outpostsState;

use crate::endpoint_lifecycle::{
    create, emulator_with_engine, owned_outposts, shared_outpost, string_at, SHARING_ACCOUNT_ID,
};

/// A third synthetic account: one that is neither the account this emulator serves nor the
/// one sharing an Outpost with it, so a row can place a record owned by an account the
/// caller has no relationship with at all. Same all-zero placeholder family as
/// `engine::emulator_account_id`; no real account id appears here.
const THIRD_ACCOUNT_ID: &str = "000000000002";

/// The subnet every row's already-stored endpoint sits in.
const COMMITTED_SUBNET_ID: &str = "subnet-0c1d2e3f405162738";

/// The security group every row's already-stored endpoint reaches it through.
const COMMITTED_SECURITY_GROUP_ID: &str = "sg-0c1d2e3f405162738";

/// Who owns the endpoint a row has already stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// The account this emulator serves — the record is created through the API.
    Caller,
    /// Another account, placed through the store's own provisioning seam.
    Other(&'static str),
}

/// Which Outpost of the fixture a row's stored endpoint sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum On {
    Owned,
    Shared,
}

/// Which of the three committed members the create under test differs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Differs {
    Nothing,
    Subnet,
    SecurityGroup,
    Outpost,
}

/// Place an endpoint owned by `owner_account` on `outpost_id`, through the store's own
/// minting seams — the same counters a created endpoint draws from, so no later
/// `CreateEndpoint` can mint a colliding id, CIDR or interface.
///
/// An endpoint of another account is not something an operation of this API creates for
/// the caller, so this is the one way a row can stand one up: the state module records it
/// as a supported scenario (an Outpost shared through Resource Access Manager carrying its
/// owner's own endpoint).
fn place_endpoint_of(
    engine: &Arc<engine::InMemoryEngine>,
    owner_account: &str,
    outpost_id: &str,
    subnet_id: &str,
    security_group_id: &str,
) -> String {
    let state = engine.state::<ComAmazonawsS3outpostsS3outpostsState>();
    let mut store = state.lock();
    let endpoint_id = store.mint_endpoint_id();
    let vpc_id = store.vpc_for_subnet(subnet_id);
    let network_interface_id = store.mint_network_interface_id();
    let cidr_block = store
        .allocate_cidr_block()
        .expect("the reserved block has a free /26 in this store");
    store.endpoints.insert(
        (outpost_id.to_string(), endpoint_id.clone()),
        EndpointRecord {
            endpoint_id: endpoint_id.clone(),
            outpost_id: outpost_id.to_string(),
            cidr_block,
            status: ENDPOINT_STATUS_AVAILABLE.to_string(),
            pending: None,
            creation_time: SystemTime::now(),
            network_interfaces: vec![network_interface_id],
            vpc_id,
            subnet_id: subnet_id.to_string(),
            security_group_id: security_group_id.to_string(),
            access_type: ACCESS_TYPE_PRIVATE.to_string(),
            customer_owned_ipv4_pool: None,
            owner_account: owner_account.to_string(),
        },
    );
    endpoint_id
}

/// How many endpoints the store holds, whoever owns them.
fn stored_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = engine.state::<ComAmazonawsS3outpostsS3outpostsState>();
    let store = state.lock();
    store.endpoints.len()
}

/// Check, in the store itself, that `outpost_id` holds no two endpoints on one (subnet,
/// security group) — the invariant the conflict exists to keep, whoever owns each record.
fn one_endpoint_per_commitment(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    outpost_id: &str,
) {
    let state = engine.state::<ComAmazonawsS3outpostsS3outpostsState>();
    let store = state.lock();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for record in store
        .endpoints
        .values()
        .filter(|record| record.outpost_id == outpost_id)
    {
        let commitment = (
            record.subnet_id.clone(),
            record.security_group_id.clone(),
        );
        assert!(
            seen.insert(commitment.clone()),
            "case {case:?}: Outpost {outpost_id} must hold at most one endpoint on \
             {commitment:?}, got {:?}",
            store.endpoints
        );
    }
}

/// A create repeating an already-committed (Outpost, subnet, security group) is the
/// modelled conflict whoever owns the endpoint holding them, and a create differing in any
/// one of the three is accepted.
#[tokio::test]
async fn a_committed_subnet_and_security_group_on_one_outpost_conflicts_whoever_owns_it() {
    let cases = [
        (
            "this account's own endpoint, all three repeated",
            Owner::Caller,
            On::Owned,
            Differs::Nothing,
            409,
        ),
        (
            "the sharing account's endpoint on the Outpost it shared, all three repeated",
            Owner::Other(SHARING_ACCOUNT_ID),
            On::Shared,
            Differs::Nothing,
            409,
        ),
        (
            "a third account's endpoint on an Outpost this account owns, all three repeated",
            Owner::Other(THIRD_ACCOUNT_ID),
            On::Owned,
            Differs::Nothing,
            409,
        ),
        (
            "the sharing account's endpoint, another subnet",
            Owner::Other(SHARING_ACCOUNT_ID),
            On::Shared,
            Differs::Subnet,
            200,
        ),
        (
            "the sharing account's endpoint, another security group",
            Owner::Other(SHARING_ACCOUNT_ID),
            On::Shared,
            Differs::SecurityGroup,
            200,
        ),
        (
            "the sharing account's endpoint, another Outpost",
            Owner::Other(SHARING_ACCOUNT_ID),
            On::Shared,
            Differs::Outpost,
            200,
        ),
        (
            "a third account's endpoint, another subnet",
            Owner::Other(THIRD_ACCOUNT_ID),
            On::Owned,
            Differs::Subnet,
            200,
        ),
        (
            "this account's own endpoint, another security group",
            Owner::Caller,
            On::Owned,
            Differs::SecurityGroup,
            200,
        ),
        (
            "this account's own endpoint, another Outpost",
            Owner::Caller,
            On::Owned,
            Differs::Outpost,
            200,
        ),
    ];

    for (case, owner, on, differs, expected_status) in cases {
        // A fresh emulator per row, so each row's store holds only what the row placed.
        let (router, engine) = emulator_with_engine();
        let owned = owned_outposts(&router).await;
        let shared = shared_outpost(&router).await;
        let committed_outpost = match on {
            On::Owned => owned[0].clone(),
            On::Shared => shared.clone(),
        };
        // The Outpost a row that varies the Outpost creates on: one this account owns that
        // carries nothing yet.
        let other_outpost = owned[1].clone();

        let committed_endpoint_id = match owner {
            Owner::Caller => {
                let answer = create(
                    &router,
                    json!({
                        "OutpostId": committed_outpost,
                        "SubnetId": COMMITTED_SUBNET_ID,
                        "SecurityGroupId": COMMITTED_SECURITY_GROUP_ID,
                    }),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the endpoint under test must be created, got {} with {:?}",
                    answer.status, answer.body
                );
                crate::endpoint_lifecycle::endpoint_id_of(&string_at(&answer.body, "EndpointArn"))
            }
            Owner::Other(account) => place_endpoint_of(
                &engine,
                account,
                &committed_outpost,
                COMMITTED_SUBNET_ID,
                COMMITTED_SECURITY_GROUP_ID,
            ),
        };

        let (outpost_id, subnet_id, security_group_id) = match differs {
            Differs::Nothing => (
                committed_outpost.clone(),
                COMMITTED_SUBNET_ID.to_string(),
                COMMITTED_SECURITY_GROUP_ID.to_string(),
            ),
            Differs::Subnet => (
                committed_outpost.clone(),
                "subnet-0c1d2e3f405162739".to_string(),
                COMMITTED_SECURITY_GROUP_ID.to_string(),
            ),
            Differs::SecurityGroup => (
                committed_outpost.clone(),
                COMMITTED_SUBNET_ID.to_string(),
                "sg-0c1d2e3f405162739".to_string(),
            ),
            Differs::Outpost => (
                other_outpost,
                COMMITTED_SUBNET_ID.to_string(),
                COMMITTED_SECURITY_GROUP_ID.to_string(),
            ),
        };

        let before = stored_count(&engine);
        let answer = create(
            &router,
            json!({
                "OutpostId": outpost_id,
                "SubnetId": subnet_id,
                "SecurityGroupId": security_group_id,
            }),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );

        let after = stored_count(&engine);
        if expected_status == 409 {
            assert_eq!(
                answer.error_code(),
                "ConflictException",
                "case {case:?}: a repeated commitment is the modelled conflict, got {:?}",
                answer.body
            );
            for named in [
                committed_endpoint_id.as_str(),
                subnet_id.as_str(),
                security_group_id.as_str(),
                outpost_id.as_str(),
            ] {
                assert!(
                    answer.message().contains(named),
                    "case {case:?}: the refusal must name {named}, got {:?}",
                    answer.message()
                );
            }
            assert_eq!(
                after, before,
                "case {case:?}: a refused create must store nothing, got {after} records"
            );
        } else {
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: an accepted create must store exactly one endpoint, \
                 got {after} records"
            );
            assert!(
                !string_at(&answer.body, "EndpointArn").is_empty(),
                "case {case:?}: an accepted create must answer an EndpointArn, got {:?}",
                answer.body
            );
        }

        for outpost in owned.iter().chain(std::iter::once(&shared)) {
            one_endpoint_per_commitment(&engine, case, outpost);
        }
    }
}
