// SPDX-License-Identifier: CC0-1.0

//! The one test harness, driven through an operation of every suite that used to carry its
//! own copy of a piece of it.
//!
//! [`crate::endpoint_lifecycle`] is this crate's single home for the harness every other
//! suite drives the service through: the emulator fixtures, the request helpers, the
//! [`Answer`] reader, the listing harness — and the ONE assertion this service's modelled
//! `ResourceNotFoundException` (404) is judged by. The rule this suite pins is that single
//! home: every door that refuses an unresolvable reference is judged by that one assertion,
//! and every suite reaches the service through those shared fixtures and request helpers,
//! so a rule is written down once and no suite can hold a weaker copy of it.
//!
//! The rows are the operations the former copies were applied to — `DeleteEndpoint`,
//! `CreateEndpoint`, `ListEndpoints`, `ListSharedEndpoints` and `ListOutpostsWithS3` — and
//! the ways a reference fails to resolve: an `OutpostId` the inventory does not hold, an
//! `EndpointId` under an Outpost that does resolve, an Outpost outside the Outpost-scoped
//! listing's own scope, and an Outpost installed in a Region other than the one the request
//! was signed for. Every case goes through the router, so each exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use serde_json::Value;

use crate::endpoint_lifecycle::{
    assert_not_found, create_in, delete_endpoint_in, emulator_holding_owned,
    emulator_in_region_with_engine, endpoint_id_of, list_endpoints_in, list_outposts_in,
    list_shared_in, minimal_create, owned_outposts_in, shared_outpost_in, shared_query, string_at,
    REGION, SHARING_ACCOUNT_ID, WHOLE_PAGE,
};

/// An `OutpostId` the model's `^(op-[a-f0-9]{17}|\d{12}|ec2)$` admits (source of the
/// pattern: `smithy_query(query="shape", shape="OutpostId")`) that no fixture here
/// provisions.
const NAMES_NO_OUTPOST: &str = "op-fffffffffffffffff";

/// A well-formed `EndpointId` (`^[a-zA-Z0-9]{19}$`, from
/// `smithy_query(query="shape", shape="EndpointId")`) this emulator never issued.
const NEVER_ISSUED_ENDPOINT_ID: &str = "A1B2C3D4E5F6G7H8I9J";

/// A Region nothing is provisioned in, for the row whose Outpost is installed somewhere
/// other than where the request is signed for.
const OTHER_REGION: &str = "eu-west-1";

/// The Outpost of this account the cross-Region rows use: installed in [`OTHER_REGION`],
/// read from [`REGION`]. An id of `OutpostId`'s `op-[a-f0-9]{17}` branch.
const OUTPOST_INSTALLED_ELSEWHERE: &str = "op-2c3d4e5f607182931";

/// Who owns the Outpost of a round-trip row: the calling account, or another account that
/// shared it with the caller — which is the one kind `ListSharedEndpoints` serves.
#[derive(Debug, Clone, Copy)]
enum Owner {
    Caller,
    Sharing,
}

/// One inventory to stand up through the harness's own fixture, and the Region to drive it
/// in: the rows differ along who owns the Outpost, the branch of the `OutpostId` pattern and
/// the Region, so no case passes by matching one fixture.
struct Row {
    case: &'static str,
    outpost_id: &'static str,
    owner: Owner,
    region: &'static str,
}

const ROUND_TRIPS: &[Row] = &[
    Row {
        case: "an Outpost of this account",
        outpost_id: "op-0a1b2c3d4e5f60718",
        owner: Owner::Caller,
        region: REGION,
    },
    Row {
        case: "an Outpost another account shared with this caller, in another Region",
        outpost_id: "op-1b2c3d4e5f6071829",
        owner: Owner::Sharing,
        region: OTHER_REGION,
    },
];

