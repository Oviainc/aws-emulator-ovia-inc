// SPDX-License-Identifier: CC0-1.0

//! `DescribeDomainControllers`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body — and
//! the controllers are only ever reached through the operation, never by reaching into
//! the state struct, so a response that is not computed from persisted state cannot pass.
//! The harness (router, call helpers) is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`.
//!
//! The cases assert the RULE over sets of inputs rather than one recorded call: the
//! reported controllers over a table of creates differing in VPC, subnet count and
//! subnet order; the status over the transient and the settled instant; the
//! `DomainControllerIds` filter over a table of filter forms; pagination over a table of
//! page sizes spanning 1, exactly the controller count and past it; and the directory
//! types the listing is served for over the two types this emulator can persist.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    create_directory, describe_one, minimal_create, ok, refused, service, PASSWORD, PAST_ONE_SPAN,
};
use crate::microsoft_ad::create_microsoft_ad;

/// The status a freshly placed domain controller carries: the first value of the modelled
/// `DomainControllerStatus` enum
/// (`smithy_query(query="shape",shape="DomainControllerStatus")`).
const STATUS_CREATING: &str = "Creating";

/// The status a placed controller reaches once the transition its placement owes falls
/// due: the next value of that same modelled enum, the one a controller in service
/// carries.
const STATUS_ACTIVE: &str = "Active";

/// A `DirectoryId` of the shape the modelled pattern `^d-[0-9a-f]{10}$` admits that no
/// create in this suite mints, so it names no stored directory.
const ABSENT_DIRECTORY_ID: &str = "d-0123456789";

/// A `DomainControllerId` of the shape the modelled pattern `^dc-[0-9a-f]{10}$` admits
/// that no placement in this suite mints, so it names no stored controller.
const ABSENT_CONTROLLER_ID: &str = "dc-0123456789";

/// A `CreateMicrosoftAD` body naming `vpc` and `subnets`, so a row of the table below
/// differs from the next in nothing but the VPC it places its controllers in.
fn microsoft_ad_in(name: &str, vpc: &str, subnets: &[&str]) -> Value {
    json!({
        "Name": name,
        "Password": PASSWORD,
        "VpcSettings": { "VpcId": vpc, "SubnetIds": subnets }
    })
}

/// The `DomainControllers` of a `DescribeDomainControllers` answer.
fn controllers(body: &Value, case: &str) -> Vec<Value> {
    body["DomainControllers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected DomainControllers, got {body}"))
        .clone()
}

/// `DescribeDomainControllers` for `directory_id` with `extra` merged into the request,
/// asserting it was served and returning the whole answer.
async fn describe_controllers(
    router: &MultiServiceRouter,
    directory_id: &str,
    extra: Value,
    case: &str,
) -> Value {
    let mut body = json!({ "DirectoryId": directory_id });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    ok(router, "DescribeDomainControllers", body, case).await
}

/// The `DomainControllerId`s a listing reported, in the order it reported them.
fn controller_ids(listed: &[Value], case: &str) -> Vec<String> {
    listed
        .iter()
        .map(|controller| {
            controller["DomainControllerId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a DomainControllerId, got {controller}")
                })
                .to_string()
        })
        .collect()
}

/// The modelled `DomainControllerId` pattern, `^dc-[0-9a-f]{10}$`, as an assertion.
fn assert_controller_id(id: &str, case: &str) {
    let suffix = id.strip_prefix("dc-").unwrap_or_default();
    assert!(
        suffix.len() == 10
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "case {case:?}: expected a DomainControllerId matching ^dc-[0-9a-f]{{10}}$, got {id:?}"
    );
}

/// One row of the create table: the VPC settings a Managed Microsoft AD is created with,
/// and the subnets its controllers must therefore reside in.
struct PlacementCase {
    name: &'static str,
    directory_name: &'static str,
    vpc: &'static str,
    subnets: &'static [&'static str],
    /// One subnet per controller, in placement order: a controller is placed in each of
    /// the subnets the create named, taken in that order.
    expect_subnets: &'static [&'static str],
}

