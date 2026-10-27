// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateTransitGatewayRouteTable`, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: `CreateTransitGatewayRouteTable` creates an ADDITIONAL route table
//! of the transit gateway the request names ("You can use CreateTransitGatewayRouteTable to
//! create additional transit gateway route tables" — `CreateTransitGateway`), so it stores
//! one new record under a service-minted `tgw-rtb-…` id that carries the named gateway, the
//! request's tags, the creation time and neither default role — answered in the
//! non-terminal `pending` state with the clock owing it `available` — and a
//! `TransitGatewayId` the account holds no gateway for is refused with that resource's own
//! code rather than served.
//!
//! Each row of the create table is a distinct input along what that rule generalises over:
//! how many tags the table is created with (none, one, several, and the documented maximum
//! per resource), whether the gateway it is created on holds the default route tables its
//! own create owed it or none at all, and whether that gateway is still `pending` or
//! already `available` when the table is asked for. The refusal rows are one per way a
//! named transit gateway id can fail to resolve and per way a tag specification can be
//! invalid.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_TAGS_PER_RESOURCE, TRANSITION_DELAY,
    TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX, TRANSIT_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE,
    TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, elements, error_code};
use super::wire::param;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

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

/// The `TagSpecifications` form fields that tag the route table the call creates, naming
/// `resource_type`.
///
/// `CreateTransitGatewayRouteTableRequest$TagSpecifications` carries no `xmlName` of its
/// own (`smithy_query(query="shape",shape="CreateTransitGatewayRouteTableRequest")`), so
/// the field is the member name itself — unlike the creates whose member is bound to
/// `TagSpecification`.
fn tag_params(resource_type: &str, tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut params = vec![param("TagSpecifications.1.ResourceType", resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(
            &format!("TagSpecifications.1.Tag.{position}.Key"),
            key,
        ));
        params.push(param(
            &format!("TagSpecifications.1.Tag.{position}.Value"),
            value,
        ));
    }
    params
}

/// A transit gateway created through the wire with `options`, as its `TransitGatewayId`.
async fn transit_gateway(router: &MultiServiceRouter, options: &[(String, String)]) -> String {
    let (status, body) = invoke(router, "CreateTransitGateway", options).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    element(&body, "transitGatewayId").expect("a created gateway reports its id")
}

/// The route table ids the store holds, in its own stable order.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_tables
        .keys()
        .cloned()
        .collect()
}

/// The state the store holds for one route table, or `None` when it holds no such table.
fn stored_state(engine: &engine::InMemoryEngine, route_table_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_tables
        .get(route_table_id)
        .and_then(|route_table| route_table.state.clone())
}

/// The tags the store holds for one route table, as (key, value) pairs in stored order.
fn stored_tags(engine: &engine::InMemoryEngine, route_table_id: &str) -> Vec<(String, String)> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_tables
        .get(route_table_id)
        .and_then(|route_table| route_table.tags.clone())
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag.key.clone().unwrap_or_default(),
                tag.value.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// One create, as a row of the table.
struct Case {
    /// The case name every assertion of the row quotes.
    name: &'static str,
    /// The options the gateway the table is created on was created with, which decides
    /// whether that gateway holds any default route table of its own.
    gateway_options: Vec<(String, String)>,
    /// Whether the gateway is left to reach `available` before the table is asked for.
    on_an_available_gateway: bool,
    /// The tags the route table is created with.
    tags: Vec<(String, String)>,
}

/// `count` distinct tags, for the row that asks for as many as EC2 documents per resource.
fn many_tags(count: usize) -> Vec<(String, String)> {
    (1..=count)
        .map(|nth| (format!("key-{nth}"), format!("value-{nth}")))
        .collect()
}

