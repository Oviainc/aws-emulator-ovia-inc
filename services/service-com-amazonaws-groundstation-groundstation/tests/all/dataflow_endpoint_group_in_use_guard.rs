// SPDX-License-Identifier: CC0-1.0

//! `DeleteDataflowEndpointGroup` (`DELETE /dataflowEndpointGroup/{dataflowEndpointGroupId}`)
//! end to end over HTTP, through the same front door as the rest of this target's suites.
//!
//! The RULE under test: a resource of this service is REMOVED only while nothing else this
//! account holds still depends on it, because a read of the depending resource resolves that
//! reference — so a dataflow endpoint group holding an endpoint a contact still to be flown
//! is wired to is not deleted. The delete is then refused with the operation's modelled
//! in-use failure (`InvalidParameterException`, HTTP 431 — the operation models no
//! `ResourceInUseException`), naming the contacts that hold it, and the store is left
//! exactly as it was; once every one of them is terminal the delete goes through and the
//! group is gone from every read.
//!
//! The suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * the refusal over the WHOLE `ContactStatus` enum (the one table of it this target
//!   keeps, `crate::mission_profile_deletion::CONTACT_STATUSES`): a contact in each value in
//!   turn wired to a group of its own, with the delete refused for every non-terminal status
//!   and accepted for every terminal one;
//! * over every endpoint member `EndpointDetails` declares (`endpoint_kinds`), since a
//!   `dataflowEndpointConfig` selects an endpoint by NAME whichever member describes it;
//! * the holders the refusal names, over a group whose two endpoints several contacts are
//!   wired to, released one at a time, while a group nothing is wired to, a group another
//!   contact is wired to, and a second group holding an endpoint by a name a live contact is
//!   wired to are each judged on their own.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    service_state, TERMINAL_CONTACT_STATUSES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_group_deletion::assert_group_absent;
use crate::dataflow_endpoint_groups::{
    create_group, delete_group, details, endpoint_kinds, expected_arn, get_group,
};
use crate::minute_usage::{epoch, reserve_pass};
use crate::mission_profile_deletion::{put_in_status, CONTACT_STATUSES};
use crate::scheduling_loop::{
    create_config, create_tracking_config, harness_with_engine, send, spectrum_config,
    stock_ground_stations, ABSENT_UUID, STATION,
};

/// How long each reserved pass runs.
const PASS_SECONDS: i64 = 20 * 60;

/// The sentence this service answers while a contact still to be flown is wired to an
/// endpoint the group holds, with the contacts the store holds listed in it.
///
/// Spelled out here rather than taken from the service's own renderer: an expectation built
/// by calling the code under test would hold whatever that code said, so only the holder ids
/// are interpolated.
fn expected_in_use_message(holders: &[String]) -> String {
    format!(
        "Dataflow Endpoint Group cannot be deleted; it is currently being used by the \
         following contacts: {}",
        holders.join(", ")
    )
}

/// How far apart two rows' windows sit. An antenna serves one pass at a time, so every
/// contact in a suite is reserved in a window of its own.
const WINDOW_STRIDE: i64 = 86_400;

/// The endpoint body of the `EndpointDetails` member `member`, named `name`.
///
/// The body comes from [`endpoint_kinds`] — the one table in this target that lists every
/// endpoint member `EndpointDetails` declares — with only the name replaced, which is what a
/// `dataflowEndpointConfig` selects an endpoint by. `pub(crate)` so a sibling suite naming
/// endpoints of a group describes them from the same table.
pub(crate) fn endpoint_body(member: &str, name: &str) -> Value {
    let kinds = endpoint_kinds();
    let kind = kinds
        .iter()
        .find(|kind| kind.member == member)
        .unwrap_or_else(|| panic!("case {name}: {member} is an endpoint kind of this table"));
    let mut body = kind
        .body
        .as_object()
        .unwrap_or_else(|| panic!("case {name}: the endpoint-kind row carries an object"))
        .clone();
    body.insert("name".to_string(), json!(name));
    Value::Object(body)
}

/// Create a dataflow endpoint group holding one endpoint per entry of `names`, each
/// described by the `EndpointDetails` member `member`; answer its `dataflowEndpointGroupId`.
///
/// Only a DATAFLOW endpoint is created with the security details Ground Station connects its
/// stream with; an agent endpoint's stream is carried by the customer's own agent.
///
/// `pub(crate)` so a sibling suite that needs groups holding named endpoints creates them
/// through this one helper.
pub(crate) async fn group_holding(
    router: &MultiServiceRouter,
    case: &str,
    member: &str,
    names: &[&str],
) -> String {
    let endpoint_details: Vec<Value> = names
        .iter()
        .map(|name| details(member, &endpoint_body(member, name), member == "endpoint"))
        .collect();
    let created = create_group(router, json!({"endpointDetails": endpoint_details})).await;
    assert_eq!(
        created.status, 200,
        "case {case}: expected CreateDataflowEndpointGroup 200, got {} {}",
        created.status, created.body
    );
    created.string("dataflowEndpointGroupId")
}