#[tokio::test]
async fn the_controllers_a_create_placed_are_reported_from_persisted_state() {
    let router = service();

    // The rows differ along what the placement is derived from: the VPC, how many subnets
    // the create named and in which order — including a three-subnet VPC, which the
    // issue's recorded call did not use.
    let cases = [
        PlacementCase {
            name: "two subnets",
            directory_name: "corp.example.com",
            vpc: "vpc-0a1b2c3d",
            subnets: &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
            expect_subnets: &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
        },
        PlacementCase {
            name: "two subnets, reversed",
            directory_name: "sales.example.org",
            vpc: "vpc-11223344",
            subnets: &["subnet-1a2b3c4d", "subnet-0a1b2c3d"],
            expect_subnets: &["subnet-1a2b3c4d", "subnet-0a1b2c3d"],
        },
        PlacementCase {
            name: "three subnets",
            directory_name: "eng.example.com",
            vpc: "vpc-abcdef01",
            subnets: &["subnet-aaaaaaaa", "subnet-bbbbbbbb", "subnet-cccccccc"],
            // A fresh directory has two controllers, so the first two named subnets are
            // the ones they reside in.
            expect_subnets: &["subnet-aaaaaaaa", "subnet-bbbbbbbb"],
        },
    ];

    let mut every_id: BTreeSet<String> = BTreeSet::new();
    for case in &cases {
        let directory_id = create_microsoft_ad(
            &router,
            microsoft_ad_in(case.directory_name, case.vpc, case.subnets),
            case.name,
        )
        .await;

        let answer = describe_controllers(&router, &directory_id, json!({}), case.name).await;
        let listed = controllers(&answer, case.name);

        // As many controllers as the directory reports it desires: the listing and
        // `DescribeDirectories$DesiredNumberOfDomainControllers` are two views of one
        // stored record, so they cannot disagree.
        let described = describe_one(&router, &directory_id, case.name).await;
        assert_eq!(
            json!(listed.len()),
            described["DesiredNumberOfDomainControllers"],
            "case {:?}: expected as many controllers as the directory desires, got {answer}",
            case.name
        );
        assert_eq!(
            listed.len(),
            case.expect_subnets.len(),
            "case {:?}: expected {} controllers, got {answer}",
            case.name,
            case.expect_subnets.len()
        );

        for (index, controller) in listed.iter().enumerate() {
            let case_name = case.name;
            let id = controller["DomainControllerId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case_name:?}: expected a DomainControllerId, got {controller}")
                });
            assert_controller_id(id, case.name);
            assert!(
                every_id.insert(id.to_string()),
                "case {case_name:?}: expected a fresh DomainControllerId, got the already \
                 reported {id:?}"
            );
            assert_eq!(
                controller["DirectoryId"],
                json!(directory_id),
                "case {case_name:?}: expected the controller to name its directory, got \
                 {controller}"
            );
            assert_eq!(
                controller["VpcId"],
                json!(case.vpc),
                "case {case_name:?}: expected the controller to name the requested VPC, got \
                 {controller}"
            );
            assert_eq!(
                controller["SubnetId"],
                json!(case.expect_subnets[index]),
                "case {case_name:?}: expected controller {index} in subnet {:?}, got {controller}",
                case.expect_subnets[index]
            );
            assert_eq!(
                controller["Status"],
                json!(STATUS_CREATING),
                "case {case_name:?}: expected the transient status straight after the create, \
                 got {controller}"
            );
            assert!(
                controller["LaunchTime"].is_number(),
                "case {case_name:?}: expected a LaunchTime on a placed controller, got \
                 {controller}"
            );
            assert!(
                controller["StatusLastUpdatedDateTime"].is_number(),
                "case {case_name:?}: expected a StatusLastUpdatedDateTime on a placed \
                 controller, got {controller}"
            );
        }

        // The final page carries no token: everything stored for this directory fitted.
        assert_eq!(
            answer["NextToken"],
            json!(null),
            "case {:?}: expected no NextToken on a complete page, got {answer}",
            case.name
        );
    }

    // Each directory reports its OWN controllers: the ids are disjoint across the table.
    assert_eq!(
        every_id.len(),
        cases
            .iter()
            .map(|case| case.expect_subnets.len())
            .sum::<usize>(),
        "expected every directory's controllers to be reported once, got {every_id:?}"
    );
}

