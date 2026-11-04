// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the page tokens of the lists this service NESTS under a parent
//! resource, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! What a token names for one list, and what makes a value no token of it at all, is
//! [`crate::page_boundary_rule`]; this suite states the half of that rule that is about
//! WHICH list a token belongs to and WHAT in a list its boundary names, over a set of
//! parents and a set of installs:
//!
//! 1. A nested list is ONE parent's list — the parents are the resources the modelled
//!    route binds, and the items are held on the parent's own record — so the token a
//!    page of it issues is a boundary of that parent's list and of no other. Replayed
//!    on another parent's list of the same kind, on the same route, it is refused as a
//!    bad request (400 `ValidationException`) whose report names the operation it was
//!    asked of, never honoured as that list's boundary and so never served as a short
//!    page with the items before it silently dropped. Each parent's own token resumes
//!    its own list, so paging one from the first token to the last yields that parent's
//!    whole list once, in order.
//! 2. The boundary such a token carries names the ITEM the page stopped at rather than
//!    the place that item sat in. Hardware installed on an Outpost takes a creation
//!    sequence of its own, so an install that REPLACES the hardware an Outpost holds —
//!    the configuration a newly named `SupportedHardwareType` names — places units
//!    created after every boundary cut from the hardware that came out, and the page a
//!    caller resumes from such a boundary drops none of the units now installed.
//!
//! Where the recorded items come from: hardware is installed by Amazon Web Services
//! once an order placed on the Outpost is fulfilled, and an instance is launched through
//! Amazon EC2 — no call in THIS API creates either — so the Outposts below order their
//! hardware over the wire ([`crate::site_and_outpost_lifecycle::install_hardware`]) and
//! record their instances through the same store seam
//! [`crate::blocking_instances`] records its own through.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::page_token_report;

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::AssetInstance;

use crate::blocking_instances::{record as record_instances, running};
use crate::capacity_task_lifecycle::{pool, settle_capacity_task, start_ok};
use crate::site_and_outpost_lifecycle::{
    install_hardware, orderable_site, query_value, segment, settle, text, Outposts,
};

/// The member the modelled `ValidationException` carries its report in.
///
/// Source: `smithy_query(query="shape", shape="ValidationException")` against this
/// service's model — `{"members": {"Message": {"target": "ErrorMessage"}}}`.
const MODELLED_MESSAGE: &str = "Message";

/// The page size a case reads a whole list at: the ceiling the modelled `MaxResults1000`
/// declares (`range(1..1000)`), which every list here is far inside.
const WHOLE_LIST: usize = 1000;

/// How many pages a walk may take before the case gives up rather than looping: the
/// longest list below is the instance types an Outpost's hardware supports, which is
/// the published catalog narrowed to one form factor, so this is well clear of it and
/// a token that failed to advance still stops here with the pages it collected
/// instead of running forever.
const PAGE_LIMIT: usize = 256;

/// One site and one Outpost on it carrying `form_factor`, as that Outpost's
/// `OutpostId`. An Outpost belongs to a site, so the site comes first — the same order a
/// real workflow takes.
async fn an_outpost(outposts: &Outposts, name: &str, form_factor: &str) -> String {
    let site = outposts
        .create_site(&orderable_site(&format!("{name}-site")))
        .await;
    let created = outposts
        .create_outpost(&json!({
            "Name": name,
            "SiteId": text(&site, "SiteId"),
            "SupportedHardwareType": form_factor,
        }))
        .await;
    text(&created, "OutpostId")
}

/// `StartCapacityTask` on one Outpost, as the `CapacityTaskId` it minted.
async fn a_task(outposts: &Outposts, outpost_id: &str, request: &Value) -> String {
    text(
        &start_ok(outposts, outpost_id, request).await,
        "CapacityTaskId",
    )
}

