// SPDX-License-Identifier: CC0-1.0

//! What `CustomerOwnedIpv4Pool` does to a `CreateEndpoint` request.
//!
//! The rule: an optional member the model cross-constrains nowhere is admitted input, so
//! carrying it never changes which check the request reaches. `AccessType` states its
//! requirement in one direction only — the on-premises access type must be given the CoIP
//! pool it allocates from — so that is the only pairing refused. A pool carried with the
//! VPC access type, named explicitly or taken from the default an absent member has, goes
//! on to resolve the Outpost exactly like a request that carries no pool: the same answer
//! for an Outpost that does not resolve, and a stored endpoint reporting the pool for one
//! that does.
//!
//! Every case goes through the router, so each exercises routing, deserialization,
//! boundary validation, the handler, the persisted state and serialization.

use serde_json::{json, Value};

use crate::endpoint_lifecycle::{
    create, emulator, endpoint_with_arn, list_endpoints, list_shared, owned_outposts,
    shared_outpost, string_at, unprovisioned_emulator,
};

/// Values of `CustomerOwnedIpv4Pool`, whose pattern (from `smithy_query(query="shape",
/// shape="CreateEndpointRequest")`) is `^ipv4pool-coip-([0-9a-f]{17})$`: the lowest and
/// highest 17 hex digits and two mixed ones, none of them the id the issue cited.
const POOLS: &[&str] = &[
    "ipv4pool-coip-00000000000000000",
    "ipv4pool-coip-fffffffffffffffff",
    "ipv4pool-coip-0123456789abcdef0",
    "ipv4pool-coip-9f8e7d6c5b4a39281",
];

/// The forms of `AccessType` that do NOT require a pool: omitted, which the member's
/// documentation says is the VPC access type, and that access type named outright. Source
/// of the catalogue: `smithy_query(query="shape", shape="EndpointAccessType")`.
const WITHOUT_REQUIREMENT: &[(&str, Option<&str>)] =
    &[("AccessType omitted", None), ("AccessType Private", Some("Private"))];

/// `OutpostId`s the model's `^(op-[a-f0-9]{17}|\d{12}|ec2)$` admits that no Outpost
/// resolves for, one per branch of the pattern.
const UNRESOLVABLE_OUTPOSTS: &[(&str, &str)] = &[
    ("op- branch", "op-fffffffffffffffff"),
    ("12-digit branch", "999999999999"),
    ("ec2 branch", "ec2"),
];

/// The one text this service answers an `OutpostId` that does not resolve with, written
/// out here so a wrong constant in the service cannot make this pass.
const OUTPOST_MISSING: &str =
    "The specified outpost does not exist, or does not have S3 capacity provisioned on it.";

/// A create request for `outpost_id` with ids keyed to `index`, so no two cases commit the
/// same subnet and security group to the same Outpost.
fn request_for(outpost_id: &str, index: usize) -> Value {
    json!({
        "OutpostId": outpost_id,
        "SubnetId": format!("subnet-0000000000000e{index:03x}"),
        "SecurityGroupId": format!("sg-0000000000000e{index:03x}"),
    })
}

