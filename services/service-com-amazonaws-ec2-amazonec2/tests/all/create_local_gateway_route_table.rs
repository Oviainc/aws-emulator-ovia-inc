// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateLocalGatewayRouteTable` and the
//! `DescribeLocalGatewayRouteTables` that reads one back, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the create COMPUTES its answer from the store: it writes a
//! route table for a local gateway the account HOLDS, mints the table id, its ARN and the
//! owning account itself, reads the Outpost the table belongs to off the stored GATEWAY,
//! answers the non-terminal `pending` state the clock owes `available`, and refuses a
//! request naming a gateway the account does not hold with that resource's own
//! `InvalidLocalGatewayID.NotFound` (and an id that is not a local gateway id at all with
//! its `.Malformed`). Every table row below is a distinct input along what that generalises
//! over — which gateway, on which Outpost, in which Region, under which mode (including
//! none), how many tags, which resource type a tag specification names, and each way a
//! `LocalGatewayId` can fail to resolve.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_LOCAL_GATEWAY_ID_MALFORMED,
    INVALID_LOCAL_GATEWAY_ID_NOT_FOUND, INVALID_PARAMETER_VALUE,
    LOCAL_GATEWAY_ROUTE_TABLE_ID_PREFIX, LOCAL_GATEWAY_ROUTE_TABLE_MODE_DIRECT_VPC_ROUTING,
    LOCAL_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE, LOCAL_GATEWAY_ROUTE_TABLE_STATE_PENDING,
    LocalGatewaySpec, MISSING_PARAMETER, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};

/// The Region most calls here are signed for: a route table's own ARN names the Region its
/// creating call was signed for, which is why one row drives [`OTHER_REGION`].
const REGION: &str = "us-east-1";

/// The second Region, so the ARN a created table carries is visibly the signed Region's
/// rather than a constant.
const OTHER_REGION: &str = "us-west-2";

/// The ec2Query action under test.
const CREATE: &str = "CreateLocalGatewayRouteTable";

/// The reader this suite proves a created route table round-trips through.
const DESCRIBE: &str = "DescribeLocalGatewayRouteTables";

/// The XML element each route table's id arrives in
/// (`LocalGatewayRouteTable$LocalGatewayRouteTableId`'s `xmlName`).
const ID_ELEMENT: &str = "localGatewayRouteTableId";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// Long enough past [`TRANSITION_DELAY`] that the move the clock owes a new route table is
/// certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `CreateLocalGatewayRouteTable` call signed for `region`, as its status and body.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, CREATE, params).await
}

/// One `CreateLocalGatewayRouteTable` call signed for [`REGION`].
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    create_in(router, REGION, params).await
}

/// One `DescribeLocalGatewayRouteTables` call, as its status and body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, DESCRIBE, params).await
}

/// The form fields for one `TagSpecification` naming `resource_type` and `tags`.
fn tag_specification(resource_type: &str, tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields = vec![param("TagSpecification.1.ResourceType", resource_type)];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        fields.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
        fields.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    fields
}

/// The ARN of a local gateway route table, as
/// `LocalGatewayRouteTable$LocalGatewayRouteTableArn` carries it:
/// `arn:aws:ec2:<region>:<account>:local-gateway-route-table/<id>`.
fn route_table_arn(region: &str, route_table_id: &str) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:local-gateway-route-table/{route_table_id}",
        engine::emulator_account_id()
    )
}

/// The ARN of the Outpost `name`, as `LocalGateway$OutpostArn` carries it.
fn outpost_arn(name: &str) -> String {
    format!(
        "arn:aws:outposts:{REGION}:{}:outpost/{name}",
        engine::emulator_account_id()
    )
}

/// One seeded local gateway: the Outpost rack it is part of and the state it is in. Nothing
/// in the EC2 API this emulator serves creates an Outpost, so a gateway is written straight
/// through the store's own seam — the one a gateway placed by an Outpost order would use.
struct GatewayRow {
    outpost: &'static str,
    /// "The state of the local gateway" (`LocalGateway$State`), an unconstrained string in
    /// the model.
    state: &'static str,
}

/// The gateways this suite creates route tables on: distinct Outposts, and one gateway in a
/// state of its own, so a created table's reported Outpost can only have come from the
/// gateway it was created on.
fn gateway_rows() -> Vec<GatewayRow> {
    vec![
        GatewayRow {
            outpost: "op-00000000000000001",
            state: "available",
        },
        GatewayRow {
            outpost: "op-00000000000000002",
            state: "available",
        },
        GatewayRow {
            outpost: "op-00000000000000003",
            state: "pending",
        },
    ]
}