/// The `AssetId` of the first COMPUTE unit of the hardware one Outpost holds — the only
/// kind of unit capacity is configured on, so the only kind a capacity task can name.
async fn a_compute_asset(outposts: &Outposts, outpost_id: &str) -> String {
    let held = outposts
        .ok(
            "GET",
            &format!("{}?MaxResults={WHOLE_LIST}", assets_route(outpost_id)),
            None,
        )
        .await;
    let compute = reported(&held, "Assets", "a compute unit of the installed hardware")
        .into_iter()
        .find(|asset| text(asset, "AssetType") == "COMPUTE")
        .unwrap_or_else(|| panic!("the installed hardware holds a compute unit, got {held}"));
    text(&compute, "AssetId")
}

/// The route `ListAssets` binds for one Outpost.
fn assets_route(outpost_id: &str) -> String {
    format!("/outposts/{}/assets", segment(outpost_id))
}

/// One paged list nested under a parent resource: the operation that serves it, the
/// output member it reports its items in, and the routes of TWO lists of that same kind
/// cut from different parents.
struct Nested {
    case: &'static str,
    operation: &'static str,
    member: &'static str,
    one: String,
    another: String,
}

/// The items one page reports, as the list its output member carries.
fn reported(output: &Value, member: &str, case: &str) -> Vec<Value> {
    output
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} list, got {output}"))
}

/// Every item the list at `path` holds, as the pages of `size` a caller walks with the
/// tokens that list hands it.
async fn walk(
    outposts: &Outposts,
    path: &str,
    member: &str,
    case: &str,
    size: usize,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..PAGE_LIMIT {
        let query = match &token {
            None => format!("?MaxResults={size}"),
            Some(token) => format!("?MaxResults={size}&NextToken={}", query_value(token)),
        };
        let page = outposts.ok("GET", &format!("{path}{query}"), None).await;
        collected.extend(reported(&page, member, case));
        match page.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return collected,
        }
    }
    panic!("case {case:?}: paging {path} {size} at a time must terminate, got {collected:?}")
}

