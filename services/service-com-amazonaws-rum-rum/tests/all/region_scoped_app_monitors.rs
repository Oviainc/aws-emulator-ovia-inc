// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the REGIONAL identity of a CloudWatch RUM app monitor.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — against ONE emulator, so a request signed for one region meets the
//! same store as a request signed for another and cross-region leakage is actually
//! reachable (a case that built a fresh emulator per region could not see it).
//!
//! The rule the suite pins: a RUM app monitor belongs to the region the request that
//! created it was signed for, so its name only has to be unique within that region, and
//! every later request — whether it names the monitor by `Name`, by its minted `Id` or by
//! its `ResourceArn` — resolves it among the monitors of the region THAT request was
//! signed for and reaches no other region's.
//!
//! The assertions pin that rule over a SET of inputs rather than one request: the region
//! catalog spans four regions in three partitions-worth of geographies, the operation
//! table walks every operation that addresses a monitor by name, the listing case varies
//! how MANY monitors a region owns (0, 1, 2 and 3), and the id and ARN cases cover the two
//! non-name ways a request names a monitor.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_rum_rum::state::MAX_REGIONS;
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{
    call_in, emulator, is_modelled_app_monitor_id, message, percent_encoded, rum_events_body,
    ACCOUNT,
};

/// The regions the rule is asserted over: a monitor is created in each and must stay
/// invisible from the others. Any `credentialScope` region an endpoint can be signed for
/// is admissible — the service reads the region off the SigV4 credential scope — so the
/// catalog spans distant geographies rather than one pair.
const REGIONS: [&str; 4] = ["us-east-1", "eu-west-2", "ap-southeast-1", "sa-east-1"];

/// The literal an app-monitor not-found message starts with, ahead of the requested name.
/// Source: `crate::state`'s `NOT_FOUND_MESSAGE_PREFIX`, pinned by this crate's
/// not-found-body suite.
const NOT_FOUND_PREFIX: &str = "Resource not found ";

/// The wire error code RUM's tagging frontend answers for a `ResourceArn` naming no
/// resource it serves, and the whole message that refusal carries. Source: this crate's
/// `list_tags_for_resource` suite, which pins both from the recorded corpus.
const TAG_NOT_FOUND_ERROR_CODE: &str = "NotFoundException";
const TAG_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The ARN a created app monitor carries:
/// `arn:aws:rum:<region>:<account>:appmonitor/<name>`.
fn app_monitor_arn(region: &str, name: &str) -> String {
    format!("arn:aws:rum:{region}:{ACCOUNT}:appmonitor/{name}")
}

/// `CreateAppMonitor` signed for `region`, returning the whole reply so a case can assert
/// on a refusal as well as on a success.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    body: Value,
) -> crate::support::Reply {
    call_in(router, region, "POST", "/appmonitor", Some(body)).await
}

/// `CreateAppMonitor` signed for `region`, asserting it succeeded and returning the minted
/// `Id`.
async fn created_id(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    body: Value,
) -> String {
    let reply = create_in(router, region, body).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected CreateAppMonitor signed for {region} to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let id = reply.json(case)["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAppMonitor returned no Id"))
        .to_string();
    assert!(
        is_modelled_app_monitor_id(&id),
        "case {case:?}: expected a modelled AppMonitorId, got {id:?}"
    );
    id
}

/// `GetAppMonitor` signed for `region`.
async fn get_in(router: &MultiServiceRouter, region: &str, name: &str) -> crate::support::Reply {
    call_in(router, region, "GET", &format!("/appmonitor/{name}"), None).await
}

