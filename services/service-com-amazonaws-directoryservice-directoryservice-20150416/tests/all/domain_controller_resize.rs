// SPDX-License-Identifier: CC0-1.0

//! `UpdateNumberOfDomainControllers`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body — and
//! the resized set is only ever read back through `DescribeDomainControllers` and
//! `DescribeDirectories`, never by reaching into the state struct, so an answer that is
//! not computed from persisted state cannot pass. The harness (router, call helpers) is
//! the sibling suite's, reached as `crate::directory_lifecycle::<item>`.
//!
//! The cases assert the RULE over sets of inputs rather than one recorded call: the
//! accepted numbers over a table spanning the modelled lower bound, numbers between it
//! and the published per-directory quota, and the quota itself; the refusal over a table
//! of numbers past that quota; the retirement order over a resize down; and the
//! directories a resize is served for over the two types this emulator can persist.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, describe_one, minimal_create, ok, refused, service, PASSWORD,
    PAST_ONE_SPAN,
};
use crate::microsoft_ad::create_microsoft_ad;

/// Directory Service's default quota of domain controllers per directory, from the
/// modelled `DomainControllerLimitExceededException`'s own documentation
/// (`smithy_query(query="shape",shape="DomainControllerLimitExceededException")`): "The
/// maximum allowed number of domain controllers per directory was exceeded. The default
/// limit per directory is 20 domain controllers."
const DOMAIN_CONTROLLER_QUOTA: i64 = 20;

/// The smallest `DesiredNumber` the modelled `DesiredNumberOfDomainControllers` admits:
/// `range { min: 2 }`, which is also what a fresh Managed Microsoft AD runs.
const MODELLED_MINIMUM: i64 = 2;

/// The status a freshly placed controller carries, and the one a controller in service
/// carries: the first two values of the modelled `DomainControllerStatus` enum
/// (`smithy_query(query="shape",shape="DomainControllerStatus")`).
const STATUS_CREATING: &str = "Creating";
const STATUS_ACTIVE: &str = "Active";

/// The status a controller being taken out of the directory carries: the `Deleting` value
/// of that same modelled enum.
const STATUS_DELETING: &str = "Deleting";

/// A `DirectoryId` of the shape the modelled pattern `^d-[0-9a-f]{10}$` admits that no
/// create in this suite mints, so it names no stored directory.
const ABSENT_DIRECTORY_ID: &str = "d-0123456789";

/// The subnets every directory in this suite is created across, so a controller added by
/// a resize can be checked to reside in one of them.
const SUBNETS: [&str; 2] = ["subnet-0a1b2c3d", "subnet-1a2b3c4d"];

/// A `CreateMicrosoftAD` body for `name` across [`SUBNETS`].
fn microsoft_ad(name: &str) -> Value {
    json!({
        "Name": name,
        "Password": PASSWORD,
        "VpcSettings": { "VpcId": "vpc-0a1b2c3d", "SubnetIds": SUBNETS }
    })
}