/// Write the seeded local gateways into the engine's store and report the ids they were
/// given, in row order.
fn seed_gateways(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    gateway_rows()
        .into_iter()
        .map(|row| {
            let case = row.outpost;
            guard
                .insert_local_gateway(LocalGatewaySpec {
                    outpost_arn: outpost_arn(row.outpost),
                    state: row.state.to_string(),
                    tags: Vec::new(),
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store must take the seeded local gateway")
                })
        })
        .collect()
}

/// How many route tables the store holds.
fn stored_tables(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .local_gateway_route_tables
        .len()
}

/// Assert that `body` carries `<member>value</member>`.
fn assert_member(case: &str, body: &str, member: &str, value: &str) {
    assert!(
        body.contains(&format!("<{member}>{value}</{member}>")),
        "case {case:?}: expected <{member}>{value}</{member}>, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The create computes its answer from the store
// ---------------------------------------------------------------------------

/// One row of the create table: which seeded gateway the table is created on, in which
/// Region the call is signed, the `Mode` the request names (`None` for a request that names
/// none) and the tags it asks for.
struct CreateRow {
    name: &'static str,
    gateway: usize,
    region: &'static str,
    mode: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The creates, spanning what the rule generalises over: both members of the model's
/// `LocalGatewayRouteTableMode` catalog and a request naming no mode at all, two Regions,
/// three gateways on three Outposts (one of them used twice, so a gateway can hold more than
/// one table), and none, one and two tags.
fn create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            name: "coip, no tags",
            gateway: 0,
            region: REGION,
            mode: Some("coip"),
            tags: Vec::new(),
        },
        CreateRow {
            name: "direct-vpc-routing, one tag",
            gateway: 0,
            region: REGION,
            mode: Some("direct-vpc-routing"),
            tags: vec![("Name", "rack-a")],
        },
        CreateRow {
            name: "no mode named, two tags",
            gateway: 1,
            region: REGION,
            mode: None,
            tags: vec![("Name", "rack-b"), ("Owner", "TeamB")],
        },
        CreateRow {
            name: "another Region",
            gateway: 2,
            region: OTHER_REGION,
            mode: Some("coip"),
            tags: vec![("Name", "rack-c")],
        },
    ]
}

/// Each created route table is answered as the store holds it — the minted id and ARN, the
/// gateway the request named, the Outpost read off that GATEWAY, the emulator's account, the
/// mode asked for (or `direct-vpc-routing` when the request named none) and every tag — and
/// reads back through `DescribeLocalGatewayRouteTables` with the same members.
#[tokio::test]
async fn each_created_route_table_is_stored_and_reads_back() {
    let (router, engine) = super::ec2_query::emulator();
    let gateways = seed_gateways(&engine);
    let mut minted: Vec<String> = Vec::new();

    for row in create_rows() {
        let case = row.name;
        let gateway_id = &gateways[row.gateway];
        let outpost = outpost_arn(gateway_rows()[row.gateway].outpost);
        let expected_mode = row
            .mode
            .unwrap_or(LOCAL_GATEWAY_ROUTE_TABLE_MODE_DIRECT_VPC_ROUTING);

        let mut params = vec![param("LocalGatewayId", gateway_id)];
        if let Some(mode) = row.mode {
            params.push(param("Mode", mode));
        }
        if !row.tags.is_empty() {
            params.extend(tag_specification("local-gateway-route-table", &row.tags));
        }

        let (status, body) = create_in(&router, row.region, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {CREATE}, got {status} with body {body}"
        );

        let route_table_id = element(&body, ID_ELEMENT)
            .unwrap_or_else(|| panic!("case {case:?}: no <{ID_ELEMENT}> in {body}"));
        assert!(
            route_table_id.starts_with(&format!("{LOCAL_GATEWAY_ROUTE_TABLE_ID_PREFIX}-")),
            "case {case:?}: expected an id minted with the route table prefix, got \
             {route_table_id:?}"
        );
        assert!(
            !minted.contains(&route_table_id),
            "case {case:?}: expected a fresh route table id, got the already-minted \
             {route_table_id:?}"
        );

        for (member, value) in [
            (
                "localGatewayRouteTableArn",
                route_table_arn(row.region, &route_table_id),
            ),
            ("localGatewayId", gateway_id.clone()),
            ("outpostArn", outpost.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("mode", expected_mode.to_string()),
            (
                "state",
                LOCAL_GATEWAY_ROUTE_TABLE_STATE_PENDING.to_string(),
            ),
        ] {
            assert_member(case, &body, member, &value);
        }
        for (key, value) in &row.tags {
            assert_member(case, &body, "key", key);
            assert_member(case, &body, "value", value);
        }

        // The round trip: the table reads back through the describe with the same members, so
        // the create answered the record it stored rather than its own request.
        let (read_status, read_body) = describe(
            &router,
            &indexed("LocalGatewayRouteTableId", &[route_table_id.as_str()]),
        )
        .await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected HTTP 200 from {DESCRIBE}, got {read_status} with body \
             {read_body}"
        );
        assert_eq!(
            elements(&read_body, ID_ELEMENT),
            vec![route_table_id.clone()],
            "case {case:?}: expected {DESCRIBE} to report exactly the created route table, got \
             {read_body}"
        );
        for (member, value) in [
            (
                "localGatewayRouteTableArn",
                route_table_arn(row.region, &route_table_id),
            ),
            ("localGatewayId", gateway_id.clone()),
            ("outpostArn", outpost.clone()),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("mode", expected_mode.to_string()),
        ] {
            assert_member(case, &read_body, member, &value);
        }
        for (key, value) in &row.tags {
            assert_member(case, &read_body, "key", key);
            assert_member(case, &read_body, "value", value);
        }

        minted.push(route_table_id);
    }

    assert_eq!(
        stored_tables(&engine),
        create_rows().len(),
        "every created route table must be stored once, got {}",
        stored_tables(&engine)
    );
}