/// The names `ListAppMonitors` reports to a request signed for `region`, following every
/// page token to the end so the set is the region's whole listing rather than one page.
async fn listed_names(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let body = token
            .as_ref()
            .map(|issued| json!({"NextToken": issued}))
            .unwrap_or_else(|| json!({}));
        let reply = call_in(router, region, "POST", "/appmonitors", Some(body)).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected ListAppMonitors signed for {region} to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let page = reply.json(case);
        for summary in page["AppMonitorSummaries"].as_array().into_iter().flatten() {
            let name = summary["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a summary carries no Name"))
                .to_string();
            assert!(
                names.insert(name.clone()),
                "case {case:?}: {name:?} was listed twice"
            );
        }
        match page["NextToken"].as_str() {
            Some(issued) => token = Some(issued.to_string()),
            None => break,
        }
    }
    names
}

/// Assert a reply is the app-monitor not-found refusal for `name`: HTTP 404, the modelled
/// `ResourceNotFoundException`, and the request-derived message.
fn assert_app_monitor_not_found(reply: &crate::support::Reply, case: &str, name: &str) {
    assert_eq!(
        reply.status,
        404,
        "case {case:?}: expected HTTP 404, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let code = reply.error_code(case);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {code:?}"
    );
    let document = reply.json(case);
    let expected = format!("{NOT_FOUND_PREFIX}{name}");
    assert_eq!(
        message(&document),
        Some(expected.as_str()),
        "case {case:?}: expected the message {expected:?}, got {:?}",
        message(&document)
    );
}

