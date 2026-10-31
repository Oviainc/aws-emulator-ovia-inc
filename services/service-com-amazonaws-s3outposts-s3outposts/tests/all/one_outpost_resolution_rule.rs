// SPDX-License-Identifier: CC0-1.0

//! The ONE Outpost resolution every door into this service's endpoint lifecycle serves.
//!
//! `CreateEndpoint`, `DeleteEndpoint` and `ListSharedEndpoints` each take an `OutpostId`,
//! and each resolves it one single way before it looks at anything else: the id must name
//! an Outpost of the inventory this caller reaches, and that Outpost must be installed in
//! the Region the request was signed for. An id that names neither is the modelled
//! `ResourceNotFoundException` (404) carrying this service's one fixed text — the same
//! answer, member for member, whichever door was knocked on; an id that names both is
//! resolved, and the door goes on to do its own work.
//!
//! What a door does once the Outpost resolves is its OWN: `ListSharedEndpoints` reports
//! endpoints for an Outpost another account shared with this caller, so an Outpost of this
//! account is outside that one door's scope and is answered with the very same refusal
//! (`shared_endpoint_listing.rs` is that scope's own subject), while the other two doors
//! serve owned and shared alike.
//!
//! Every case goes through the router, so each exercises routing, deserialization, the
//! handler, the persisted state and serialization — the path a client takes. The rows are
//! the door and the way the id resolves or fails to: the branch of `OutpostId`'s pattern,
//! an Outpost of this account, one another account shared with it, and one installed in
//! another Region than the request was signed for, each driven in more than one Region.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    assert_not_found, create_in, delete_endpoint_in, emulator_holding_shared,
    emulator_in_region_with_engine, endpoint_id_of, list_endpoints_in, list_shared_in,
    owned_outposts_in, shared_outpost_in, string_at, Answer, REGION,
};

/// One `OutpostId` per branch of its `^(op-[a-f0-9]{17}|\d{12}|ec2)$` (from
/// `smithy_query(query="shape", shape="OutpostId")`), none of which any fixture here
/// provisions: the rows differ along the branch, the `ec2` branch the issue did not cite
/// included.
const NAMES_NO_OUTPOST: &[(&str, &str)] = &[
    ("the op- branch", "op-fffffffffffffffff"),
    ("the 12-digit branch", "999999999999"),
    ("the ec2 branch", "ec2"),
];

/// A well-formed `EndpointId` (`^[a-zA-Z0-9]{19}$`) this emulator never issued, for the
/// delete rows whose Outpost is what fails to resolve.
const NEVER_ISSUED_ENDPOINT_ID: &str = "A1B2C3D4E5F6G7H8I9J";

/// A Region no fixture here provisions anything in, for the Outpost that is installed
/// somewhere other than where the request is signed for.
const OTHER_REGION: &str = "us-west-2";

/// A second Region every case is repeated in, so no row depends on one Region code.
const SECOND_REGION: &str = "eu-west-1";

/// Each operation that takes an `OutpostId` — the three doors into the endpoint lifecycle,
/// each of which resolved the Outpost for itself before the rule moved into `state`.
#[derive(Debug, Clone, Copy)]
enum Door {
    Create,
    Delete,
    ListShared,
}

/// Every door, in one table so each case drives all three.
const DOORS: &[Door] = &[Door::Create, Door::Delete, Door::ListShared];

/// A `CreateEndpoint` body whose subnet and security group are this case's own, in the
/// 17-hex-digit width `SubnetId`'s `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` and
/// `SecurityGroupId`'s `^sg-([0-9a-f]{8}|[0-9a-f]{17})$` admit: one endpoint per (Outpost,
/// subnet, security group), so a distinct `tag` keeps the creates below off each other's
/// commitments.
fn create_request(outpost_id: &str, tag: usize) -> Value {
    json!({
        "OutpostId": outpost_id,
        "SubnetId": format!("subnet-{tag:017x}"),
        "SecurityGroupId": format!("sg-{tag:017x}"),
    })
}