/// One (key, value) pair, as the table rows spell their tags.
fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Creating a route table stores one new table of the named gateway and answers it as
/// stored: a service-minted id, the gateway the request named, `pending`, neither default
/// role, a creation time and the request's tags. The clock then owes the table
/// `available`, and the implemented reader of a route table's propagations resolves the new
/// id — which an id no create minted is refused for.
#[tokio::test]
async fn creating_a_route_table_stores_one_table_of_the_named_gateway() {
    let cases = vec![
        Case {
            name: "no tags, on a gateway holding the default route table its create owed it",
            gateway_options: Vec::new(),
            on_an_available_gateway: false,
            tags: Vec::new(),
        },
        Case {
            name: "one tag, on a gateway that has become available",
            gateway_options: Vec::new(),
            on_an_available_gateway: true,
            tags: vec![tag("Name", "spoke")],
        },
        Case {
            name: "three tags, on a gateway that holds no default route table at all",
            gateway_options: vec![
                param("Options.DefaultRouteTableAssociation", "disable"),
                param("Options.DefaultRouteTablePropagation", "disable"),
            ],
            on_an_available_gateway: false,
            // An empty tag value is a value EC2 accepts, so one row carries it.
            tags: vec![tag("Env", "prod"), tag("Tier", "edge"), tag("Owner", "")],
        },
        Case {
            name: "as many tags as EC2 documents per resource",
            gateway_options: vec![param("Options.DefaultRouteTablePropagation", "disable")],
            on_an_available_gateway: false,
            tags: many_tags(MAX_TAGS_PER_RESOURCE),
        },
    ];

    for case in cases {
        let Case {
            name,
            gateway_options,
            on_an_available_gateway,
            tags,
        } = case;
        let (router, engine) = super::ec2_query::emulator();
        let transit_gateway_id = transit_gateway(&router, &gateway_options).await;
        if on_an_available_gateway {
            tokio::time::sleep(past_the_transition()).await;
        }
        let before = stored_ids(&engine);

        let mut request = vec![param("TransitGatewayId", &transit_gateway_id)];
        if !tags.is_empty() {
            request.extend(tag_params(
                ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY_ROUTE_TABLE,
                &tags,
            ));
        }
        let (status, body) = invoke(&router, "CreateTransitGatewayRouteTable", &request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from the create, got {status} with body {body}"
        );

        let route_table_id = element(&body, "transitGatewayRouteTableId")
            .unwrap_or_else(|| panic!("case {name:?}: the create must report an id, got {body}"));
        assert!(
            route_table_id.starts_with(&format!("{TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX}-")),
            "case {name:?}: expected a minted route table id, got {route_table_id:?}"
        );
        assert!(
            !before.contains(&route_table_id),
            "case {name:?}: the create must mint a NEW id, got {route_table_id:?} which the \
             store already held in {before:?}"
        );
        assert_eq!(
            element(&body, "transitGatewayId").as_deref(),
            Some(transit_gateway_id.as_str()),
            "case {name:?}: expected the gateway the request named, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING),
            "case {name:?}: expected the non-terminal state of a create, got {body}"
        );
        assert_eq!(
            element(&body, "defaultAssociationRouteTable").as_deref(),
            Some("false"),
            "case {name:?}: an additional route table is not the default association table, \
             got {body}"
        );
        assert_eq!(
            element(&body, "defaultPropagationRouteTable").as_deref(),
            Some("false"),
            "case {name:?}: an additional route table is not the default propagation table, \
             got {body}"
        );
        let creation_time = element(&body, "creationTime").unwrap_or_else(|| {
            panic!("case {name:?}: the create must report a creation time, got {body}")
        });
        assert!(
            as_timestamp(&creation_time).is_some(),
            "case {name:?}: expected a modelled timestamp, got {creation_time:?}"
        );
        assert_eq!(
            elements(&body, "key"),
            tags.iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<String>>(),
            "case {name:?}: expected the tag keys the request asked for, got {body}"
        );
        for (key, value) in tags.iter().filter(|(_, value)| !value.is_empty()) {
            assert!(
                body.contains(&format!("<value>{value}</value>")),
                "case {name:?}: expected the value of the tag {key:?} to be reported, got \
                 {body}"
            );
        }

        // Exactly ONE record is added, and it is the one the response reported.
        let after = stored_ids(&engine);
        assert_eq!(
            after.len(),
            before.len() + 1,
            "case {name:?}: expected one new stored route table, had {before:?} and now hold \
             {after:?}"
        );
        assert!(
            after.contains(&route_table_id),
            "case {name:?}: the store must hold the table the create reported, got {after:?}"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id).as_deref(),
            Some(TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING),
            "case {name:?}: the store must hold the state the response reported"
        );
        assert_eq!(
            stored_tags(&engine, &route_table_id),
            tags,
            "case {name:?}: the store must hold the tags the request asked for"
        );

        // The round trip: the implemented reader of a route table's propagations resolves
        // the new id and reports no propagation, because creating a route table propagates
        // nothing into it — while a well-formed id no create ever minted is refused by that
        // same reader, so the success above is the stored record and not a blanket answer.
        let read = vec![param("TransitGatewayRouteTableId", &route_table_id)];
        let (status, body) =
            invoke(&router, "GetTransitGatewayRouteTablePropagations", &read).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the created route table must be readable, got {status} with body \
             {body}"
        );
        assert_eq!(
            elements(&body, "transitGatewayAttachmentId"),
            Vec::<String>::new(),
            "case {name:?}: a route table a create just made propagates nothing, got {body}"
        );
        let (status, body) = invoke(
            &router,
            "GetTransitGatewayRouteTablePropagations",
            &[param(
                "TransitGatewayRouteTableId",
                "tgw-rtb-0000000000000000f",
            )],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: a route table no create minted must be refused, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
            "case {name:?}: expected the route table's own not-found code, got {body}"
        );

        // The move the clock owed falls due on its own, without anybody asking for it.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) =
            invoke(&router, "GetTransitGatewayRouteTablePropagations", &read).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the route table must still be readable, got {status} with body \
             {body}"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id).as_deref(),
            Some(TRANSIT_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE),
            "case {name:?}: the clock owed this route table its available state"
        );
    }
}