/// Rule 1, over every list this service nests under a parent and over both directions
/// of every pair of parents.
///
/// The rows differ along what the rule generalises over: the kind of nested list (the
/// hardware installed on an Outpost, the instances recorded on it, the instances one of
/// its capacity tasks would need stopped, the instance types its capacity is configured
/// with, the ones its hardware supports), which parent of the route differs (the
/// Outpost, and — for the list nested two deep — the capacity task under one Outpost),
/// and which of the two parents the token was minted on.
#[tokio::test]
async fn a_nested_page_token_is_a_boundary_of_its_own_parents_list_only() {
    let outposts = Outposts::new();
    // Two Outposts of one region, each holding the hardware its own order installed:
    // two lists of every kind below, each non-empty, so a page served off the wrong
    // parent shows up as a missing item rather than as nothing at all.
    let one = an_outpost(&outposts, "one", "RACK").await;
    let another = an_outpost(&outposts, "another", "RACK").await;
    for outpost_id in [&one, &another] {
        install_hardware(&outposts, outpost_id).await;
    }
    // A capacity task of each Outpost, and a SECOND task of the first one: an Outpost
    // runs one active capacity task at a time, so the first is let finish before the
    // next is started. Each of the first tasks configures capacity on ONE compute unit
    // of its Outpost, so the configuration standing on the Outpost afterwards names
    // more than one instance type and `GetOutpostInstanceTypes` has a report to page.
    let mut tasks: Vec<String> = Vec::new();
    let mut named_assets: Vec<String> = Vec::new();
    for outpost_id in [&one, &another] {
        let asset_id = a_compute_asset(&outposts, outpost_id).await;
        tasks.push(
            a_task(
                &outposts,
                outpost_id,
                &json!({
                    "AssetId": &asset_id,
                    "InstancePools": [pool("m5.4xlarge", 1), pool("c5.4xlarge", 1)],
                }),
            )
            .await,
        );
        named_assets.push(asset_id);
    }
    let (task_of_one, task_of_another) = (tasks[0].clone(), tasks[1].clone());
    settle_capacity_task();
    let later_task_of_one = a_task(
        &outposts,
        &one,
        &json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
    )
    .await;
    // The instances running on each Outpost, recorded after the tasks were started so
    // the capacity they hold is no part of what those tasks were weighed against. Each
    // Outpost's run on the compute unit its own task named, so they are instances that
    // task would need stopped — a task scoped to one asset is held up by nothing running
    // on another ([`crate::blocking_instance_asset_scope`] states that scope).
    for (name, outpost_id, asset_id) in [
        ("one", &one, &named_assets[0]),
        ("another", &another, &named_assets[1]),
    ] {
        let instances: Vec<AssetInstance> = running()
            .into_iter()
            .map(|instance| AssetInstance {
                asset_id: Some(asset_id.clone()),
                ..instance
            })
            .collect();
        record_instances(&outposts, outpost_id, &instances, name);
    }

    let blocking = |outpost_id: &str, task_id: &str| {
        format!(
            "/outposts/{}/capacity/{}/blockingInstances",
            segment(outpost_id),
            segment(task_id)
        )
    };
    let rows = vec![
        Nested {
            case: "the hardware installed on another Outpost",
            operation: "ListAssets",
            member: "Assets",
            one: assets_route(&one),
            another: assets_route(&another),
        },
        Nested {
            case: "the instances recorded on another Outpost",
            operation: "ListAssetInstances",
            member: "AssetInstances",
            one: format!("/outposts/{}/assetInstances", segment(&one)),
            another: format!("/outposts/{}/assetInstances", segment(&another)),
        },
        Nested {
            case: "a capacity task of another Outpost",
            operation: "ListBlockingInstancesForCapacityTask",
            member: "BlockingInstances",
            one: blocking(&one, &task_of_one),
            another: blocking(&another, &task_of_another),
        },
        Nested {
            case: "another capacity task of the same Outpost",
            operation: "ListBlockingInstancesForCapacityTask",
            member: "BlockingInstances",
            one: blocking(&one, &task_of_one),
            another: blocking(&one, &later_task_of_one),
        },
        Nested {
            case: "the capacity configured on another Outpost",
            operation: "GetOutpostInstanceTypes",
            member: "InstanceTypes",
            one: format!("/outposts/{}/instanceTypes", segment(&one)),
            another: format!("/outposts/{}/instanceTypes", segment(&another)),
        },
        Nested {
            case: "the instance types another Outpost's hardware supports",
            operation: "GetOutpostSupportedInstanceTypes",
            member: "InstanceTypes",
            one: format!("/outposts/{}/supportedInstanceTypes", segment(&one)),
            another: format!("/outposts/{}/supportedInstanceTypes", segment(&another)),
        },
    ];

    for row in &rows {
        for (from, onto) in [(&row.one, &row.another), (&row.another, &row.one)] {
            let case = format!("{} ({} minted on {from}, replayed on {onto})", row.case, row.operation);
            // The whole of the list the token is replayed ON, so an item it could drop
            // is an item this case knows about.
            let whole = walk(&outposts, onto, row.member, &case, WHOLE_LIST).await;
            assert!(
                whole.len() > 1,
                "case {case:?}: the list replayed on must hold more than one item for a \
                 dropped item to show up, got {whole:?}"
            );
            // Its OWN tokens resume its own list: one item at a time, the pages a
            // caller walks are exactly that list, in that order.
            let paged = walk(&outposts, onto, row.member, &case, 1).await;
            assert_eq!(
                paged, whole,
                "case {case:?}: paging a list with its own tokens must yield it once, in \
                 order"
            );

            // The token the first page of the OTHER parent's list issues.
            let first = outposts
                .ok("GET", &format!("{from}?MaxResults=1"), None)
                .await;
            let token = text(&first, "NextToken");
            assert!(
                !token.is_empty(),
                "case {case:?}: a list of more than one item served one at a time issues a \
                 NextToken, got {first}"
            );

            // Replayed on this list, it is refused — not read as this list's boundary.
            let replayed = outposts
                .call(
                    "GET",
                    &format!(
                        "{onto}?MaxResults={WHOLE_LIST}&NextToken={}",
                        query_value(&token)
                    ),
                    None,
                )
                .await;
            assert_eq!(
                (replayed.status, replayed.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected the refusal of a token this list never issued, not \
                 a page of it, got {} {} {}",
                replayed.status,
                replayed.error_code,
                replayed.body
            );
            let expected = page_token_report(row.operation);
            assert_eq!(
                replayed.body.get(MODELLED_MESSAGE).and_then(Value::as_str),
                Some(expected.as_str()),
                "case {case:?}: expected the refusal to name {}, got {}",
                row.operation,
                replayed.body
            );
        }
    }
}

/// One row of rule 2: the form factor the Outpost's hardware is installed from, the form
/// factors named for it afterwards — each installing the configuration it names — and
/// whether that leaves the Outpost holding an installation other than the one the token
/// was cut from.
#[derive(Debug)]
struct Install {
    case: &'static str,
    installed: &'static str,
    then_named: &'static [&'static str],
    replaced: bool,
}