#[tokio::test]
async fn one_name_names_an_independent_monitor_in_each_region() {
    let (router, _engine) = emulator();
    // One name, created in every region of the catalog: each create must succeed, because
    // a name is only taken within the region that holds it.
    let name = "shop";
    let mut ids: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let case = format!("create in {region}");
        // A per-region domain, so the records are distinguishable by a member a read
        // reports and not only by their ids.
        let id = created_id(
            &router,
            &case,
            region,
            json!({"Name": name, "Domain": format!("{region}.example.com")}),
        )
        .await;
        ids.push((region, id));
    }

    // Independent resources: no two regions' monitors share an id.
    let distinct: BTreeSet<&String> = ids.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "case \"distinct ids\": expected one id per region, got {ids:?}"
    );

    // And each region reports ITS own monitor: the id its own create minted and the
    // domain its own create named.
    for (region, id) in &ids {
        let case = format!("read in {region}");
        let reply = get_in(&router, region, name).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let monitor = reply.json(&case)["AppMonitor"].clone();
        assert_eq!(
            monitor["Id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the monitor created in {region}, got {:?}",
            monitor["Id"]
        );
        let domain = format!("{region}.example.com");
        assert_eq!(
            monitor["Domain"].as_str(),
            Some(domain.as_str()),
            "case {case:?}: expected Domain {domain:?}, got {:?}",
            monitor["Domain"]
        );
    }

    // A write in one region reaches that region's monitor alone, and a delete in one
    // region leaves every other region's monitor of the same name collecting data.
    let updated_region = REGIONS[1];
    let patched = call_in(
        &router,
        updated_region,
        "PATCH",
        &format!("/appmonitor/{name}"),
        Some(json!({"Domain": "patched.example.com"})),
    )
    .await;
    assert_eq!(
        patched.status,
        200,
        "case \"update one region\": expected UpdateAppMonitor to answer 200, got {} {:?}",
        patched.status,
        String::from_utf8_lossy(&patched.body)
    );
    let deleted_region = REGIONS[0];
    let deleted = call_in(
        &router,
        deleted_region,
        "DELETE",
        &format!("/appmonitor/{name}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status,
        200,
        "case \"delete one region\": expected DeleteAppMonitor to answer 200, got {} {:?}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    for (region, id) in &ids {
        let case = format!("after the write and the delete, {region}");
        let reply = get_in(&router, region, name).await;
        if *region == deleted_region {
            assert_app_monitor_not_found(&reply, &case, name);
            continue;
        }
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let monitor = reply.json(&case)["AppMonitor"].clone();
        assert_eq!(
            monitor["Id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the monitor created in {region}, got {:?}",
            monitor["Id"]
        );
        let expected_domain = if *region == updated_region {
            "patched.example.com".to_string()
        } else {
            format!("{region}.example.com")
        };
        assert_eq!(
            monitor["Domain"].as_str(),
            Some(expected_domain.as_str()),
            "case {case:?}: expected Domain {expected_domain:?}, got {:?}",
            monitor["Domain"]
        );
    }
}

#[tokio::test]
async fn a_name_addressed_operation_reaches_only_its_own_regions_monitor() {
    let (router, _engine) = emulator();
    let owner = REGIONS[0];
    let other = REGIONS[2];

    // Every operation that addresses an app monitor by its name, as
    // (case, method, path template, body). Source: the crate's `OPERATIONS.md` routes.
    let operations: [(&str, &str, &str, Option<Value>); 11] = [
        ("GetAppMonitor", "GET", "/appmonitor/{name}", None),
        (
            "UpdateAppMonitor",
            "PATCH",
            "/appmonitor/{name}",
            Some(json!({"Domain": "other.example.com"})),
        ),
        ("DeleteAppMonitor", "DELETE", "/appmonitor/{name}", None),
        (
            "GetAppMonitorData",
            "POST",
            "/appmonitor/{name}/data",
            // `TimeRange` is `@required`, so the body carries the window the query is
            // taken over; `After: 0` spans everything the monitor holds.
            Some(json!({"TimeRange": {"After": 0}})),
        ),
        ("GetResourcePolicy", "GET", "/appmonitor/{name}/policy", None),
        (
            "PutResourcePolicy",
            "PUT",
            "/appmonitor/{name}/policy",
            Some(json!({"PolicyDocument": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                    "Action": "rum:PutRumEvents",
                    "Resource": "*"
                }]
            })
            .to_string()})),
        ),
        (
            "DeleteResourcePolicy",
            "DELETE",
            "/appmonitor/{name}/policy",
            None,
        ),
        (
            "PutRumMetricsDestination",
            "POST",
            "/rummetrics/{name}/metricsdestination",
            Some(json!({"Destination": "CloudWatch"})),
        ),
        (
            "ListRumMetricsDestinations",
            "GET",
            "/rummetrics/{name}/metricsdestination",
            None,
        ),
        (
            "DeleteRumMetricsDestination",
            "DELETE",
            "/rummetrics/{name}/metricsdestination?destination=CloudWatch",
            None,
        ),
        (
            "BatchGetRumMetricDefinitions",
            "GET",
            "/rummetrics/{name}/metrics?destination=CloudWatch",
            None,
        ),
    ];

    for (index, (op, method, template, body)) in operations.into_iter().enumerate() {
        // One monitor per row, created in the owning region only, so one row's write
        // cannot change what the next row resolves.
        let name = format!("owned-{index}");
        let case = format!("{op} from {other}");
        created_id(
            &router,
            &case,
            owner,
            json!({"Name": name, "Domain": "example.com"}),
        )
        .await;
        let path = template.replace("{name}", &name);

        // Signed for a region that owns no monitor of that name: the name resolves to
        // nothing there, which is the service's app-monitor not-found refusal.
        let refused = call_in(&router, other, method, &path, body.clone()).await;
        assert_app_monitor_not_found(&refused, &case, &name);

        // The very same call, signed for the region that DOES own it, gets past the
        // app-monitor resolution — so the refusal above is about the region, not about
        // the request. (Getting past it is all that is asserted: a row may still be
        // refused for a resource BELOW the monitor, like a policy or a destination it
        // does not hold, which is a different refusal with a different message.)
        let owning_case = format!("{op} from {owner}");
        let served = call_in(&router, owner, method, &path, body).await;
        let not_found = format!("{NOT_FOUND_PREFIX}{name}");
        let served_message = (served.status == 404)
            .then(|| served.json(&owning_case))
            .and_then(|document| message(&document).map(str::to_string));
        assert_ne!(
            served_message.as_deref(),
            Some(not_found.as_str()),
            "case {owning_case:?}: the owning region must resolve the monitor, got {} {:?}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
    }
}