/// Create a mission profile whose ONE dataflow edge runs from an `antenna-downlink` config
/// into a `dataflowEndpointConfig` selecting the endpoint named `endpoint_name`; answer its
/// `missionProfileArn`.
///
/// A contact reserved against this profile therefore reports one `dataflowList` entry whose
/// destination resolves to that endpoint — exactly what deleting the group holding it would
/// take away.
async fn profile_wired_to(
    router: &MultiServiceRouter,
    case: &str,
    endpoint_name: &str,
) -> String {
    let (_, _, downlink_arn) = create_config(
        router,
        &format!("{case} downlink"),
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let (_, _, endpoint_config_arn) = create_config(
        router,
        &format!("{case} endpoint"),
        json!({"dataflowEndpointConfig": {"dataflowEndpointName": endpoint_name}}),
        None,
    )
    .await;
    let tracking_arn = create_tracking_config(router, &format!("{case} tracking")).await;

    let created = send(
        router,
        "POST",
        "/missionprofile",
        Some(json!({
            "name": format!("{case} profile"),
            "minimumViableContactDurationSeconds": 60,
            "dataflowEdges": [[downlink_arn, endpoint_config_arn]],
            "trackingConfigArn": tracking_arn,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case}: expected CreateMissionProfile 200, got {} {}",
        created.status, created.body
    );
    let profile_id = created.string("missionProfileId");
    let read = send(
        router,
        "GET",
        &format!("/missionprofile/{profile_id}"),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected GetMissionProfile 200, got {} {}",
        read.status, read.body
    );
    read.string("missionProfileArn")
}

/// The NAME of the endpoint the first dataflow of a described contact resolves at its
/// destination, or `None` when that end reports no endpoint at all — which is the state this
/// guard exists to keep a live contact out of.
///
/// Whichever endpoint member describes it carries the name
/// (`smithy_query(query="shape", shape="EndpointDetails")`), so the lookup is for the one
/// member of the reported `endpointDetails` that has one.
fn wired_endpoint_name(body: &Value) -> Option<String> {
    let details = body
        .get("dataflowList")?
        .as_array()?
        .first()?
        .get("destination")?
        .get("configDetails")?
        .get("endpointDetails")?;
    details
        .as_object()?
        .values()
        .find_map(|member| member.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `DescribeContact` for `contact_id`, asserting it answered.
async fn describe_contact(router: &MultiServiceRouter, case: &str, contact_id: &str) -> Value {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeContact 200, got {} {}",
        described.status, described.body
    );
    described.body
}

/// Assert that the delete of `group_id` is refused as in use by exactly `holders`, and that
/// it wrote nothing: the group still reads back and still holds the endpoint `endpoint_name`.
async fn assert_refused_as_in_use(
    router: &MultiServiceRouter,
    case: &str,
    group_id: &str,
    endpoint_name: &str,
    holders: &[String],
) {
    let refused = delete_group(router, group_id).await;
    assert_eq!(
        refused.status, 431,
        "case {case}: a group a contact still to be flown is wired to must answer the \
         modelled InvalidParameterException status 431, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterException",
        "case {case}: wrong error shape, got {}",
        refused.body
    );
    assert_eq!(
        refused.string("message"),
        expected_in_use_message(holders),
        "case {case}: the refusal must name exactly the contacts wired to the group, got {}",
        refused.body
    );

    let kept = get_group(router, group_id).await;
    assert_eq!(
        kept.status, 200,
        "case {case}: a refused delete must leave the group readable, got {} {}",
        kept.status, kept.body
    );
    let held: Vec<String> = kept
        .field("endpointsDetails")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: the group reports its endpoints, got {}",
                kept.body
            )
        })
        .iter()
        .filter_map(|details| {
            details
                .as_object()?
                .values()
                .find_map(|member| member.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(
        held.iter().any(|name| name == endpoint_name),
        "case {case}: the kept group must still hold {endpoint_name:?}, got {held:?}"
    );
}

#[tokio::test]
async fn a_group_is_deleted_only_when_no_contact_still_to_be_flown_is_wired_to_its_endpoints() {
    let (router, engine) = harness_with_engine();
    // The account is served by the station every pass below is booked at.
    stock_ground_stations(&engine, STATION);
    let first_window = epoch(2033, 5, 17, 9, 0);

    for (slot, status) in CONTACT_STATUSES.iter().enumerate() {
        // A group, a profile and a contact of their own per row, in a window of its own, so
        // one row's delete cannot be what the next row observes.
        let endpoint_name = format!("endpoint-{}", status.to_lowercase().replace('_', "-"));
        let group_id = group_holding(&router, status, "endpoint", &[&endpoint_name]).await;
        let profile_arn = profile_wired_to(&router, status, &endpoint_name).await;
        let start = first_window
            + i64::try_from(slot).expect("a row index fits in i64") * WINDOW_STRIDE;
        let contact_id = reserve_pass(&router, &profile_arn, status, start, PASS_SECONDS).await;
        put_in_status(&engine, &contact_id, status);

        // Before the delete, every row's contact resolves the endpoint it is wired to.
        let before = describe_contact(&router, status, &contact_id).await;
        assert_eq!(
            wired_endpoint_name(&before),
            Some(endpoint_name.clone()),
            "case {status}: the contact must resolve the endpoint its dataflow is wired to, \
             got {before}"
        );

        if TERMINAL_CONTACT_STATUSES.contains(status) {
            let deleted = delete_group(&router, &group_id).await;
            assert_eq!(
                deleted.status, 200,
                "case {status}: a group only a terminal contact is wired to must delete, got \
                 {} {}",
                deleted.status, deleted.body
            );
            assert_eq!(
                deleted.string("dataflowEndpointGroupId"),
                group_id,
                "case {status}: the delete must answer the id it removed, got {}",
                deleted.body
            );
            assert_group_absent(
                &format!("{status}: read after the delete"),
                &get_group(&router, &group_id).await,
            );
            // The contact no longer depends on what it was wired to, and says so: its
            // dataflow reports no endpoint behind that end any more.
            let after = describe_contact(&router, status, &contact_id).await;
            assert_eq!(
                wired_endpoint_name(&after),
                None,
                "case {status}: a terminal contact whose group is gone must resolve no \
                 endpoint, got {after}"
            );
            continue;
        }

        assert_refused_as_in_use(
            &router,
            status,
            &group_id,
            &endpoint_name,
            std::slice::from_ref(&contact_id),
        )
        .await;

        // A refused delete writes nothing, so the contact still resolves its endpoint out of
        // the group that holds it.
        let after = describe_contact(&router, status, &contact_id).await;
        assert_eq!(
            wired_endpoint_name(&after),
            Some(endpoint_name.clone()),
            "case {status}: a refused delete must leave the contact resolving its endpoint, \
             got {after}"
        );
        let state = service_state(&*engine).expect("this service's state store");
        let guard = state.lock();
        assert!(
            guard.dataflow_endpoint_groups.contains_key(&group_id),
            "case {status}: a refused delete must leave the group in state"
        );
        assert!(
            guard.arn_index.contains_key(&expected_arn(&group_id)),
            "case {status}: a refused delete must leave the group's ARN resolving"
        );
    }
}

#[tokio::test]
async fn the_guard_holds_for_every_endpoint_member_and_lifts_once_the_contact_is_released() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let first_window = epoch(2034, 8, 21, 7, 0);

    // Rows differ along which member of `EndpointDetails` describes the endpoint the
    // contact's dataflow selects: the selection is by name, so the guard cannot depend on
    // the member.
    for (slot, kind) in endpoint_kinds().iter().enumerate() {
        let case = kind.label;
        let endpoint_name = format!("wired-{}", kind.member.to_lowercase());
        let group_id = group_holding(&router, case, kind.member, &[&endpoint_name]).await;
        let profile_arn = profile_wired_to(&router, case, &endpoint_name).await;
        let start = first_window
            + i64::try_from(slot).expect("a row index fits in i64") * WINDOW_STRIDE;
        let contact_id = reserve_pass(&router, &profile_arn, case, start, PASS_SECONDS).await;

        assert_refused_as_in_use(
            &router,
            case,
            &group_id,
            &endpoint_name,
            std::slice::from_ref(&contact_id),
        )
        .await;

        // Releasing the reservation leaves the contact terminal, so nothing is still to be
        // flown through that endpoint and the same delete lands.
        let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
        assert_eq!(
            cancelled.status, 200,
            "case {case}: expected CancelContact 200, got {} {}",
            cancelled.status, cancelled.body
        );
        let deleted = delete_group(&router, &group_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: with its only holder released the delete must land, got {} {}",
            deleted.status, deleted.body
        );
        assert_group_absent(case, &get_group(&router, &group_id).await);
    }
}

#[tokio::test]
async fn the_refusal_names_every_contact_wired_to_the_group_and_no_other() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let first_window = epoch(2035, 3, 4, 12, 0);
    let mut slot = 0_i64;
    let mut next_window = || {
        slot += 1;
        first_window + (slot - 1) * WINDOW_STRIDE
    };

    // ONE group holding TWO endpoints, with a profile wired to each: a group is in use
    // through ANY endpoint it holds, so the count of endpoints and which of them is selected
    // are not part of the rule.
    let shared = group_holding(&router, "shared", "endpoint", &["shared-a", "shared-b"]).await;
    let first_profile = profile_wired_to(&router, "shared-a", "shared-a").await;
    let second_profile = profile_wired_to(&router, "shared-b", "shared-b").await;

    let mut holders: Vec<String> = Vec::new();
    for (case, profile_arn) in [
        ("holder-a1", &first_profile),
        ("holder-a2", &first_profile),
        ("holder-b1", &second_profile),
    ] {
        holders.push(reserve_pass(&router, profile_arn, case, next_window(), PASS_SECONDS).await);
    }
    // The refusal lists the holders in a stable order, so it reads the same on every call.
    let mut expected = holders.clone();
    expected.sort();

    // A group nothing is wired to, and a group ANOTHER contact is wired to: each group is
    // judged by the contacts wired to IT, not by every contact the store holds.
    let spare = group_holding(&router, "spare", "endpoint", &["spare-endpoint"]).await;
    let other = group_holding(&router, "other", "endpoint", &["other-endpoint"]).await;
    let other_profile = profile_wired_to(&router, "other", "other-endpoint").await;
    let other_holder =
        reserve_pass(&router, &other_profile, "other", next_window(), PASS_SECONDS).await;

    assert_refused_as_in_use(&router, "three-holders", &shared, "shared-a", &expected).await;
    assert_refused_as_in_use(
        &router,
        "other-group",
        &other,
        "other-endpoint",
        std::slice::from_ref(&other_holder),
    )
    .await;

    // A name is unique only within ONE group, so a second group may hold an endpoint by a
    // name a live contact is wired to as well. A read resolves that name to one of the two
    // while the account holds both, so a contact wired to it holds either group and neither
    // delete decides for the caller which of them their pass was meant to run through.
    let twin = group_holding(&router, "twin", "endpoint", &["shared-a"]).await;
    let mut wired_to_shared_a: Vec<String> = holders[..2].to_vec();
    wired_to_shared_a.sort();
    assert_refused_as_in_use(&router, "twin-group", &twin, "shared-a", &wired_to_shared_a).await;

    let deleted_spare = delete_group(&router, &spare).await;
    assert_eq!(
        deleted_spare.status, 200,
        "case spare: a group no contact is wired to must delete while other contacts stand, \
         got {} {}",
        deleted_spare.status, deleted_spare.body
    );

    // An id naming no group at all is still the missing-resource refusal, not the in-use one.
    assert_group_absent(
        "an id the account never held",
        &delete_group(&router, ABSENT_UUID).await,
    );

    // Released one at a time: a cancel of a pass that has yet to start leaves the contact
    // terminal, so it stops holding the group while the others still do.
    let mut still_held = expected.clone();
    for (released, holder) in holders.iter().enumerate() {
        let cancelled = send(&router, "DELETE", &format!("/contact/{holder}"), None).await;
        assert_eq!(
            cancelled.status, 200,
            "case release-{released}: expected CancelContact 200, got {} {}",
            cancelled.status, cancelled.body
        );
        still_held.retain(|contact_id| contact_id != holder);

        if still_held.is_empty() {
            let deleted = delete_group(&router, &shared).await;
            assert_eq!(
                deleted.status, 200,
                "case release-{released}: with every holder released the delete must land, \
                 got {} {}",
                deleted.status, deleted.body
            );
            assert_eq!(
                deleted.string("dataflowEndpointGroupId"),
                shared,
                "case release-{released}: the delete must answer the id it removed, got {}",
                deleted.body
            );
        } else {
            assert_refused_as_in_use(
                &router,
                &format!("release-{released}"),
                &shared,
                "shared-a",
                &still_held,
            )
            .await;
        }
    }

    assert_group_absent("the deleted group", &get_group(&router, &shared).await);
    // The group the released contacts were never wired to is untouched by any of it.
    let still = get_group(&router, &other).await;
    assert_eq!(
        still.status, 200,
        "case other-group: the group another contact is wired to must still read back, got \
         {} {}",
        still.status, still.body
    );
}