/// A new route table is answered in the non-terminal `pending` state, and the clock owes it
/// `available` one `TRANSITION_DELAY` later whether or not anybody calls again.
#[tokio::test]
async fn a_route_table_starts_pending_and_the_clock_owes_it_available() {
    let (router, engine) = super::ec2_query::emulator();
    let gateways = seed_gateways(&engine);

    let (status, body) = create(&router, &[param("LocalGatewayId", &gateways[0])]).await;
    assert_eq!(
        status, 200,
        "the clock: expected HTTP 200 from {CREATE}, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(LOCAL_GATEWAY_ROUTE_TABLE_STATE_PENDING),
        "the clock: a new route table starts in the transient state, got {body}"
    );
    let route_table_id =
        element(&body, ID_ELEMENT).unwrap_or_else(|| panic!("the clock: no <{ID_ELEMENT}> in {body}"));

    tokio::time::sleep(past_the_transition()).await;

    let (read_status, read_body) = describe(&router, &[]).await;
    assert_eq!(
        read_status, 200,
        "the clock: expected HTTP 200 from {DESCRIBE}, got {read_status} with body {read_body}"
    );
    assert_eq!(
        elements(&read_body, ID_ELEMENT),
        vec![route_table_id.clone()],
        "the clock: expected the created table to be reported, got {read_body}"
    );
    assert_eq!(
        element(&read_body, "state").as_deref(),
        Some(LOCAL_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE),
        "the clock: the move owed one TRANSITION_DELAY after the create must have been \
         applied, got {read_body}"
    );
}

/// Two creates naming the same gateway are two route tables: each gets its own id and both
/// are reported, so the store is what the answer is computed from rather than one fixed
/// record.
#[tokio::test]
async fn two_creates_on_one_gateway_make_two_route_tables() {
    let (router, engine) = super::ec2_query::emulator();
    let gateways = seed_gateways(&engine);
    let params = [param("LocalGatewayId", &gateways[0])];

    let mut ids = Vec::new();
    for attempt in 1..=2 {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "create {attempt}: expected HTTP 200, got {status} with body {body}"
        );
        ids.push(
            element(&body, ID_ELEMENT)
                .unwrap_or_else(|| panic!("create {attempt}: no <{ID_ELEMENT}> in {body}")),
        );
    }
    assert_ne!(
        ids[0], ids[1],
        "two creates must mint two ids, got {ids:?} twice"
    );

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "both tables: expected HTTP 200 from {DESCRIBE}, got {status} with body {body}"
    );
    let mut reported = elements(&body, ID_ELEMENT);
    reported.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(
        reported, expected,
        "both tables: expected {expected:?} to be reported, got {reported:?}"
    );
    assert_eq!(
        stored_tables(&engine),
        2,
        "both tables: expected two stored route tables, got {}",
        stored_tables(&engine)
    );
}

// ---------------------------------------------------------------------------
// A LocalGatewayId that resolves to no local gateway
// ---------------------------------------------------------------------------