/// Every door that refuses a reference this service does not resolve is judged by the ONE
/// assertion the harness owns — the modelled 404, the one fixed text, no authorization
/// wording and none of the request's own ids — and every one of them answers the very same
/// body, which is what one assertion over all of them means.
///
/// The rows are an operation from each suite that used to carry its own copy of that
/// assertion: `DeleteEndpoint`, `CreateEndpoint` and `ListSharedEndpoints`, on an Outpost
/// the inventory does not hold, on an endpoint under an Outpost that does resolve, on an
/// Outpost outside the Outpost-scoped listing's scope, and on an Outpost installed in
/// another Region than the request was signed for.
#[tokio::test]
async fn the_one_not_found_assertion_judges_every_door_that_refuses() {
    let mut bodies: Vec<(String, Value)> = Vec::new();

    let (router, _engine) = emulator_in_region_with_engine(REGION);
    let owned = owned_outposts_in(&router, REGION).await;
    let resolvable = owned
        .first()
        .expect("the fixture presents an Outpost of this account")
        .clone();
    let shared = shared_outpost_in(&router, REGION).await;

    // An `OutpostId` the inventory does not hold, through all three doors that take one.
    for (case, answer) in [
        (
            "DeleteEndpoint on an id naming no Outpost",
            delete_endpoint_in(
                &router,
                REGION,
                NAMES_NO_OUTPOST,
                NEVER_ISSUED_ENDPOINT_ID,
            )
            .await,
        ),
        (
            "CreateEndpoint on an id naming no Outpost",
            create_in(&router, REGION, minimal_create(NAMES_NO_OUTPOST)).await,
        ),
        (
            "ListSharedEndpoints on an id naming no Outpost",
            list_shared_in(&router, REGION, &shared_query(NAMES_NO_OUTPOST)).await,
        ),
    ] {
        assert_not_found(
            case,
            &answer,
            &[NAMES_NO_OUTPOST, NEVER_ISSUED_ENDPOINT_ID],
        );
        bodies.push((case.to_string(), answer.body));
    }

    // An endpoint that does not resolve under an Outpost that does.
    let case = "DeleteEndpoint on an endpoint never issued under a resolvable Outpost";
    let answer = delete_endpoint_in(&router, REGION, &resolvable, NEVER_ISSUED_ENDPOINT_ID).await;
    assert_not_found(case, &answer, &[&resolvable, NEVER_ISSUED_ENDPOINT_ID]);
    bodies.push((case.to_string(), answer.body));

    // An Outpost this account owns is outside the Outpost-scoped listing's own scope.
    let case = "ListSharedEndpoints on an Outpost this account owns";
    let answer = list_shared_in(&router, REGION, &shared_query(&resolvable)).await;
    assert_not_found(case, &answer, &[&resolvable]);
    bodies.push((case.to_string(), answer.body));

    // And an Outpost the store DOES hold, installed in another Region than the request is
    // signed for: that Outpost carries an endpoint of this account in its own Region, so
    // each door below refuses a reference its own Region serves.
    let elsewhere = emulator_holding_owned(&[(
        OUTPOST_INSTALLED_ELSEWHERE,
        engine::emulator_account_id(),
        OTHER_REGION,
    )])
    .0;
    let outpost_id = OUTPOST_INSTALLED_ELSEWHERE;
    let created = create_in(&elsewhere, OTHER_REGION, minimal_create(outpost_id)).await;
    assert_eq!(
        created.status, 200,
        "setup: creating an endpoint in the Outpost's own Region must answer 200, got {} \
         with {:?}",
        created.status, created.body
    );
    let endpoint_id = endpoint_id_of(&string_at(&created.body, "EndpointArn"));
    for (case, answer) in [
        (
            "DeleteEndpoint signed for another Region than the Outpost is installed in",
            delete_endpoint_in(&elsewhere, REGION, outpost_id, &endpoint_id).await,
        ),
        (
            "CreateEndpoint signed for another Region than the Outpost is installed in",
            create_in(&elsewhere, REGION, minimal_create(outpost_id)).await,
        ),
        (
            "ListSharedEndpoints signed for another Region than the Outpost is installed in",
            list_shared_in(&elsewhere, REGION, &shared_query(outpost_id)).await,
        ),
    ] {
        assert_not_found(case, &answer, &[outpost_id, endpoint_id.as_str()]);
        bodies.push((case.to_string(), answer.body));
    }

    // The refusal every door and every suite is judged by is ONE answer, member for member.
    assert_eq!(
        bodies.len(),
        8,
        "every door of every former copy must be driven, got {:?}",
        bodies.iter().map(|(case, _)| case).collect::<Vec<_>>()
    );
    let (first_case, first_body) = &bodies[0];
    for (case, body) in &bodies[1..] {
        assert_eq!(
            body, first_body,
            "case {case:?}: the body must be identical to {first_case:?}, got {body:?}"
        );
    }

    // The unresolvable Outpost is refused because the store holds nothing for it, not
    // because this fixture serves nothing: the Outpost it does hold is served.
    let served = list_shared_in(&router, REGION, &shared_query(&shared)).await;
    assert_eq!(
        served.status, 200,
        "an Outpost another account shared with this caller must be served, got {} with {:?}",
        served.status, served.body
    );
}