impl Door {
    /// The operation this door is.
    fn operation(&self) -> &'static str {
        match self {
            Door::Create => "CreateEndpoint",
            Door::Delete => "DeleteEndpoint",
            Door::ListShared => "ListSharedEndpoints",
        }
    }

    /// Knock on this door in `region` with `outpost_id`, naming `endpoint_id` where the
    /// door takes one. Nothing beyond the `OutpostId` is meant to resolve: this is how a
    /// refusal row is driven.
    async fn knock(
        &self,
        router: &MultiServiceRouter,
        region: &str,
        outpost_id: &str,
        endpoint_id: &str,
    ) -> Answer {
        match self {
            Door::Create => create_in(router, region, create_request(outpost_id, 0x1)).await,
            Door::Delete => delete_endpoint_in(router, region, outpost_id, endpoint_id).await,
            Door::ListShared => {
                list_shared_in(router, region, &format!("?outpostId={outpost_id}")).await
            }
        }
    }

    /// Drive this door in `region` at an Outpost that Region DOES resolve, and assert it
    /// got past the resolution and did its OWN work: a create whose endpoint the Outpost's
    /// listing then reports under the Region and Outpost the request resolved, the listing
    /// of exactly that Outpost's endpoints, or an accepted delete of an endpoint the same
    /// listing reported.
    ///
    /// `ListSharedEndpoints` serves the Outposts another account shared with this caller,
    /// so that door is driven here only at such an Outpost; at one this account owns it
    /// answers its own modelled 404 ([`Door::assert_outside_its_scope`]).
    ///
    /// `tag` keeps each row's commitments distinct (see [`create_request`]).
    async fn assert_served(
        &self,
        case: &str,
        router: &MultiServiceRouter,
        region: &str,
        outpost_id: &str,
        tag: usize,
    ) {
        match self {
            Door::Create => {
                let created = create_in(router, region, create_request(outpost_id, tag)).await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: an Outpost this Region resolves must be served, \
                     got {} with {:?}",
                    created.status, created.body
                );
                let arn = string_at(&created.body, "EndpointArn");
                assert!(
                    arn.contains(&format!(":s3-outposts:{region}:")),
                    "case {case:?}: the EndpointArn must name the Region the request was \
                     signed for, got {arn:?}"
                );
                assert!(
                    arn.contains(&format!(":outpost/{outpost_id}/endpoint/")),
                    "case {case:?}: the EndpointArn must name the Outpost the request \
                     resolved, got {arn:?}"
                );
                // The create STORED the endpoint: the Outpost's own endpoints report it.
                let endpoints = own_endpoints_on(router, region, outpost_id).await;
                assert!(
                    endpoints
                        .iter()
                        .any(|endpoint| endpoint["EndpointArn"].as_str() == Some(arn.as_str())),
                    "case {case:?}: the Outpost's endpoints must report the endpoint {arn:?} \
                     the create stored, got {endpoints:?}"
                );
            }
            Door::Delete => {
                // An endpoint that exists on this Outpost, reported by the Outpost's own
                // listing before the delete: so the delete below is accepted for a record
                // this Region reaches, and not for nothing.
                let endpoint_id = endpoint_on(router, region, outpost_id, tag).await;
                let before = own_endpoints_on(router, region, outpost_id).await;
                assert!(
                    reports(&before, &endpoint_id),
                    "case {case:?}: the endpoint {endpoint_id} must exist before it is \
                     deleted, got {before:?}"
                );
                let deleted = delete_endpoint_in(router, region, outpost_id, &endpoint_id).await;
                assert_eq!(
                    deleted.status, 200,
                    "case {case:?}: deleting an endpoint this Region resolves must be \
                     accepted, got {} with {:?}",
                    deleted.status, deleted.body
                );
            }
            Door::ListShared => {
                let endpoint_id = endpoint_on(router, region, outpost_id, tag).await;
                let listing = shared_listing(router, region, outpost_id).await;
                assert!(
                    reports(listing.endpoints(), &endpoint_id),
                    "case {case:?}: the listing must report the endpoint {endpoint_id} \
                     stored on this Outpost, got {:?}",
                    listing.body
                );
                // And it is scoped to the Outpost the request resolved, nothing else.
                for endpoint in listing.endpoints() {
                    assert_eq!(
                        string_at(endpoint, "OutpostsId"),
                        outpost_id,
                        "case {case:?}: every reported endpoint must be on the Outpost the \
                         request named, got {endpoint:?}"
                    );
                }
            }
        }
    }

    /// Drive this door in `region` at an Outpost that Region resolves but the door's own
    /// scope excludes, and assert the one shared refusal: `ListSharedEndpoints` serves the
    /// Outposts another account shared with this caller, so an Outpost this account owns is
    /// not one it reports endpoints for.
    async fn assert_outside_its_scope(
        &self,
        case: &str,
        router: &MultiServiceRouter,
        region: &str,
        outpost_id: &str,
    ) {
        assert!(
            matches!(self, Door::ListShared),
            "case {case:?}: only ListSharedEndpoints narrows the Outposts it serves"
        );
        let answer = self
            .knock(router, region, outpost_id, NEVER_ISSUED_ENDPOINT_ID)
            .await;
        assert_not_found(case, &answer, &[outpost_id, NEVER_ISSUED_ENDPOINT_ID]);
    }
}

