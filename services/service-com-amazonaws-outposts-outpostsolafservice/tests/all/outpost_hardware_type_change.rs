// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for recording an Outpost's hardware type AFTER it was created,
//! driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! [`crate::outpost_hardware_installation`] states where an Outpost's hardware comes
//! from for the form factor its CREATE named; this suite states the same rule for the
//! form factor an update names, which only state that pairs the two can satisfy:
//!
//! 1. `SupportedHardwareType` names the hardware configuration Amazon Web Services
//!    installs on the Outpost, so the form factor an Outpost reports is always the one
//!    whose configuration it holds: recording a form factor other than the one it
//!    carries installs that form factor's configuration, and recording the one it
//!    already carries installs nothing again. An Outpost given a form factor by an
//!    update is therefore indistinguishable, hardware and all, from one created with
//!    it.
//! 2. Installing hardware takes the span installing an Outpost's hardware takes, so
//!    the install an update owes is owed at an instant rather than applied on the next
//!    read: until it falls due the Outpost holds no asset, which is what an Outpost
//!    whose hardware is being installed holds.
//! 3. Hardware installed by an update is hardware like any other: its `AssetId`s are
//!    the ones `GetOutpostSupportedInstanceTypes` and `StartCapacityTask` take.
//! 4. Only a form factor the modelled enum declares names a configuration, so a value
//!    outside it is refused as a bad request and installs nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::capacity_task_lifecycle::capacity_route;
use crate::site_and_outpost_lifecycle::{query_value, segment, settle, text, Outposts};

/// Both values the modelled `SupportedHardwareType` enum declares
/// (`smithy_query(query="shape",shape="SupportedHardwareType")`) — the form factors
/// `CreateOutpost` and `UpdateOutpost` can name.
const HARDWARE_TYPES: &[&str] = &["RACK", "SERVER"];

/// The modelled `AssetType` of the compute hardware that hosts instances, the only
/// kind of unit the model carries `ComputeAttributes` for.
const ASSET_TYPE_COMPUTE: &str = "COMPUTE";

/// One site and one Outpost on it carrying `form_factor` (or none at all), as that
/// Outpost's `OutpostId`. An Outpost belongs to a site, so the site comes first.
async fn an_outpost(outposts: &Outposts, name: &str, form_factor: Option<&str>) -> String {
    let site = outposts
        .create_site(&json!({ "Name": format!("{name}-site") }))
        .await;
    let mut request = json!({ "Name": name, "SiteId": text(&site, "SiteId") });
    if let Some(form_factor) = form_factor {
        request["SupportedHardwareType"] = json!(form_factor);
    }
    let outpost = outposts.create_outpost(&request).await;
    text(&outpost, "OutpostId")
}

/// `UpdateOutpost` naming `form_factor`, requiring HTTP 200 and that the answer
/// reports the form factor the request named.
async fn name_hardware_type(outposts: &Outposts, outpost_id: &str, form_factor: &str, case: &str) {
    let patched = outposts
        .ok(
            "PATCH",
            &format!("/outposts/{}", segment(outpost_id)),
            Some(&json!({ "SupportedHardwareType": form_factor })),
        )
        .await;
    let reported = text(&patched["Outpost"], "SupportedHardwareType");
    assert_eq!(
        reported, form_factor,
        "case {case:?}: the update must answer the form factor it recorded, expected \
         {form_factor:?}, got {reported:?}"
    );
}