#[tokio::test]
async fn a_controller_status_is_the_one_the_clock_owes() {
    let case = "status transition";
    let router = service();

    let directory_id = create_microsoft_ad(
        &router,
        microsoft_ad_in(
            "status.example.com",
            "vpc-0a1b2c3d",
            &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
        ),
        case,
    )
    .await;

    // A caller acting straight after the create still sees the transient status the
    // modelled `DomainControllerStatus` enum opens with.
    let fresh = describe_controllers(&router, &directory_id, json!({}), case).await;
    for controller in controllers(&fresh, case) {
        assert_eq!(
            controller["Status"],
            json!(STATUS_CREATING),
            "case {case:?}: expected the transient status, got {controller}"
        );
    }

    tokio::time::sleep(PAST_ONE_SPAN).await;

    let settled = controllers(
        &describe_controllers(&router, &directory_id, json!({}), case).await,
        case,
    );
    for controller in &settled {
        assert_eq!(
            controller["Status"],
            json!(STATUS_ACTIVE),
            "case {case:?}: expected the status the clock owed, got {controller}"
        );
    }

    // Reading twice does not move a status on again, nor restamp when it last changed:
    // the listing reports what the clock already owed and nothing more.
    let again = controllers(
        &describe_controllers(&router, &directory_id, json!({}), case).await,
        case,
    );
    assert_eq!(
        again, settled,
        "case {case:?}: expected a second read to report the same controllers, got {again:?}"
    );
}

#[tokio::test]
async fn the_domain_controller_ids_filter_narrows_the_listing() {
    let case = "filter";
    let router = service();

    let directory_id = create_microsoft_ad(
        &router,
        microsoft_ad_in(
            "filter.example.com",
            "vpc-0a1b2c3d",
            &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
        ),
        case,
    )
    .await;
    let all = controller_ids(
        &controllers(
            &describe_controllers(&router, &directory_id, json!({}), case).await,
            case,
        ),
        case,
    );
    assert_eq!(
        all.len(),
        2,
        "case {case:?}: expected the two controllers of a fresh directory, got {all:?}"
    );

    // The rows differ in the filter's form: absent, empty (no particular controller
    // asked for, so an unfiltered listing), one id, the other id, and both.
    let cases: [(&str, Value, Vec<String>); 5] = [
        ("absent", json!(null), all.clone()),
        ("empty", json!([]), all.clone()),
        ("first only", json!([all[0]]), vec![all[0].clone()]),
        ("second only", json!([all[1]]), vec![all[1].clone()]),
        ("both", json!([all[1], all[0]]), all.clone()),
    ];

    for (name, filter, expected) in cases {
        let extra = if filter.is_null() {
            json!({})
        } else {
            json!({ "DomainControllerIds": filter })
        };
        let answer = describe_controllers(&router, &directory_id, extra, name).await;
        let reported = controller_ids(&controllers(&answer, name), name);
        assert_eq!(
            reported, expected,
            "case {name:?}: expected {expected:?} in placement order, got {reported:?}"
        );
    }

    // An id naming no controller of this directory is reported rather than dropped from
    // the page.
    refused(
        &router,
        "DescribeDomainControllers",
        json!({
            "DirectoryId": &directory_id,
            "DomainControllerIds": [&all[0], ABSENT_CONTROLLER_ID]
        }),
        "EntityDoesNotExistException",
        "unknown controller id",
    )
    .await;

    // Nor is another directory's controller one of this directory's.
    let other = create_microsoft_ad(
        &router,
        microsoft_ad_in(
            "other.example.com",
            "vpc-11223344",
            &["subnet-11223344", "subnet-55667788"],
        ),
        case,
    )
    .await;
    let other_ids = controller_ids(
        &controllers(
            &describe_controllers(&router, &other, json!({}), case).await,
            case,
        ),
        case,
    );
    refused(
        &router,
        "DescribeDomainControllers",
        json!({ "DirectoryId": &directory_id, "DomainControllerIds": [&other_ids[0]] }),
        "EntityDoesNotExistException",
        "another directory's controller",
    )
    .await;
}