/// The Outpost-scoped listing of `outpost_id` in `region`, in one page large enough for
/// every endpoint these cases store. It serves an Outpost another account shared with this
/// caller, which is the only kind it is called with here.
async fn shared_listing(router: &MultiServiceRouter, region: &str, outpost_id: &str) -> Answer {
    let listing = list_shared_in(
        router,
        region,
        &format!("?outpostId={outpost_id}&maxResults=100"),
    )
    .await;
    assert_eq!(
        listing.status,
        200,
        "reading the endpoints of {outpost_id} in {region} must answer 200, got {} with {:?}",
        listing.status,
        listing.body
    );
    listing
}

/// The endpoints of this account on `outpost_id`, read in `region` through the listing of
/// its own endpoints: the door into an Outpost's endpoints that is open whoever owns it.
async fn own_endpoints_on(
    router: &MultiServiceRouter,
    region: &str,
    outpost_id: &str,
) -> Vec<Value> {
    let listing = list_endpoints_in(router, region, "?maxResults=100").await;
    assert_eq!(
        listing.status,
        200,
        "reading this account's endpoints in {region} must answer 200, got {} with {:?}",
        listing.status,
        listing.body
    );
    listing
        .endpoints()
        .iter()
        .filter(|endpoint| string_at(endpoint, "OutpostsId") == outpost_id)
        .cloned()
        .collect()
}

/// Whether `endpoints` holds the endpoint `endpoint_id`, read off the id at the tail of
/// each reported `EndpointArn`.
fn reports(endpoints: &[Value], endpoint_id: &str) -> bool {
    endpoints
        .iter()
        .any(|endpoint| endpoint_id_of(&string_at(endpoint, "EndpointArn")) == endpoint_id)
}

/// Create one endpoint of this account on `outpost_id` through `region`'s own endpoint and
/// answer the `EndpointId` it was issued.
async fn endpoint_on(
    router: &MultiServiceRouter,
    region: &str,
    outpost_id: &str,
    tag: usize,
) -> String {
    let created = create_in(router, region, create_request(outpost_id, tag)).await;
    assert_eq!(
        created.status,
        200,
        "setup: creating an endpoint on {outpost_id} in {region} must answer 200, got {} with {:?}",
        created.status,
        created.body
    );
    endpoint_id_of(&string_at(&created.body, "EndpointArn"))
}