/// Rule 2: a boundary cut from one installation is a boundary AT A UNIT, so the page a
/// caller resumes from it after the Outpost's hardware was replaced holds every unit now
/// installed.
///
/// The rows differ along what the rule generalises over: a replacement that installs
/// FEWER units than the token's installation held (a rack's hardware replaced by a
/// server's), one that installs none at all (the form factor the Outpost already
/// carries, which replaces nothing, so the token goes on resuming the hardware it was
/// cut from), and two replacements in a row, the last of which installs MORE units than
/// the token's installation held.
#[tokio::test]
async fn a_boundary_cut_from_one_installation_drops_no_unit_of_the_next() {
    let rows = &[
        Install {
            case: "a rack's hardware replaced by a server's",
            installed: "RACK",
            then_named: &["SERVER"],
            replaced: true,
        },
        Install {
            case: "a rack given the form factor it already carries",
            installed: "RACK",
            then_named: &["RACK"],
            replaced: false,
        },
        Install {
            case: "a rack replaced by a server's hardware and then by a rack's again",
            installed: "RACK",
            then_named: &["SERVER", "RACK"],
            replaced: true,
        },
    ];

    let outposts = Outposts::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let outpost_id = an_outpost(&outposts, &format!("install-{index}"), row.installed).await;
        install_hardware(&outposts, &outpost_id).await;
        let path = assets_route(&outpost_id);

        // The first page of the hardware that order installed, and the token it issues.
        let first = outposts
            .ok("GET", &format!("{path}?MaxResults=1"), None)
            .await;
        assert_eq!(
            reported(&first, "Assets", case).len(),
            1,
            "case {case:?}: a page of one holds one unit, got {first}"
        );
        let token = text(&first, "NextToken");
        assert!(
            !token.is_empty(),
            "case {case:?}: an installation of more than one unit served one at a time \
             issues a NextToken, got {first}"
        );

        // The installs the newly named form factors owe, each materialised before the
        // next one is named.
        for form_factor in row.then_named {
            outposts
                .ok(
                    "PATCH",
                    &format!("/outposts/{}", segment(&outpost_id)),
                    Some(&json!({ "SupportedHardwareType": form_factor })),
                )
                .await;
            settle();
            outposts
                .ok("GET", &format!("{path}?MaxResults={WHOLE_LIST}"), None)
                .await;
        }

        // The hardware the Outpost holds now, and what the stale token has to resume:
        // every unit created after the one it names, which for a fresh installation is
        // all of it and for the untouched one is everything after that unit.
        let held = outposts
            .ok("GET", &format!("{path}?MaxResults={WHOLE_LIST}"), None)
            .await;
        let current = reported(&held, "Assets", case);
        assert!(
            !current.is_empty(),
            "case {case:?}: the Outpost must hold hardware to resume into, got {held}"
        );
        let expected: Vec<Value> = match row.replaced {
            true => current.clone(),
            false => current[1..].to_vec(),
        };

        let resumed = outposts
            .call(
                "GET",
                &format!(
                    "{path}?MaxResults={WHOLE_LIST}&NextToken={}",
                    query_value(&token)
                ),
                None,
            )
            .await;
        assert_eq!(
            resumed.status,
            200,
            "case {case:?}: a token of this Outpost's own asset list is served, got {} {}",
            resumed.status,
            resumed.body
        );
        assert_eq!(
            reported(&resumed.body, "Assets", case),
            expected,
            "case {case:?}: expected the page to resume after the unit the token names \
             and drop none of the {} units installed, got {}",
            current.len(),
            resumed.body
        );
    }
}