/// A `LocalGatewayId` that resolves to no gateway of the account is refused and nothing is
/// written, and the CODE says HOW the value failed: a well-formed local gateway id the
/// account holds no gateway for is `InvalidLocalGatewayID.NotFound`, and a value that is no
/// local gateway id at all is `InvalidLocalGatewayID.Malformed`. The rows are each way a
/// value can fail to resolve: both published EC2 id suffix lengths with digits nothing was
/// created for, a suffix of neither length, an upper-case suffix, a non-hex suffix, the id of
/// a route table on a gateway, another resource's id, and free text.
#[tokio::test]
async fn a_local_gateway_id_that_resolves_to_no_gateway_is_refused() {
    let prefix = ec2::state::LOCAL_GATEWAY_ID_PREFIX;
    let cases = [
        (
            "the long form",
            format!("{prefix}-0123456789abcdef0"),
            INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "the short form",
            format!("{prefix}-00000000"),
            INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        ),
        (
            "neither length",
            format!("{prefix}-0000000000000000"),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "an upper-case suffix",
            format!("{prefix}-0ABCDEF01"),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "a non-hex suffix",
            format!("{prefix}-zzzzzzzz"),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "a route table's own id",
            "lgw-rtb-0123456789abcdef0".to_string(),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "another resource's id",
            "subnet-00000000000000000".to_string(),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "free text",
            "not-a-gateway-at-all".to_string(),
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
    ];

    for (case, local_gateway_id, expected_code) in cases {
        let (router, engine) = super::ec2_query::emulator();
        seed_gateways(&engine);

        let (status, body) = create(&router, &[param("LocalGatewayId", &local_gateway_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code:?}, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&format!("'{local_gateway_id}'")),
            "case {case:?}: expected the message to name {local_gateway_id:?}, got {message:?}"
        );
        assert_eq!(
            stored_tables(&engine),
            0,
            "case {case:?}: a refused create must write nothing, got {} route tables",
            stored_tables(&engine)
        );
    }
}

/// A gateway the account DOES hold is what separates the refusals above from a success: the
/// same request that was refused for an unknown id succeeds once a gateway with that id is in
/// the store, so the answer is the store's rather than the request's.
#[tokio::test]
async fn the_same_id_is_refused_before_the_gateway_is_stored_and_served_after() {
    let (router, engine) = super::ec2_query::emulator();
    let unknown = format!("{}-0123456789abcdef0", ec2::state::LOCAL_GATEWAY_ID_PREFIX);

    let (status, body) = create(&router, &[param("LocalGatewayId", &unknown)]).await;
    assert_eq!(
        status, 400,
        "before: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        "before: expected the gateway's own NotFound, got {body}"
    );

    let stored = seed_gateways(&engine);
    let (status, body) = create(&router, &[param("LocalGatewayId", &stored[0])]).await;
    assert_eq!(
        status, 200,
        "after: expected HTTP 200, got {status} with body {body}"
    );
    assert_member(
        "after",
        &body,
        "localGatewayId",
        &stored[0],
    );
}

/// A request that carries no `LocalGatewayId` names no gateway to create a table on, so it is
/// refused rather than answered with a table on nothing: a request that leaves the member out
/// is refused by the boundary validator before the handler runs, and one that sends it empty
/// is EC2's `MissingParameter`.
#[tokio::test]
async fn a_request_that_names_no_gateway_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed_gateways(&engine);

    for (case, params, expected_code) in [
        ("no LocalGatewayId at all", Vec::new(), None),
        (
            "an empty LocalGatewayId",
            vec![param("LocalGatewayId", "")],
            Some(MISSING_PARAMETER),
        ),
    ] {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        if let Some(code) = expected_code {
            assert_eq!(
                error_code(&body),
                code,
                "case {case:?}: expected {code:?}, got {body}"
            );
        }
        assert_eq!(
            stored_tables(&engine),
            0,
            "case {case:?}: a refused create must write nothing, got {} route tables",
            stored_tables(&engine)
        );
    }
}

