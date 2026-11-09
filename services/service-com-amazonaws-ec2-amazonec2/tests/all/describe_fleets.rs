// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeFleets`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the report is the EC2 Fleets the store holds, each as `CreateFleet` stored it: every
//!    member the create set round-trips, and an `instant` fleet also reports the instances it
//!    launched, read back out of the INSTANCE store;
//! 2. a report that names no ids leaves out the fleets of type `instant`: "if a fleet is of
//!    type instant, you must specify the fleet ID in the request, otherwise the fleet does
//!    not appear in the response" (`DescribeFleets`);
//! 3. `FleetIds` narrows the report, a `FleetId` that is not a fleet id at all is refused
//!    with `InvalidFleetId.Malformed` and a well-formed one this account holds no fleet for
//!    with `InvalidFleetId.NotFound`, in that order;
//! 4. the documented filters narrow it, over EC2's AND/OR rule, and a filter name the
//!    operation does not document is refused;
//! 5. it is cut into `MaxResults`-sized pages a `NextToken` continues, in `FleetId` order;
//! 6. `DryRun` answers `DryRunOperation`, and describing is a READ: two calls answer the
//!    same.
//!
//! Each table row is a distinct input along what the rule generalises over — which filter,
//! which value, which malformed id form, which page size — so no row is the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FLEET_ACTIVITY_STATUS_FULFILLED, FLEET_STATE_ACTIVE,
    FLEET_STATE_SUBMITTED, FLEET_TYPE_INSTANT, FLEET_TYPE_MAINTAIN, FLEET_TYPE_REQUEST,
    MAX_RESOURCE_IDS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, indexed};
use super::wire::document;

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// A well-formed EC2 Fleet id no fleet of the account is stored under: the mint counts from
/// one, so a high counter value names nothing.
const UNKNOWN_FLEET_ID: &str = "fleet-0000ffff-0000-0000-0000-00000000ffff";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// An AMI of the published catalog a fresh account can already see, read out of the
/// service's own state so the test cannot invent an id.
fn image_id() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeFleets", params).await
}

/// Create one launch template and answer its `LaunchTemplateId`.
async fn launch_template(router: &MultiServiceRouter, case: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "describe-fleets-suite"),
            p("LaunchTemplateData.ImageId", image_id()),
            p("LaunchTemplateData.InstanceType", "t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate must succeed, got {status}: {body}"
    );
    element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: no launchTemplateId in {body}"))
}

/// Create one fleet of `fleet_type` with `extra` parameters and answer its `FleetId`.
async fn create_fleet(
    router: &MultiServiceRouter,
    case: &str,
    template_id: &str,
    fleet_type: &str,
    extra: &[(String, String)],
) -> String {
    let mut params = vec![
        p(
            "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateId",
            template_id,
        ),
        p("Type", fleet_type),
        p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
    ];
    params.extend(extra.iter().cloned());
    let (status, body) = call(router, REGION, "CreateFleet", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateFleet must succeed for a {fleet_type} fleet, got {status}: {body}"
    );
    element(&body, "fleetId").unwrap_or_else(|| panic!("case {case:?}: no fleetId in {body}"))
}

/// The `fleetId`s a `DescribeFleets` response reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "fleetId")
}

/// The instance ids a reported fleet's per-instance set carries: the `<item>` texts of every
/// `instanceIds` set in the body, as ec2Query renders a list.
fn launched_instance_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "instanceIds")
        .flat_map(|set| {
            set.children()
                .filter(|node| node.is_element() && node.tag_name().name() == "item")
                .map(|node| node.text().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        })
        .collect()
}