/// A Managed Microsoft AD directory whose create's stage transition has already fallen
/// due, so it is in service and a resize is not refused for being unreachable.
async fn directory_in_service(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let directory_id = create_microsoft_ad(router, microsoft_ad(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    directory_id
}

/// The `DomainControllers` `DescribeDomainControllers` reports for `directory_id`, in the
/// order it reports them.
async fn listed(router: &MultiServiceRouter, directory_id: &str, case: &str) -> Vec<Value> {
    let answer = ok(
        router,
        "DescribeDomainControllers",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["DomainControllers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected DomainControllers, got {answer}"))
        .clone()
}

/// The `DomainControllerId`s of a listing, in the order it reported them.
fn ids_of(controllers: &[Value], case: &str) -> Vec<String> {
    controllers
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

/// `UpdateNumberOfDomainControllers`, asserting it answered 200 with the modelled empty
/// result.
async fn resize(router: &MultiServiceRouter, directory_id: &str, desired: i64, case: &str) {
    let answer = ok(
        router,
        "UpdateNumberOfDomainControllers",
        json!({ "DirectoryId": directory_id, "DesiredNumber": desired }),
        case,
    )
    .await;
    assert_eq!(
        answer,
        json!({}),
        "case {case:?}: expected the modelled empty result, got {answer}"
    );
}

#[tokio::test]
async fn an_accepted_resize_runs_exactly_the_number_of_controllers_asked_for() {
    let router = service();

    // The rows differ along the number asked for: the modelled minimum (which a fresh
    // directory already runs, so the difference is nothing), one more than it, a step of
    // three, and the published per-directory quota itself — a number the issue's
    // recorded call never asked for.
    let cases: [(&str, &str, i64); 4] = [
        ("the modelled minimum", "min.example.com", MODELLED_MINIMUM),
        ("one more", "one.example.com", MODELLED_MINIMUM + 1),
        ("three more", "three.example.com", MODELLED_MINIMUM + 3),
        ("the quota", "quota.example.com", DOMAIN_CONTROLLER_QUOTA),
    ];

    for (case, name, desired) in cases {
        let directory_id = directory_in_service(&router, name, case).await;
        let before = ids_of(&listed(&router, &directory_id, case).await, case);

        resize(&router, &directory_id, desired, case).await;

        // The controllers the directory was already running are the ones it keeps: a
        // resize acts on the difference, not on the whole set.
        let after = listed(&router, &directory_id, case).await;
        let after_ids = ids_of(&after, case);
        assert_eq!(
            after_ids.len() as i64,
            desired,
            "case {case:?}: expected {desired} controllers, got {after_ids:?}"
        );
        assert_eq!(
            after_ids[..before.len()],
            before[..],
            "case {case:?}: expected the running controllers to be kept, got {after_ids:?}"
        );

        // A controller the resize added is not in service the instant the call answers,
        // and resides in a subnet of the directory, like one a create placed.
        for controller in &after[before.len()..] {
            assert_eq!(
                controller["Status"],
                json!(STATUS_CREATING),
                "case {case:?}: expected the transient status on an added controller, got \
                 {controller}"
            );
            let subnet = controller["SubnetId"].as_str().unwrap_or_default();
            assert!(
                SUBNETS.contains(&subnet),
                "case {case:?}: expected an added controller in one of {SUBNETS:?}, got \
                 {controller}"
            );
            assert_eq!(
                controller["DirectoryId"],
                json!(directory_id),
                "case {case:?}: expected the controller to name its directory, got {controller}"
            );
        }

        // The listing and `DescribeDirectories$DesiredNumberOfDomainControllers` are two
        // views of one stored record, so they cannot disagree.
        let described = describe_one(&router, &directory_id, case).await;
        assert_eq!(
            described["DesiredNumberOfDomainControllers"],
            json!(desired),
            "case {case:?}: expected the directory to desire {desired}, got {described}"
        );

        // Once the transition the resize owes falls due, every controller is in service.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let settled = listed(&router, &directory_id, case).await;
        assert_eq!(
            ids_of(&settled, case),
            after_ids,
            "case {case:?}: expected the same controllers once the clock caught up, got \
             {settled:?}"
        );
        for controller in &settled {
            assert_eq!(
                controller["Status"],
                json!(STATUS_ACTIVE),
                "case {case:?}: expected the status the clock owed, got {controller}"
            );
        }
    }
}

#[tokio::test]
async fn a_desired_number_past_the_per_directory_quota_is_refused_and_changes_nothing() {
    let router = service();
    let case = "over the quota";
    let directory_id = directory_in_service(&router, "over.example.com", case).await;
    let before = ids_of(&listed(&router, &directory_id, case).await, case);

    // The rows differ in how far past the published quota they reach: one past it, a
    // round hundred, the number the issue's recorded call sent, and the largest value the
    // modelled integer carries.
    for desired in [
        DOMAIN_CONTROLLER_QUOTA + 1,
        100,
        999,
        i64::from(i32::MAX),
    ] {
        let name = format!("DesiredNumber {desired}");
        let (status, parsed) = call(
            &router,
            "UpdateNumberOfDomainControllers",
            json!({ "DirectoryId": &directory_id, "DesiredNumber": desired }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status} with {parsed}"
        );
        assert!(
            parsed["__type"]
                .as_str()
                .unwrap_or_default()
                .contains("DomainControllerLimitExceededException"),
            "case {name:?}: expected DomainControllerLimitExceededException, got {parsed}"
        );

        // The message states the quota that was broken, in Directory Service's own
        // wording, and carries the modelled `RequestId`. The number the request asked for
        // is NOT part of it: the refusal states the ceiling, not the request.
        let message = parsed["Message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(&format!(
                "Domain controllers limit exceeded. A maximum of {DOMAIN_CONTROLLER_QUOTA} \
                 domain controllers may be requested."
            )),
            "case {name:?}: expected the limit-exceeded wording naming {DOMAIN_CONTROLLER_QUOTA}, \
             got {parsed}"
        );
        assert!(
            parsed["RequestId"].is_string(),
            "case {name:?}: expected the modelled RequestId, got {parsed}"
        );

        // A refused resize leaves the directory running exactly what it was running.
        let after = ids_of(&listed(&router, &directory_id, &name).await, &name);
        assert_eq!(
            after, before,
            "case {name:?}: expected the controller set unchanged, got {after:?}"
        );
    }

    // And the quota itself IS accepted, so the refusals above are about the number and
    // not about the operation.
    resize(&router, &directory_id, DOMAIN_CONTROLLER_QUOTA, case).await;
    let accepted = ids_of(&listed(&router, &directory_id, case).await, case);
    assert_eq!(
        accepted.len() as i64,
        DOMAIN_CONTROLLER_QUOTA,
        "case {case:?}: expected the quota to be reachable, got {accepted:?}"
    );
}

#[tokio::test]
async fn a_number_below_the_modelled_minimum_is_refused_and_changes_nothing() {
    let router = service();
    let case = "below the minimum";
    let directory_id = directory_in_service(&router, "below.example.com", case).await;
    let before = ids_of(&listed(&router, &directory_id, case).await, case);

    // The modelled range's lower bound is 2, so the values below it — one past the bound,
    // zero and a negative one — are refused by the request walk before any resize runs.
    for desired in [MODELLED_MINIMUM - 1, 0, -1] {
        let name = format!("DesiredNumber {desired}");
        let (status, parsed) = call(
            &router,
            "UpdateNumberOfDomainControllers",
            json!({ "DirectoryId": &directory_id, "DesiredNumber": desired }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400 for a number under the modelled range, got {status} \
             with {parsed}"
        );
        let after = ids_of(&listed(&router, &directory_id, &name).await, &name);
        assert_eq!(
            after, before,
            "case {name:?}: expected the controller set unchanged, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_resize_down_retires_the_controllers_placed_last() {
    let router = service();
    let case = "resize down";
    let directory_id = directory_in_service(&router, "down.example.com", case).await;

    resize(&router, &directory_id, MODELLED_MINIMUM + 3, case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let grown = ids_of(&listed(&router, &directory_id, case).await, case);
    assert_eq!(
        grown.len(),
        5,
        "case {case:?}: expected the grown set, got {grown:?}"
    );

    resize(&router, &directory_id, MODELLED_MINIMUM + 1, case).await;

    // The controllers on their way out are still reported, in the transient status the
    // modelled enum carries for exactly that, until their removal falls due.
    let retiring = listed(&router, &directory_id, case).await;
    assert_eq!(
        ids_of(&retiring, case),
        grown,
        "case {case:?}: expected every controller to be reported while two are retiring, got \
         {retiring:?}"
    );
    for (index, controller) in retiring.iter().enumerate() {
        let expected = if index < 3 {
            STATUS_ACTIVE
        } else {
            STATUS_DELETING
        };
        assert_eq!(
            controller["Status"],
            json!(expected),
            "case {case:?}: expected controller {index} in {expected}, got {controller}"
        );
    }

    // Once the removal falls due the directory runs the number it was asked for, and the
    // ones it kept are the ones it placed FIRST.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let kept = listed(&router, &directory_id, case).await;
    assert_eq!(
        ids_of(&kept, case),
        grown[..3].to_vec(),
        "case {case:?}: expected the three controllers placed first, got {kept:?}"
    );
    for controller in &kept {
        assert_eq!(
            controller["Status"],
            json!(STATUS_ACTIVE),
            "case {case:?}: expected the kept controllers in service, got {controller}"
        );
    }
    let described = describe_one(&router, &directory_id, case).await;
    assert_eq!(
        described["DesiredNumberOfDomainControllers"],
        json!(MODELLED_MINIMUM + 1),
        "case {case:?}: expected the directory to desire the number asked for, got {described}"
    );
}

#[tokio::test]
async fn only_a_reachable_directory_whose_controllers_this_service_runs_is_resized() {
    let router = service();

    // An id naming no stored directory at all: there is nothing to resize.
    refused(
        &router,
        "UpdateNumberOfDomainControllers",
        json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "DesiredNumber": MODELLED_MINIMUM + 1 }),
        "EntityDoesNotExistException",
        "unknown directory",
    )
    .await;

    // A Simple AD directory runs no domain controller of the caller's domain, so it has
    // no set to resize — the same refusal the listing answers for it.
    let simple = create_directory(&router, minimal_create("simple.example.com"), "SimpleAD").await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    refused(
        &router,
        "UpdateNumberOfDomainControllers",
        json!({ "DirectoryId": &simple, "DesiredNumber": MODELLED_MINIMUM + 1 }),
        "UnsupportedOperationException",
        "SimpleAD",
    )
    .await;

    // A directory still being built by the transition its create owes is not reachable
    // for a resize.
    let building = create_microsoft_ad(&router, microsoft_ad("building.example.com"), "building")
        .await;
    refused(
        &router,
        "UpdateNumberOfDomainControllers",
        json!({ "DirectoryId": &building, "DesiredNumber": MODELLED_MINIMUM + 1 }),
        "DirectoryUnavailableException",
        "still being built",
    )
    .await;

    // Once it is in service the same request is accepted, so the refusal above is about
    // the stage and not about the operation.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    resize(&router, &building, MODELLED_MINIMUM + 1, "in service").await;

    // A second resize while the first is still running is refused — "During this time,
    // you cannot make another update request" — and leaves the running one alone.
    refused(
        &router,
        "UpdateNumberOfDomainControllers",
        json!({ "DirectoryId": &building, "DesiredNumber": MODELLED_MINIMUM + 2 }),
        "DirectoryUnavailableException",
        "a resize already running",
    )
    .await;
    let running = "a resize already running";
    let during = ids_of(&listed(&router, &building, running).await, running);
    assert_eq!(
        during.len(),
        3,
        "case {running:?}: expected the first resize's set, got {during:?}"
    );

    // And once that resize's transition has fallen due, the next one is accepted.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    resize(&router, &building, MODELLED_MINIMUM + 2, "after the first resize").await;
    let again = ids_of(
        &listed(&router, &building, "after the first resize").await,
        "after the first resize",
    );
    assert_eq!(
        again.len(),
        4,
        "case {:?}: expected the second resize's set, got {again:?}",
        "after the first resize"
    );
}