#[tokio::test]
async fn the_listing_pages_through_its_own_next_token() {
    let case = "pagination";
    let router = service();

    let directory_id = create_microsoft_ad(
        &router,
        microsoft_ad_in(
            "paged.example.com",
            "vpc-0a1b2c3d",
            &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
        ),
        case,
    )
    .await;
    let all = controller_ids(
        &controllers(
            &describe_controllers(&router, &directory_id, json!({}), case).await,
            case,
        ),
        case,
    );

    // The page sizes span 1, exactly the stored count and one past it; `Limit`'s own
    // documentation makes 0 mean "the limitations of the operation", so it pages
    // everything.
    for limit in [1_usize, 2, 3, 0] {
        let name = format!("limit {limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut extra = json!({ "Limit": limit });
            if let Some(next) = token.as_ref() {
                extra["NextToken"] = json!(next);
            }
            let answer = describe_controllers(&router, &directory_id, extra, &name).await;
            let page = controllers(&answer, &name);
            if limit > 0 {
                assert!(
                    page.len() <= limit,
                    "case {name:?}: expected at most {limit} per page, got {answer}"
                );
            }
            seen.extend(controller_ids(&page, &name));
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {name:?}: expected the paging to terminate, got {pages} pages"
            );
            match answer["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "case {name:?}: expected every controller once, in placement order, got {seen:?}"
        );
    }

    // A token this listing did not issue is refused rather than served as a page of it:
    // one that is not this service's at all, and one another listing of this service
    // issued.
    for (name, token) in [
        ("not a token", json!("not-a-token")),
        (
            "another listing's token",
            ok(
                &router,
                "DescribeDirectories",
                json!({ "Limit": 1 }),
                "another listing's token",
            )
            .await["NextToken"]
                .clone(),
        ),
    ] {
        if token.is_null() {
            continue;
        }
        refused(
            &router,
            "DescribeDomainControllers",
            json!({ "DirectoryId": &directory_id, "NextToken": token }),
            "InvalidNextTokenException",
            name,
        )
        .await;
    }
}

#[tokio::test]
async fn only_a_directory_whose_controllers_this_service_runs_is_listed() {
    let router = service();

    // A Simple AD directory runs no domain controller of the caller's domain — the
    // modelled `DesiredNumberOfDomainControllers` is reported "if the directory is
    // Microsoft AD" — so the listing is refused for it rather than served as an empty
    // page.
    let simple = create_directory(&router, minimal_create("simple.example.com"), "SimpleAD").await;
    refused(
        &router,
        "DescribeDomainControllers",
        json!({ "DirectoryId": &simple }),
        "UnsupportedOperationException",
        "SimpleAD",
    )
    .await;

    // An id naming no stored directory at all is a different failure, and says so.
    refused(
        &router,
        "DescribeDomainControllers",
        json!({ "DirectoryId": ABSENT_DIRECTORY_ID }),
        "EntityDoesNotExistException",
        "unknown directory",
    )
    .await;

    // And a Managed Microsoft AD one IS served, so the refusal above is about the type
    // and not about the listing.
    let managed = create_microsoft_ad(
        &router,
        microsoft_ad_in(
            "managed.example.com",
            "vpc-0a1b2c3d",
            &["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
        ),
        "MicrosoftAD",
    )
    .await;
    let listed = controllers(
        &describe_controllers(&router, &managed, json!({}), "MicrosoftAD").await,
        "MicrosoftAD",
    );
    assert_eq!(
        listed.len(),
        2,
        "case {:?}: expected the controllers of a Managed Microsoft AD directory, got {listed:?}",
        "MicrosoftAD"
    );
}