#[tokio::test]
async fn the_report_is_the_fleets_the_store_holds() {
    let case = "three fleets, one of each type";
    let (router, _engine) = emulator();
    let template_id = launch_template(&router, case).await;
    let maintain = create_fleet(&router, case, &template_id, FLEET_TYPE_MAINTAIN, &[]).await;
    let requested = create_fleet(&router, case, &template_id, FLEET_TYPE_REQUEST, &[]).await;
    let instant = create_fleet(&router, case, &template_id, FLEET_TYPE_INSTANT, &[]).await;

    // "If a fleet is of type instant, you must specify the fleet ID in the request,
    // otherwise the fleet does not appear in the response."
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status}: {body}"
    );
    let mut unnamed = reported_ids(&body);
    unnamed.sort();
    let mut expected = vec![maintain.clone(), requested.clone()];
    expected.sort();
    assert_eq!(
        unnamed, expected,
        "case {case:?}: a report naming no ids leaves out the instant fleet, expected \
         {expected:?}, got {unnamed:?}"
    );

    // Describing is a READ: the same call answers the same fleets.
    let (_, again) = describe(&router, &[]).await;
    let mut repeated = reported_ids(&again);
    repeated.sort();
    assert_eq!(
        repeated, unnamed,
        "case {case:?}: describing twice reports the same fleets, got {repeated:?}"
    );

    // Every member the create stored round-trips, and the instant fleet reports the
    // instances it launched.
    let case = "the instant fleet by id";
    let (status, body) = describe(&router, &indexed("FleetId", &[instant.as_str()])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status}: {body}"
    );
    assert_eq!(
        reported_ids(&body),
        vec![instant.clone()],
        "case {case:?}: FleetIds narrows the report to the fleet it names, got {body}"
    );
    assert_eq!(
        element(&body, "type").as_deref(),
        Some(FLEET_TYPE_INSTANT),
        "case {case:?}: the fleet type round-trips, got {body}"
    );
    assert_eq!(
        element(&body, "fleetState").as_deref(),
        Some(FLEET_STATE_ACTIVE),
        "case {case:?}: an instant fleet is {FLEET_STATE_ACTIVE} as soon as it is created, \
         got {body}"
    );
    assert_eq!(
        element(&body, "activityStatus").as_deref(),
        Some(FLEET_ACTIVITY_STATUS_FULFILLED),
        "case {case:?}: a fleet that placed its whole target is \
         {FLEET_ACTIVITY_STATUS_FULFILLED}, got {body}"
    );
    assert_eq!(
        element(&body, "totalTargetCapacity").as_deref(),
        Some("1"),
        "case {case:?}: the target capacity round-trips, got {body}"
    );
    assert_eq!(
        element(&body, "launchTemplateId").as_deref(),
        Some(template_id.as_str()),
        "case {case:?}: the launch template the fleet launches from round-trips, got {body}"
    );
    assert_eq!(
        element(&body, "fulfilledCapacity").as_deref(),
        Some("1.0"),
        "case {case:?}: the capacity the fleet placed is reported fulfilled, got {body}"
    );
    // The instances are the ones the instance store holds for the fleet.
    let launched = launched_instance_ids(case, &body);
    assert_eq!(
        launched.len(),
        1,
        "case {case:?}: an instant fleet reports the instance it launched, got {body}"
    );
    let (status, instances) = call(
        &router,
        REGION,
        "DescribeInstances",
        &[p("InstanceId.1", launched[0].clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances must report the fleet's instance, got {status}: \
         {instances}"
    );

    // A fleet of an asynchronous type reports neither an instance set nor an error set:
    // "supported only for fleets of type instant".
    let case = "the maintain fleet by id";
    let (status, body) = describe(&router, &indexed("FleetId", &[maintain.as_str()])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status}: {body}"
    );
    assert!(
        !body.contains("<fleetInstanceSet>"),
        "case {case:?}: an asynchronous fleet reports no per-instance set, got {body}"
    );
    assert_eq!(
        element(&body, "fleetState").as_deref(),
        Some(FLEET_STATE_SUBMITTED),
        "case {case:?}: a fleet whose request EC2 has just taken on is \
         {FLEET_STATE_SUBMITTED}, got {body}"
    );
}

/// One filter row: the filter entries it sends, and the fleets it must select out of the
/// three the suite creates.
struct FilterCase {
    case: &'static str,
    /// `(name, values)` filter entries.
    entries: fn(&str) -> Vec<(String, String)>,
    /// Which of the created fleets the filter selects, by position: maintain, request.
    selects: &'static [usize],
}