/// The harness's own fixture and request helpers serve the whole round trip every suite
/// drove through its own copies of them: one inventory stood up through the shared fixture,
/// an endpoint created, read back through both endpoint listings and the Outpost inventory,
/// then removed — whoever owns the Outpost and whichever Region it is installed in.
#[tokio::test]
async fn the_one_fixture_and_request_helpers_serve_every_listing_and_door() {
    for Row {
        case,
        outpost_id,
        owner,
        region,
    } in ROUND_TRIPS
    {
        let account = match owner {
            Owner::Caller => engine::emulator_account_id().to_string(),
            Owner::Sharing => SHARING_ACCOUNT_ID.to_string(),
        };
        let router = emulator_holding_owned(&[(outpost_id, account.as_str(), region)]).0;

        // The inventory the shared fixture provisioned is the one the control plane reports.
        let inventory = list_outposts_in(&router, region, WHOLE_PAGE).await;
        assert_eq!(
            inventory.status, 200,
            "case {case:?}: ListOutpostsWithS3 must answer 200, got {} with {:?}",
            inventory.status, inventory.body
        );
        let reported: Vec<String> = inventory
            .outposts()
            .iter()
            .map(|outpost| string_at(outpost, "OutpostId"))
            .collect();
        assert_eq!(
            reported,
            vec![outpost_id.to_string()],
            "case {case:?}: the inventory must report exactly the provisioned Outpost, got \
             {reported:?}"
        );
        assert_eq!(
            string_at(&inventory.outposts()[0], "OwnerId"),
            account,
            "case {case:?}: the Outpost must report its owning account, got {:?}",
            inventory.body
        );

        let created = create_in(&router, region, minimal_create(outpost_id)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateEndpoint must answer 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_at(&created.body, "EndpointArn");
        let endpoint_id = endpoint_id_of(&arn);

        // Read back through the listing of this account's own endpoints, which serves an
        // endpoint of this account whoever owns the Outpost it sits on.
        let own = list_endpoints_in(&router, region, WHOLE_PAGE).await;
        assert_eq!(
            own.status, 200,
            "case {case:?}: ListEndpoints must answer 200, got {} with {:?}",
            own.status, own.body
        );
        let own_arns: Vec<String> = own
            .endpoints()
            .iter()
            .map(|endpoint| string_at(endpoint, "EndpointArn"))
            .collect();
        assert_eq!(
            own_arns,
            vec![arn.clone()],
            "case {case:?}: ListEndpoints must report exactly the created endpoint, got \
             {own_arns:?}"
        );

        // The Outpost-scoped listing serves an Outpost another account shared with this
        // caller, and answers the one shared refusal for an Outpost this account owns.
        let scoped = list_shared_in(&router, region, &shared_query(outpost_id)).await;
        match owner {
            Owner::Sharing => {
                assert_eq!(
                    scoped.status, 200,
                    "case {case:?}: ListSharedEndpoints must answer 200, got {} with {:?}",
                    scoped.status, scoped.body
                );
                let scoped_arns: Vec<String> = scoped
                    .endpoints()
                    .iter()
                    .map(|endpoint| string_at(endpoint, "EndpointArn"))
                    .collect();
                assert_eq!(
                    scoped_arns,
                    vec![arn.clone()],
                    "case {case:?}: ListSharedEndpoints must report exactly the created \
                     endpoint, got {scoped_arns:?}"
                );
            }
            Owner::Caller => assert_not_found(case, &scoped, &[outpost_id, endpoint_id.as_str()]),
        }

        // And the door that removes it accepts the endpoint the listings reported.
        let deleted = delete_endpoint_in(&router, region, outpost_id, &endpoint_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteEndpoint must answer 200, got {} with {:?}",
            deleted.status, deleted.body
        );
    }
}