/// A `TagSpecification` may only name the resource type this action creates: anything else is
/// refused and nothing is written. Rows: the resource type of the gateway the table hangs
/// off, the type of an association made in the table, and a type from another surface
/// entirely.
#[tokio::test]
async fn a_tag_specification_naming_another_resource_type_is_refused() {
    for resource_type in [
        "local-gateway",
        "local-gateway-route-table-vpc-association",
        "subnet",
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let gateways = seed_gateways(&engine);

        let mut params = vec![param("LocalGatewayId", &gateways[0])];
        params.extend(tag_specification(resource_type, &[("Name", "rack")]));
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {resource_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {resource_type:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            stored_tables(&engine),
            0,
            "case {resource_type:?}: a refused create must write nothing, got {} route tables",
            stored_tables(&engine)
        );
    }
}

/// A `Mode` that is not a member of the model's own `LocalGatewayRouteTableMode` catalog is
/// refused before the handler runs, so nothing is written.
#[tokio::test]
async fn a_mode_outside_the_modelled_catalog_is_refused() {
    for mode in ["COIP", "direct_vpc_routing", "anything"] {
        let (router, engine) = super::ec2_query::emulator();
        let gateways = seed_gateways(&engine);

        let (status, body) = create(
            &router,
            &[param("LocalGatewayId", &gateways[0]), param("Mode", mode)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {mode:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            stored_tables(&engine),
            0,
            "case {mode:?}: a refused create must write nothing, got {} route tables",
            stored_tables(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and writes
/// nothing, whether or not the gateway it names exists; `DryRun=false` is an ordinary
/// create.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_writing() {
    let (router, engine) = super::ec2_query::emulator();
    let gateways = seed_gateways(&engine);
    let unknown = format!("{}-0123456789abcdef0", ec2::state::LOCAL_GATEWAY_ID_PREFIX);

    // Rows: a gateway the store holds, and an id it does not — the flag asks whether the
    // caller is permitted, which is not a question about the gateway.
    for (case, local_gateway_id) in [
        ("a stored gateway", gateways[0].as_str()),
        ("an unknown gateway", unknown.as_str()),
    ] {
        let (status, body) = create(
            &router,
            &[
                param("LocalGatewayId", local_gateway_id),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            stored_tables(&engine),
            0,
            "case {case:?}: a dry run must write nothing, got {} route tables",
            stored_tables(&engine)
        );
    }

    // `DryRun=false` is no dry run at all: the table is created.
    let (status, body) = create(
        &router,
        &[
            param("LocalGatewayId", &gateways[0]),
            param("DryRun", "false"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_tables(&engine),
        1,
        "DryRun=false: expected the route table to be created, got {} route tables",
        stored_tables(&engine)
    );
}

// ---------------------------------------------------------------------------
// Which refusal a request that fails in several ways at once gets
// ---------------------------------------------------------------------------

/// A request can fail in more than one way at once, and EC2 answers exactly one of them: the
/// request's own FORM is read before any permission is checked, and permission before
/// anything the request names is RESOLVED against the account's resources. Each row below
/// carries the same three defects — a `LocalGatewayId` that cannot resolve, a
/// `TagSpecification` naming another resource type, and `DryRun` — and drops one defect at a
/// time, so the code that comes back is the first refusal in that order rather than whichever
/// check happens to run last.
#[tokio::test]
async fn a_request_that_fails_several_ways_gets_the_first_refusal_in_order() {
    let unknown = format!("{}-0123456789abcdef0", ec2::state::LOCAL_GATEWAY_ID_PREFIX);
    let malformed = format!("{}-zzzzzzzz", ec2::state::LOCAL_GATEWAY_ID_PREFIX);

    // (case, LocalGatewayId, a TagSpecification naming another resource type, DryRun,
    //  expected status, expected code)
    let cases: [(&str, &str, bool, bool, u16, &str); 5] = [
        (
            "no id, a foreign tag type and a dry run",
            "",
            true,
            true,
            400,
            MISSING_PARAMETER,
        ),
        (
            "a malformed id, a foreign tag type and a dry run",
            malformed.as_str(),
            true,
            true,
            400,
            INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        ),
        (
            "an unresolvable id, a foreign tag type and a dry run",
            unknown.as_str(),
            true,
            true,
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an unresolvable id and a dry run",
            unknown.as_str(),
            false,
            true,
            DRY_RUN_STATUS,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "an unresolvable id alone",
            unknown.as_str(),
            false,
            false,
            400,
            INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        ),
    ];

    for (case, local_gateway_id, foreign_tag_type, dry_run, expected_status, expected_code) in cases
    {
        let (router, engine) = super::ec2_query::emulator();
        seed_gateways(&engine);

        let mut params = vec![param("LocalGatewayId", local_gateway_id)];
        if foreign_tag_type {
            params.extend(tag_specification("local-gateway", &[("Name", "rack")]));
        }
        if dry_run {
            params.push(param("DryRun", "true"));
        }

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code:?}, got {body}"
        );
        assert_eq!(
            stored_tables(&engine),
            0,
            "case {case:?}: a refused create must write nothing, got {} route tables",
            stored_tables(&engine)
        );
    }
}