/// A `TransitGatewayId` the account holds no transit gateway for has nothing to add a route
/// table to, and a tag specification outside what this action may tag is invalid: each is
/// refused with its own documented code — never with a 501 or an empty success — and
/// nothing is stored.
///
/// The rows are one per way a named gateway id can fail to resolve, and one per way a tag
/// specification can be invalid.
#[tokio::test]
async fn a_request_that_does_not_resolve_is_refused_with_its_own_code() {
    let (router, engine) = super::ec2_query::emulator();
    let transit_gateway_id = transit_gateway(&router, &[]).await;
    let default_route_table_id = stored_ids(&engine)
        .first()
        .cloned()
        .expect("a gateway created with the documented defaults owns a default route table");
    let before = stored_ids(&engine);

    // (case, the request's fields, the HTTP status and the code expected)
    type RefusalCase = (&'static str, Vec<(String, String)>, u16, &'static str);
    let cases: Vec<RefusalCase> = vec![
        (
            "a transit gateway id of the right form that names no gateway",
            vec![param("TransitGatewayId", "tgw-0123456789abcdef0")],
            400,
            ec2::state::INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "a short-form transit gateway id that names no gateway",
            vec![param("TransitGatewayId", "tgw-0000000f")],
            400,
            ec2::state::INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "something that is not a transit gateway id at all",
            vec![param("TransitGatewayId", "tgw-not-an-id")],
            400,
            ec2::state::INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "a route table id where a gateway id belongs",
            vec![param("TransitGatewayId", &default_route_table_id)],
            400,
            ec2::state::INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "a tag specification naming another resource type",
            {
                let mut params = vec![param("TransitGatewayId", &transit_gateway_id)];
                params.extend(tag_params(
                    ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY,
                    &[tag("Name", "misdirected")],
                ));
                params
            },
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a tag specification that names no resource type",
            vec![
                param("TransitGatewayId", &transit_gateway_id),
                param("TagSpecifications.1.Tag.1.Key", "Name"),
                param("TagSpecifications.1.Tag.1.Value", "untyped"),
            ],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "a tag key in the reserved namespace",
            {
                let mut params = vec![param("TransitGatewayId", &transit_gateway_id)];
                params.extend(tag_params(
                    ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY_ROUTE_TABLE,
                    &[tag("aws:created-by", "me")],
                ));
                params
            },
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "one tag key given twice",
            {
                let mut params = vec![param("TransitGatewayId", &transit_gateway_id)];
                params.extend(tag_params(
                    ec2::state::RESOURCE_TYPE_TRANSIT_GATEWAY_ROUTE_TABLE,
                    &[tag("Name", "first"), tag("Name", "second")],
                ));
                params
            },
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a dry run of a request that would have succeeded",
            vec![
                param("TransitGatewayId", &transit_gateway_id),
                param("DryRun", "true"),
            ],
            DRY_RUN_STATUS,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];

    for (name, request, expected_status, expected_code) in &cases {
        let (status, body) = invoke(&router, "CreateTransitGatewayRouteTable", request).await;
        assert_eq!(
            status, *expected_status,
            "case {name:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, *expected_code,
            "case {name:?}: expected the code {expected_code:?}, got {code:?} in {body}"
        );
    }

    let after = stored_ids(&engine);
    assert_eq!(
        after, before,
        "a refused create must store nothing: expected {before:?}, got {after:?}"
    );
}