#[tokio::test]
async fn the_documented_filters_narrow_the_report() {
    let (router, _engine) = emulator();
    let template_id = launch_template(&router, "filters").await;
    let maintain = create_fleet(
        &router,
        "filters",
        &template_id,
        FLEET_TYPE_MAINTAIN,
        &[
            p("ExcessCapacityTerminationPolicy", "termination"),
            p("ReplaceUnhealthyInstances", "true"),
        ],
    )
    .await;
    let requested = create_fleet(&router, "filters", &template_id, FLEET_TYPE_REQUEST, &[]).await;
    let fleets = [maintain, requested];

    let rows = vec![
        FilterCase {
            case: "the fleet type",
            entries: |_| super::ec2_query::filters(&[("type", &[FLEET_TYPE_MAINTAIN])]),
            selects: &[0],
        },
        FilterCase {
            case: "the fleet state both fleets are in",
            entries: |_| super::ec2_query::filters(&[("fleet-state", &[FLEET_STATE_SUBMITTED])]),
            selects: &[0, 1],
        },
        FilterCase {
            case: "the activity status",
            entries: |_| {
                super::ec2_query::filters(&[(
                    "activity-status",
                    &[FLEET_ACTIVITY_STATUS_FULFILLED],
                )])
            },
            selects: &[0, 1],
        },
        FilterCase {
            case: "unhealthy-instance replacement",
            entries: |_| {
                super::ec2_query::filters(&[("replace-unhealthy-instances", &["true"])])
            },
            selects: &[0],
        },
        FilterCase {
            case: "the excess capacity policy",
            entries: |_| {
                super::ec2_query::filters(&[(
                    "excess-capacity-termination-policy",
                    &["termination"],
                )])
            },
            selects: &[0],
        },
        FilterCase {
            case: "two filters that must both hold",
            entries: |_| {
                super::ec2_query::filters(&[
                    ("type", &[FLEET_TYPE_MAINTAIN]),
                    ("replace-unhealthy-instances", &["false"]),
                ])
            },
            selects: &[],
        },
        FilterCase {
            case: "either of two values of one filter",
            entries: |_| {
                super::ec2_query::filters(&[(
                    "type",
                    &[FLEET_TYPE_MAINTAIN, FLEET_TYPE_REQUEST],
                )])
            },
            selects: &[0, 1],
        },
        FilterCase {
            case: "a value no fleet carries",
            entries: |_| super::ec2_query::filters(&[("fleet-state", &[FLEET_STATE_ACTIVE])]),
            selects: &[],
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, body) = describe(&router, &(row.entries)(&template_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status}: {body}"
        );
        let mut reported = reported_ids(&body);
        reported.sort();
        let mut expected: Vec<String> = row
            .selects
            .iter()
            .map(|index| fleets[*index].clone())
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the fleets {expected:?}, got {reported:?}"
        );
    }

    let case = "a filter name the operation does not document";
    let (status, body) = describe(
        &router,
        &super::ec2_query::filters(&[("fleet-colour", &["green"])]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got {body}"
    );
}

#[tokio::test]
async fn the_report_is_paged_and_refuses_what_it_cannot_serve() {
    let case = "two fleets and a page of one";
    let (router, _engine) = emulator();
    let template_id = launch_template(&router, case).await;
    let first = create_fleet(&router, case, &template_id, FLEET_TYPE_MAINTAIN, &[]).await;
    let second = create_fleet(&router, case, &template_id, FLEET_TYPE_REQUEST, &[]).await;
    let mut in_id_order = [first, second];
    in_id_order.sort();

    let (status, body) = describe(&router, &[p("MaxResults", "1")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status}: {body}"
    );
    assert_eq!(
        reported_ids(&body),
        vec![in_id_order[0].clone()],
        "case {case:?}: a page of one carries the first fleet in id order, got {body}"
    );
    let token = element(&body, "nextToken")
        .unwrap_or_else(|| panic!("case {case:?}: a fleet follows the page, so a token is \
                                  reported: {body}"));

    let (status, body) = describe(
        &router,
        &[p("MaxResults", "1"), p("NextToken", token.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status}: {body}"
    );
    assert_eq!(
        reported_ids(&body),
        vec![in_id_order[1].clone()],
        "case {case:?}: the token continues from the end of the previous page, got {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case {case:?}: no token when no fleet follows, got {body}"
    );

    // The refusals, each with EC2's own code for it.
    let case = "a fleet id that is not a fleet id at all";
    let (status, body) = describe(&router, &indexed("FleetId", &["fleet-nope"])).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidFleetId.Malformed",
        "case {case:?}: expected InvalidFleetId.Malformed, got {body}"
    );

    let case = "a well-formed fleet id this account holds no fleet for";
    let (status, body) = describe(&router, &indexed("FleetId", &[UNKNOWN_FLEET_ID])).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidFleetId.NotFound",
        "case {case:?}: expected InvalidFleetId.NotFound, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("The FleetId(s) {UNKNOWN_FLEET_ID} do not exist."),
        "case {case:?}: expected EC2's own sentence for a fleet id nothing is stored \
         under, got {body}"
    );

    let case = "more fleet ids than one request may name";
    let too_many: Vec<(String, String)> = (0..=MAX_RESOURCE_IDS)
        .map(|index| p(&format!("FleetId.{}", index + 1), UNKNOWN_FLEET_ID))
        .collect();
    let (status, body) = describe(&router, &too_many).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got {body}"
    );

    let case = "a page size of none";
    let (status, body) = describe(&router, &[p("MaxResults", "0")]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case {case:?}: expected InvalidParameterValue, got {body}"
    );

    let case = "a pagination token this service did not issue";
    let (status, body) = describe(&router, &[p("NextToken", "not-a-token")]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "case {case:?}: expected InvalidPaginationToken, got {body}"
    );

    let case = "a dry run of a request that would have succeeded";
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );

    let case = "a dry run naming a fleet that does not exist";
    let mut params = indexed("FleetId", &[UNKNOWN_FLEET_ID]);
    params.push(p("DryRun", "true"));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidFleetId.NotFound",
        "case {case:?}: a dry run that would have been refused is refused with that \
         reason, got {body}"
    );
}