/// Every door serves an `OutpostId` the signing Region resolves: an Outpost of this
/// account's own and one another account shared with it are each reached through all three
/// doors, in two Regions — the Outpost-scoped listing answering its own modelled 404 for
/// the Outposts outside its scope.
#[tokio::test]
async fn every_door_serves_an_outpost_its_region_resolves() {
    for region in [REGION, SECOND_REGION] {
        let (router, _engine) = emulator_in_region_with_engine(region);
        let owned = owned_outposts_in(&router, region).await;
        assert!(
            owned.len() >= 2,
            "the fixture in {region} must hold two Outposts of this account, got {owned:?}"
        );
        let shared = shared_outpost_in(&router, region).await;
        let resolvable = [
            ("an Outpost of this account", owned[0].clone(), false),
            ("another Outpost of this account", owned[1].clone(), false),
            ("an Outpost another account shared with it", shared, true),
        ];

        let mut tag = 0x100usize;
        for (kind, outpost_id, shared_with_caller) in &resolvable {
            for door in DOORS {
                let case = format!("{} in {region} on {kind}", door.operation());
                tag += 1;
                // Every door resolves the Outpost the same way; what a door does once it
                // has is its own, and `ListSharedEndpoints` reports endpoints only for an
                // Outpost another account shared with this caller.
                if matches!(door, Door::ListShared) && !shared_with_caller {
                    door.assert_outside_its_scope(&case, &router, region, outpost_id)
                        .await;
                    continue;
                }
                door.assert_served(&case, &router, region, outpost_id, tag)
                    .await;
            }
        }
    }
}

/// An `OutpostId` the signing Region does not resolve is refused by EVERY door with one
/// identical answer: the modelled 404 and the one fixed text, whichever branch of the id's
/// pattern was used and whether the id names no Outpost at all or one installed in another
/// Region.
#[tokio::test]
async fn every_door_refuses_an_unresolvable_outpost_identically() {
    let mut bodies: Vec<(String, Value)> = Vec::new();

    // An id that names no Outpost of the inventory at all, per branch of its pattern.
    for region in [REGION, SECOND_REGION] {
        let (router, _engine) = emulator_in_region_with_engine(region);
        for (branch, outpost_id) in NAMES_NO_OUTPOST {
            for door in DOORS {
                let case = format!(
                    "{} in {region} on an id of {branch} naming no Outpost",
                    door.operation()
                );
                let answer = door
                    .knock(&router, region, outpost_id, NEVER_ISSUED_ENDPOINT_ID)
                    .await;
                assert_not_found(&case, &answer, &[outpost_id, NEVER_ISSUED_ENDPOINT_ID]);
                bodies.push((case, answer.body));
            }
        }
    }

    // An Outpost that DOES exist, with an endpoint of this account on it, installed in a
    // Region other than the one the request is signed for: the resolution is the Region's
    // too, so each door refuses the very endpoint its own Region serves.
    let outpost_id = "op-0a1b2c3d4e5f60718";
    // Shared with this caller by another account, so every door — the Outpost-scoped
    // listing included — serves it through its own Region below.
    let router = emulator_holding_shared(&[(outpost_id.to_string(), OTHER_REGION)]);
    let endpoint_id = endpoint_on(&router, OTHER_REGION, outpost_id, 0x11).await;

    for signed_for in [REGION, SECOND_REGION] {
        for door in DOORS {
            let case = format!(
                "{} signed for {signed_for} on an Outpost installed in {OTHER_REGION}",
                door.operation()
            );
            let answer = door
                .knock(&router, signed_for, outpost_id, &endpoint_id)
                .await;
            assert_not_found(&case, &answer, &[outpost_id, endpoint_id.as_str()]);
            bodies.push((case, answer.body));
        }
    }

    // The same doors and the same Outpost — through the Region the Outpost IS installed
    // in: so the rows above are refused for where the Outpost is and not because there was
    // nothing to serve.
    let mut tag = 0x20usize;
    for door in DOORS {
        let case = format!(
            "{} through the Outpost's own Region {OTHER_REGION}",
            door.operation()
        );
        tag += 1;
        door.assert_served(&case, &router, OTHER_REGION, outpost_id, tag)
            .await;
    }

    // One body for every door, branch and Region: the refusal is the one shared
    // resolution's, not something a door words for itself.
    let (first_case, first_body) = &bodies[0];
    for (case, body) in &bodies[1..] {
        assert_eq!(
            body, first_body,
            "case {case:?}: the body must be identical to {first_case:?}, got {body:?}"
        );
    }
}