/// A pool carried on a request for an Outpost that does not resolve answers exactly what
/// the same request without the pool answers: the modelled 404 with the service's fixed
/// not-found text, byte-identical body — the pool reaches no check of its own.
#[tokio::test]
async fn a_coip_pool_does_not_change_the_answer_for_an_unresolvable_outpost() {
    let router = unprovisioned_emulator();

    for (branch, outpost_id) in UNRESOLVABLE_OUTPOSTS {
        let baseline = create(&router, request_for(outpost_id, 0)).await;
        assert_eq!(
            (baseline.status, baseline.error_code()),
            (404, "ResourceNotFoundException"),
            "case {branch:?}: the baseline create must be the modelled 404, got {}/{:?} with {:?}",
            baseline.status,
            baseline.error_code(),
            baseline.body
        );
        assert_eq!(
            baseline.message(),
            OUTPOST_MISSING,
            "case {branch:?}: the baseline Message must be the fixed not-found text, got {:?}",
            baseline.message()
        );

        for (form, access_type) in WITHOUT_REQUIREMENT {
            for pool in POOLS {
                let case = format!("{branch} with {form} and pool {pool}");
                let mut request = request_for(outpost_id, 0);
                if let Some(access_type) = access_type {
                    request["AccessType"] = json!(access_type);
                }
                request["CustomerOwnedIpv4Pool"] = json!(pool);

                let answer = create(&router, request).await;
                assert_eq!(
                    answer.body, baseline.body,
                    "case {case:?}: expected the baseline body {:?}, got {:?}",
                    baseline.body, answer.body
                );
                assert_eq!(
                    answer.status, baseline.status,
                    "case {case:?}: expected HTTP {}, got {}",
                    baseline.status, answer.status
                );
            }
        }

        // The one direction the documentation does state is still refused, and it is a
        // different answer from the not-found one.
        let mut customer_owned = request_for(outpost_id, 0);
        customer_owned["AccessType"] = json!("CustomerOwnedIp");
        let refused = create(&router, customer_owned).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "ValidationException"),
            "case {branch:?}: CustomerOwnedIp with no pool must be the modelled 400, got {}/{:?} \
             with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.message().contains("CustomerOwnedIpv4Pool"),
            "case {branch:?}: the refusal must name the member it is about, got {:?}",
            refused.message()
        );
    }
}

/// On an Outpost that DOES resolve, the same requests are created and the stored endpoint
/// reports the pool it was given beside the access type it was given, through both
/// endpoint listings.
#[tokio::test]
async fn a_coip_pool_is_stored_and_reported_whatever_the_access_type() {
    let router = emulator();
    let owned = owned_outposts(&router).await[0].clone();
    let shared = shared_outpost(&router).await;

    let mut cases: Vec<(String, String, Option<&str>, &str)> = Vec::new();
    for (outpost_label, outpost_id) in [("an owned Outpost", &owned), ("a shared Outpost", &shared)]
    {
        for (form, access_type) in WITHOUT_REQUIREMENT {
            for pool in POOLS {
                cases.push((
                    format!("{outpost_label}, {form}, pool {pool}"),
                    outpost_id.clone(),
                    *access_type,
                    pool,
                ));
            }
        }
    }

    for (index, (case, outpost_id, access_type, pool)) in cases.into_iter().enumerate() {
        let mut request = request_for(&outpost_id, index + 1);
        if let Some(access_type) = access_type {
            request["AccessType"] = json!(access_type);
        }
        request["CustomerOwnedIpv4Pool"] = json!(pool);

        let created = create(&router, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must answer 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_at(&created.body, "EndpointArn");

        // Read the record back through the listing that reports this caller's endpoints,
        // and — for an Outpost another account shared with this caller, the Outposts the
        // Outpost-scoped listing serves (`shared_endpoint_listing.rs`) — through that one
        // too: one stored record, the members it was given.
        let from_own = endpoint_with_arn(&list_endpoints(&router, "?maxResults=100").await, &arn);
        let from_shared = if outpost_id == shared {
            Some(endpoint_with_arn(
                &list_shared(&router, &format!("?outpostId={outpost_id}&maxResults=100")).await,
                &arn,
            ))
        } else {
            None
        };
        for (listing, endpoint) in [("ListEndpoints", Some(&from_own)), ("ListSharedEndpoints", from_shared.as_ref())]
        {
            let Some(endpoint) = endpoint else {
                continue;
            };
            assert_eq!(
                endpoint["CustomerOwnedIpv4Pool"],
                json!(pool),
                "case {case:?}: {listing} must report the pool the create was given, got {:?}",
                endpoint["CustomerOwnedIpv4Pool"]
            );
            assert_eq!(
                endpoint["AccessType"],
                json!(access_type.unwrap_or("Private")),
                "case {case:?}: {listing} must report the access type the create was given, got {:?}",
                endpoint["AccessType"]
            );
        }
        if let Some(from_shared) = &from_shared {
            assert_eq!(
                &from_own, from_shared,
                "case {case:?}: both listings report the one stored record, got {from_own:?} and \
                 {from_shared:?}"
            );
        }
    }
}