/// The hardware one Outpost holds, in installation order, as `ListAssets` reports it.
async fn installed(outposts: &Outposts, outpost_id: &str) -> Vec<Value> {
    outposts
        .ok(
            "GET",
            &format!("/outposts/{}/assets", segment(outpost_id)),
            None,
        )
        .await["Assets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// What one installed unit IS, with the identifiers derived from the Outpost's own
/// creation order reduced to whether the unit reports one at all.
///
/// `AssetId`, `RackId` and `ComputeAttributes.HostId` identify the unit within the one
/// Outpost that holds it, so no two Outposts ever report the same values for them —
/// which is exactly what makes them the members to drop when comparing the
/// configuration of one Outpost with the configuration of another.
fn unit(asset: &Value) -> Value {
    let mut compute = asset.get("ComputeAttributes").cloned();
    if let Some(Value::Object(attributes)) = compute.as_mut() {
        let hosted = attributes.remove("HostId").is_some();
        attributes.insert("HostsADedicatedHost".to_string(), json!(hosted));
    }
    json!({
        "AssetType": asset.get("AssetType"),
        "Identified": asset.get("AssetId").is_some(),
        "Racked": asset.get("RackId").is_some(),
        "AssetLocation": asset.get("AssetLocation"),
        "ComputeAttributes": compute,
    })
}

/// The configuration a list of installed units is, unit by unit. See [`unit`].
fn configuration(assets: &[Value]) -> Vec<Value> {
    assets.iter().map(unit).collect()
}

/// The `InstanceType`s `GetOutpostSupportedInstanceTypes` answers for one Outpost,
/// under the query string `query`.
async fn supported_types(outposts: &Outposts, outpost_id: &str, query: &str) -> Vec<String> {
    outposts
        .ok(
            "GET",
            &format!(
                "/outposts/{}/supportedInstanceTypes?MaxResults=1000{query}",
                segment(outpost_id)
            ),
            None,
        )
        .await["InstanceTypes"]
        .as_array()
        .map(|items| items.iter().map(|item| text(item, "InstanceType")).collect())
        .unwrap_or_default()
}

/// One row of rule 1: the form factor the Outpost is created with, the one the update
/// then names for it, and whether that update lands while the Outpost is still
/// provisioning or after it has settled holding its create-side hardware.
#[derive(Debug)]
struct Row {
    case: &'static str,
    created: Option<&'static str>,
    updated: &'static str,
    while_provisioning: bool,
}

/// Rules 1 and 2: over every pair of form factors an Outpost can be created with and
/// then given, what it holds afterwards.
///
/// The rows differ along the form factor the create recorded (none, each of the two the
/// enum declares), the form factor the update names (each of the two, including the one
/// the Outpost already carries), and when the update lands relative to the install the
/// create owes.
#[tokio::test]
async fn the_hardware_an_outpost_holds_is_the_configuration_its_form_factor_names() {
    let rows = [
        Row {
            case: "no form factor at create, a rack named afterwards",
            created: None,
            updated: "RACK",
            while_provisioning: false,
        },
        Row {
            case: "no form factor at create, a server named afterwards",
            created: None,
            updated: "SERVER",
            while_provisioning: false,
        },
        Row {
            case: "a settled rack given the server form factor",
            created: Some("RACK"),
            updated: "SERVER",
            while_provisioning: false,
        },
        Row {
            case: "a settled server given the rack form factor",
            created: Some("SERVER"),
            updated: "RACK",
            while_provisioning: false,
        },
        Row {
            case: "a settled rack given the rack form factor again",
            created: Some("RACK"),
            updated: "RACK",
            while_provisioning: false,
        },
        Row {
            case: "a settled server given the server form factor again",
            created: Some("SERVER"),
            updated: "SERVER",
            while_provisioning: false,
        },
        Row {
            case: "no form factor at create, a rack named while still provisioning",
            created: None,
            updated: "RACK",
            while_provisioning: true,
        },
        Row {
            case: "a provisioning rack given the server form factor",
            created: Some("RACK"),
            updated: "SERVER",
            while_provisioning: true,
        },
    ];

    let outposts = Outposts::new();
    // One Outpost per form factor created WITH it: the configuration the create-side
    // install places is what each row's Outpost must end up holding, read from the
    // emulator rather than spelled out here.
    let mut reference: BTreeMap<&str, String> = BTreeMap::new();
    for form_factor in HARDWARE_TYPES {
        reference.insert(
            form_factor,
            an_outpost(
                &outposts,
                &format!("reference-{form_factor}"),
                Some(form_factor),
            )
            .await,
        );
    }

    let mut created = Vec::new();
    for row in &rows {
        created.push(an_outpost(&outposts, row.case, row.created).await);
    }
    // The rows whose update lands DURING provisioning name their form factor before
    // the install the create owes has fallen due.
    for (row, outpost_id) in rows.iter().zip(&created) {
        if row.while_provisioning {
            name_hardware_type(&outposts, outpost_id, row.updated, row.case).await;
        }
    }

    settle();

    // Every install owed so far has run, so a row that patches a SETTLED Outpost now
    // patches one holding the hardware of the form factor its create recorded.
    let mut before: Vec<Option<Vec<Value>>> = Vec::new();
    for (row, outpost_id) in rows.iter().zip(&created) {
        before.push(match row.while_provisioning {
            true => None,
            false => Some(installed(&outposts, outpost_id).await),
        });
    }
    for ((row, outpost_id), before) in rows.iter().zip(&created).zip(&before) {
        if row.while_provisioning {
            continue;
        }
        let case = row.case;
        name_hardware_type(&outposts, outpost_id, row.updated, case).await;
        // Rule 2: installing the hardware a newly named form factor names takes the
        // span it takes, so right after the call the Outpost holds no asset — unless
        // the update named the form factor it already carried, where nothing is
        // installed again and the hardware it holds is untouched.
        let straight_after = installed(&outposts, outpost_id).await;
        let unchanged = row.created == Some(row.updated);
        match unchanged {
            true => assert_eq!(
                Some(&straight_after),
                before.as_ref(),
                "case {case:?}: naming the form factor the Outpost already carries installs \
                 nothing again, got {straight_after:?}"
            ),
            false => assert!(
                straight_after.is_empty(),
                "case {case:?}: an Outpost whose hardware is being installed holds no asset \
                 until that install falls due, got {straight_after:?}"
            ),
        }
    }

    settle();

    for ((row, outpost_id), before) in rows.iter().zip(&created).zip(&before) {
        let case = row.case;
        let outpost = outposts.get_outpost(outpost_id).await;
        let reported = text(&outpost, "SupportedHardwareType");
        assert_eq!(
            reported, row.updated,
            "case {case:?}: the Outpost must report the form factor the update recorded, \
             expected {:?}, got {reported:?}",
            row.updated
        );

        let held = installed(&outposts, outpost_id).await;
        let wanted = installed(&outposts, &reference[row.updated]).await;
        assert!(
            !wanted.is_empty(),
            "case {case:?}: an Outpost created with {:?} must hold that form factor's \
             configuration for this row to compare against, got {wanted:?}",
            row.updated
        );
        // Rule 1: the hardware is the configuration the REPORTED form factor names, so
        // it is the hardware of an Outpost created with that form factor.
        assert_eq!(
            configuration(&held),
            configuration(&wanted),
            "case {case:?}: an Outpost reporting {reported:?} holds that form factor's \
             configuration, got {held:?}"
        );
        // A row that replaced one configuration with another holds neither the old
        // units nor none at all.
        if let (Some(before), false) = (before, row.created == Some(row.updated)) {
            if !before.is_empty() {
                assert_ne!(
                    configuration(&held),
                    configuration(before),
                    "case {case:?}: the configuration the previous form factor named must not \
                     still be installed, got {held:?}"
                );
            }
        }
    }
}

/// The boundary of rule 1: only a form factor the modelled enum declares names a
/// configuration, so a value outside it is refused as a bad request and the Outpost
/// keeps both the form factor and the hardware it had.
///
/// The rows differ along how the value misses the enum: a word it does not declare,
/// the lower-cased form of one it does, and the empty string — which carries no value
/// at all for a member, as [`crate::update_persistence`] states for the other update
/// members.
#[tokio::test]
async fn a_form_factor_outside_the_modelled_enum_installs_nothing() {
    let outposts = Outposts::new();
    let outpost_id = an_outpost(&outposts, "unmodelled-form-factor", Some("RACK")).await;

    settle();

    let before = installed(&outposts, &outpost_id).await;
    assert!(
        !before.is_empty(),
        "the Outpost must hold its create-side configuration before the rows run, got \
         {before:?}"
    );

    for value in ["BANANA", "rack", ""] {
        let case = &format!("the form factor {value:?}");
        let reply = outposts
            .call(
                "PATCH",
                &format!("/outposts/{}", segment(&outpost_id)),
                Some(&json!({ "SupportedHardwareType": value })),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a form factor outside the modelled enum is a bad request, got {} \
             {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let outpost = outposts.get_outpost(&outpost_id).await;
        let reported = text(&outpost, "SupportedHardwareType");
        assert_eq!(
            reported, "RACK",
            "case {case:?}: the refused update must leave the recorded form factor alone, got \
             {reported:?}"
        );
        let held = installed(&outposts, &outpost_id).await;
        assert_eq!(
            held, before,
            "case {case:?}: the refused update must leave the installed hardware alone, got \
             {held:?}"
        );
    }
}

/// Rule 3: hardware installed by an update is nameable wherever the API takes an asset
/// of the Outpost — `GetOutpostSupportedInstanceTypes` scoped to it and
/// `StartCapacityTask` for it.
///
/// The rows differ along the form factor the create recorded and the one the update
/// named, so each ends up with a configuration this service installed for a form factor
/// the create never saw.
#[tokio::test]
async fn an_asset_installed_by_an_update_is_usable_where_the_api_takes_one() {
    let rows: &[(&str, Option<&str>, &str)] = &[
        ("a rack named after the create", None, "RACK"),
        ("a server named after the create", None, "SERVER"),
        ("a rack replaced by a server", Some("RACK"), "SERVER"),
        ("a server replaced by a rack", Some("SERVER"), "RACK"),
    ];

    let outposts = Outposts::new();
    let mut created = Vec::new();
    for (case, form_factor, _) in rows {
        created.push(an_outpost(&outposts, case, *form_factor).await);
    }

    settle();

    for ((case, _, updated), outpost_id) in rows.iter().zip(&created) {
        name_hardware_type(&outposts, outpost_id, updated, case).await;
    }

    settle();

    for ((case, _, updated), outpost_id) in rows.iter().zip(&created) {
        let held = installed(&outposts, outpost_id).await;
        assert!(
            !held.is_empty(),
            "case {case:?}: the Outpost must hold the hardware {updated:?} names, got {held:?}"
        );
        // What the Outpost as a whole supports: what one of its compute units supports
        // is what that unit's own hardware admits of it.
        let whole = supported_types(&outposts, outpost_id, "").await;
        assert!(
            !whole.is_empty(),
            "case {case:?}: an Outpost holding compute hardware supports instance types, got \
             {whole:?}"
        );

        for asset in &held {
            let asset_id = text(asset, "AssetId");
            let asset_type = text(asset, "AssetType");
            let case = &format!("{case} ({asset_type} {asset_id})");
            let scoped = supported_types(
                &outposts,
                outpost_id,
                &format!("&AssetId={}", query_value(&asset_id)),
            )
            .await;
            match asset_type == ASSET_TYPE_COMPUTE {
                true => assert_eq!(
                    scoped, whole,
                    "case {case:?}: a compute unit runs what its Outpost's hardware runs, got \
                     {scoped:?}"
                ),
                false => assert!(
                    scoped.is_empty(),
                    "case {case:?}: a unit that hosts no instance supports no instance type, \
                     got {scoped:?}"
                ),
            }
        }

        // One task at a time runs on an Outpost, so one compute unit of each row is
        // named, with a pool of an instance type that Outpost supports.
        let host = held
            .iter()
            .find(|asset| text(asset, "AssetType") == ASSET_TYPE_COMPUTE)
            .map(|asset| text(asset, "AssetId"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the configuration {updated:?} names holds a compute unit")
            });
        let started = outposts
            .ok(
                "POST",
                &capacity_route(outpost_id),
                Some(&json!({
                    "InstancePools": [{ "InstanceType": whole[0], "Count": 1 }],
                    "AssetId": host,
                })),
            )
            .await;
        let named = text(&started, "AssetId");
        assert_eq!(
            named, host,
            "case {case:?}: the task must be for the asset the request named, expected \
             {host:?}, got {named:?}"
        );
    }
}