#[tokio::test]
async fn a_listing_reports_only_the_monitors_of_the_region_it_is_signed_for() {
    let (router, _engine) = emulator();
    // The row count varies per region — 0, 1, 2 and 3 monitors — so the listing is
    // asserted on an empty region, a single one and regions holding several.
    let mut expected: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for (count, region) in REGIONS.into_iter().enumerate() {
        let mut names = BTreeSet::new();
        for index in 0..count {
            // The SAME names in every region, so a listing that leaked would be reporting
            // names it must not see rather than merely more rows.
            let name = format!("listed-{index}");
            created_id(
                &router,
                &format!("seed {name} in {region}"),
                region,
                json!({"Name": name, "Domain": "example.com"}),
            )
            .await;
            names.insert(name);
        }
        expected.push((region, names));
    }

    for (region, names) in &expected {
        let case = format!("list in {region}");
        let listed = listed_names(&router, &case, region).await;
        assert_eq!(
            listed, *names,
            "case {case:?}: expected exactly {names:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_minted_id_resolves_only_in_the_region_that_minted_it() {
    let (router, _engine) = emulator();
    let owner = REGIONS[0];
    let id = created_id(
        &router,
        "events owner",
        owner,
        json!({"Name": "events-regional", "Domain": "example.com"}),
    )
    .await;

    // The data plane is as regional as the control plane: the id names a monitor of the
    // region it was minted in, so from any other region it resolves to nothing — and an
    // id that resolves to nothing leaves no resource to authorize the write against,
    // which is the refusal this service answers for an unresolvable id.
    for region in REGIONS.into_iter().filter(|region| *region != owner) {
        let case = format!("PutRumEvents from {region}");
        let reply = call_in(
            &router,
            region,
            "POST",
            &format!("/appmonitors/{id}/"),
            Some(rum_events_body(&id)),
        )
        .await;
        assert_eq!(
            reply.status,
            403,
            "case {case:?}: expected HTTP 403 for an id of another region, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(&case);
        assert_eq!(
            code, "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {code:?}"
        );
    }

    // The same batch, signed for the region that minted the id, is accepted.
    let accepted = call_in(
        &router,
        owner,
        "POST",
        &format!("/appmonitors/{id}/"),
        Some(rum_events_body(&id)),
    )
    .await;
    assert_eq!(
        accepted.status,
        200,
        "case \"PutRumEvents from the minting region\": expected 200, got {} {:?}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
}

#[tokio::test]
async fn an_arn_addressed_tag_write_reaches_only_its_own_regions_monitor() {
    let (router, _engine) = emulator();
    let owner = REGIONS[1];
    let name = "tagged-regional";
    created_id(
        &router,
        "tag owner",
        owner,
        json!({"Name": name, "Domain": "example.com", "Tags": {"Env": "owner"}}),
    )
    .await;
    let arn = app_monitor_arn(owner, name);
    let tags_path = format!("/tags/{}", percent_encoded(&arn));

    // (case, method, path, body): the two tag WRITES and the tag read, each addressed by
    // the ARN the create minted but signed for a region that owns no such resource.
    let rows: [(&str, &str, String, Option<Value>); 3] = [
        (
            "TagResource",
            "POST",
            tags_path.clone(),
            Some(json!({"Tags": {"Env": "intruder"}})),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("{tags_path}?tagKeys=Env"),
            None,
        ),
        ("ListTagsForResource", "GET", tags_path.clone(), None),
    ];

    for (op, method, path, body) in rows {
        for region in REGIONS.into_iter().filter(|region| *region != owner) {
            let case = format!("{op} from {region}");
            let reply = call_in(&router, region, method, &path, body.clone()).await;
            assert_eq!(
                reply.status,
                404,
                "case {case:?}: expected HTTP 404 for an ARN of another region, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let code = reply.error_code(&case);
            assert_eq!(
                code, TAG_NOT_FOUND_ERROR_CODE,
                "case {case:?}: expected {TAG_NOT_FOUND_ERROR_CODE}, got {code:?}"
            );
            assert_eq!(
                message(&reply.json(&case)),
                Some(TAG_NOT_FOUND_MESSAGE),
                "case {case:?}: expected the message {TAG_NOT_FOUND_MESSAGE:?}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
        }
    }

    // None of those refused calls touched the monitor: its tags are exactly what its
    // create seeded, read back from the region that owns it.
    let reply = call_in(&router, owner, "GET", &tags_path, None).await;
    assert_eq!(
        reply.status,
        200,
        "case \"owner reads its tags\": expected 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let document = reply.json("owner reads its tags");
    assert_eq!(
        document["Tags"],
        json!({"Env": "owner"}),
        "case \"owner reads its tags\": expected the seeded tags, got {:?}",
        document["Tags"]
    );
}

#[tokio::test]
async fn the_store_keeps_monitors_for_a_bounded_number_of_regions() {
    // The region is read off the request's own credential scope, so the number of
    // per-region buckets is bounded as well as the monitors inside each: once monitors
    // exist in `MAX_REGIONS` regions, a create signed for a region the store holds none
    // for is refused with the error the operation models for an exceeded allowance,
    // rather than opening another bucket.
    let (router, _engine) = emulator();
    let name = "bounded";
    for index in 0..MAX_REGIONS {
        // Region strings of the shape a credential scope carries, one per bucket.
        let region = format!("us-test-{index}");
        created_id(
            &router,
            &format!("fill bucket {index}"),
            &region,
            json!({"Name": name, "Domain": "example.com"}),
        )
        .await;
    }

    let overflowing = create_in(
        &router,
        "us-test-overflow",
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
    assert_eq!(
        overflowing.status,
        402,
        "case \"one region too many\": expected the modelled quota refusal (HTTP 402), got {} {:?}",
        overflowing.status,
        String::from_utf8_lossy(&overflowing.body)
    );
    let code = overflowing.error_code("one region too many");
    assert_eq!(
        code, "ServiceQuotaExceededException",
        "case \"one region too many\": expected ServiceQuotaExceededException, got {code:?}"
    );

    // A region the store already holds monitors for is still served: the ceiling is on
    // how many regions hold records, not on the calls a filled region may make.
    let served = get_in(&router, "us-test-0", name).await;
    assert_eq!(
        served.status,
        200,
        "case \"a filled region still reads\": expected 200, got {} {:?}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
    let other_name = "bounded-second";
    created_id(
        &router,
        "a filled region still writes",
        "us-test-0",
        json!({"Name": other_name, "Domain": "example.com"}),
    )
    .await;
}

#[tokio::test]
async fn a_duplicate_name_conflict_is_taken_per_region() {
    // The duplicate-name conflict is taken per region, so the SECOND create of a name in
    // one region is refused while the first create of that name in another region is not:
    // the conflict is about the region's own names.
    let (router, _engine) = emulator();
    let name = "conflict-regional";
    created_id(
        &router,
        "first create",
        REGIONS[0],
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;

    let repeated = create_in(
        &router,
        REGIONS[0],
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
    assert_eq!(
        repeated.status,
        409,
        "case \"same region twice\": expected HTTP 409, got {} {:?}",
        repeated.status,
        String::from_utf8_lossy(&repeated.body)
    );
    let code = repeated.error_code("same region twice");
    assert_eq!(
        code, "ConflictException",
        "case \"same region twice\": expected ConflictException, got {code:?}"
    );

    for region in REGIONS.into_iter().skip(1) {
        let case = format!("first create in {region}");
        created_id(
            &router,
            &case,
            region,
            json!({"Name": name, "Domain": "example.com"}),
        )
        .await;
    }
}
