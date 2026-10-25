// SPDX-License-Identifier: CC0-1.0

//! A packaging configuration is its manifests, so a package block that configures none
//! is refused.
//!
//! The rule under test, stated once: every packaging format's manifest list is a
//! `required` list of manifest configurations
//! (`smithy_query(query="shape",shape="HlsPackage,DashPackage,CmafPackage,MssPackage")`
//! — `HlsPackage$HlsManifests`, `DashPackage$DashManifests`, `CmafPackage$HlsManifests`
//! and `MssPackage$MssManifests` each `required`), so a `CreatePackagingConfiguration`
//! whose package block carries no manifest configuration describes no output manifest
//! and is refused with `UnprocessableEntityException` (HTTP 422) attributed to that
//! list's own wire path, storing nothing; a block carrying one or more manifests is
//! accepted and reads back with exactly the manifests it was created with.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! packaging format (all four, so the refusal is not keyed to one block), the FORM the
//! manifest-less block arrives in (an empty array, an empty array beside the block's
//! other settings, and the required member omitted — which the boundary validator
//! refuses as a missing required member before a handler sees it) and — on the
//! accepting side — how MANY manifests the block carries and whether each names a
//! `ManifestName`.
//!
//! The refusal's consequence is asserted too, because the defect this pins was not only
//! a missing refusal: a stored manifest-less configuration made every asset in the
//! group derive an `EgressEndpoint` ("The URL of the parent manifest for the repackaged
//! Asset") for a manifest the caller never configured. So each refused row also asserts
//! that the id does not exist afterwards and that an asset in the same group derives no
//! endpoint for it.
//!
//! Every case drives real HTTP through the router the production binary builds. The
//! harness is the one `tests/all/packaging_lifecycle.rs` defines for this service.

use serde_json::{json, Value};

use service_com_amazonaws_mediapackagevod_mediapackagevod::state as vod;

use crate::packaging_lifecycle::{call, call_raw, create_asset, create_group, router};

/// The modelled refusal of an invalid request parameter: status and error code
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")` — `errorKind
/// client`, `httpError 422`).
const UNPROCESSABLE: (u16, &str) = (422, "UnprocessableEntityException");

/// The modelled refusal of a resource this service does not hold
/// (`smithy_query(query="shape",shape="NotFoundException")` — `httpError 404`).
const NOT_FOUND: (u16, &str) = (404, "NotFoundException");

/// How the request walk's boundary validator refuses a request that omits a `required`
/// member: it never reaches a handler, so this is not the service's own refusal.
const BOUNDARY_REFUSAL: (u16, &str) = (400, "ValidationException");

/// The success status of `CreatePackagingConfiguration`, as the handler documents it.
const ACCEPTED: u16 = 202;

/// One packaging format of `CreatePackagingConfiguration`: the wire name of its package
/// block, the wire name of the `required` manifest list inside it, how the refusal's
/// message spells that list's path, and the parent-manifest file extension that
/// format's `EgressEndpoint$Url` carries.
///
/// The first three columns come from the model (the `jsonName`s of the four package
/// blocks and of their manifest lists); the extension is the manifest kind each format
/// publishes (HLS and CMAF an HLS playlist, DASH an MPD, MSS a server manifest).
struct Format {
    block: &'static str,
    list: &'static str,
    message_path: &'static str,
    suffix: &'static str,
}

/// Every packaging format, so no row can leave one of the four lists untested.
const FORMATS: [Format; 4] = [
    Format {
        block: "hlsPackage",
        list: "hlsManifests",
        message_path: "hls_package.hls_manifests",
        suffix: "m3u8",
    },
    Format {
        block: "dashPackage",
        list: "dashManifests",
        message_path: "dash_package.dash_manifests",
        suffix: "mpd",
    },
    Format {
        block: "cmafPackage",
        list: "hlsManifests",
        message_path: "cmaf_package.hls_manifests",
        suffix: "m3u8",
    },
    Format {
        block: "mssPackage",
        list: "mssManifests",
        message_path: "mss_package.mss_manifests",
        suffix: "ism/Manifest",
    },
];

impl Format {
    /// The wire path the refusal attributes itself to: the list's name under the block
    /// it arrived in.
    fn member(&self) -> String {
        format!("{}.{}", self.block, self.list)
    }

    /// A `CreatePackagingConfiguration` body for `id` in `group` carrying this format's
    /// block with `members`.
    fn body(&self, id: &str, group: &str, members: Value) -> Value {
        json!({
            "id": id,
            "packagingGroupId": group,
            self.block: members,
        })
    }

    /// Settings of this format's block OTHER than its manifest list, so a row can show
    /// that an otherwise well-formed block is still refused for its manifests alone.
    /// Each member is one the model declares on that block.
    fn other_members(&self) -> Value {
        match self.block {
            "hlsPackage" => json!({ "segmentDurationSeconds": 6, "includeDvbSubtitles": true }),
            "dashPackage" => json!({ "segmentDurationSeconds": 4, "includeIframeOnlyStream": true }),
            "cmafPackage" => {
                json!({ "segmentDurationSeconds": 8, "includeEncoderConfigurationInSegments": true })
            }
            _ => json!({ "segmentDurationSeconds": 2 }),
        }
    }
}

/// Which refusal a manifest-less row must get: the service's own, or the boundary
/// validator's refusal of a missing `required` member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The service judged the list it was sent: HTTP 422 attributed to the list.
    Service,
    /// The request never carried the list, so the request walk refused it first.
    Boundary,
}

/// The forms a block carrying no manifest configuration arrives in, for one format:
/// (case, the block's members, the refusal it must get).
///
/// A client writes the empty list and the omitted list both, so both are rows; they are
/// refused at different points, and NEITHER stores a configuration.
fn manifest_less_blocks(format: &Format) -> Vec<(String, Value, Refusal)> {
    let mut with_others = format.other_members();
    with_others[format.list] = json!([]);
    vec![
        (
            format!("{} carrying an empty {}", format.block, format.list),
            json!({ format.list: [] }),
            Refusal::Service,
        ),
        (
            format!(
                "{} carrying an empty {} beside its other settings",
                format.block, format.list
            ),
            with_others,
            Refusal::Service,
        ),
        (
            format!("{} omitting {} entirely", format.block, format.list),
            json!({}),
            Refusal::Boundary,
        ),
    ]
}

/// The manifest lists a block IS accepted with, for one format: (case, the manifests as
/// sent, the `ManifestName` each entry resolves to).
///
/// Rows differ in COUNT (one, two, three) and in whether an entry names a
/// `ManifestName` — "an optional string to include in the name of the manifest", so an
/// entry that names none resolves to [`vod::DEFAULT_MANIFEST_NAME`].
fn accepted_manifests() -> Vec<(String, Value, Vec<String>)> {
    let default_name = vod::DEFAULT_MANIFEST_NAME.to_string();
    vec![
        (
            "one manifest naming nothing".to_string(),
            json!([{}]),
            vec![default_name.clone()],
        ),
        (
            "one named manifest".to_string(),
            json!([{ "manifestName": "primary" }]),
            vec!["primary".to_string()],
        ),
        (
            "two named manifests".to_string(),
            json!([{ "manifestName": "primary" }, { "manifestName": "backup" }]),
            vec!["primary".to_string(), "backup".to_string()],
        ),
        (
            "three manifests, the first naming nothing".to_string(),
            json!([{}, { "manifestName": "second" }, { "manifestName": "third" }]),
            vec![default_name, "second".to_string(), "third".to_string()],
        ),
    ]
}

/// The error shape `x-amzn-errortype` names, which is how restJson1 resolves one.
fn errortype(response: &http::Response<bytes::Bytes>) -> String {
    response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<unset>")
        .to_string()
}

/// The `packagingConfigurationId` of every endpoint an asset reports.
fn endpoint_config_ids(asset: &Value) -> Vec<String> {
    asset["egressEndpoints"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry["packagingConfigurationId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A create whose package block configures no manifest is refused with the modelled
/// 422 attributed to that block's manifest list, and stores nothing: the id reads back
/// as not found and an asset in the same group derives no endpoint for it.
#[tokio::test]
async fn a_package_block_configuring_no_manifest_is_refused_and_stores_nothing() {
    let router = router();
    create_group(&router, "g", None).await;

    let mut refused_ids: Vec<String> = Vec::new();
    for format in &FORMATS {
        for (index, (case, members, refusal)) in
            manifest_less_blocks(format).into_iter().enumerate()
        {
            let id = format!("cfg-{}-{index}", format.block);
            let response = call_raw(
                &router,
                "POST",
                "/packaging_configurations",
                Some(format.body(&id, "g", members)),
            )
            .await;
            let code = errortype(&response);
            let status = response.status().as_u16();
            let body: Value =
                serde_json::from_slice(&response.into_body()).unwrap_or(Value::Null);
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("<unset>")
                .to_string();
            match refusal {
                Refusal::Service => {
                    assert_eq!(
                        (status, code.as_str()),
                        UNPROCESSABLE,
                        "case {case:?}: expected {UNPROCESSABLE:?}, got {status} {code:?} \
                         body {body}"
                    );
                    assert_eq!(
                        body.get("errors").cloned().unwrap_or(Value::Null),
                        json!({ format.member(): [vod::MANIFEST_LIST_REQUIREMENT] }),
                        "case {case:?}: the refusal must attribute itself to the manifest \
                         list, got {}",
                        body.get("errors").cloned().unwrap_or(Value::Null)
                    );
                    assert_eq!(
                        message,
                        format!("{} {}", format.message_path, vod::MANIFEST_LIST_REQUIREMENT),
                        "case {case:?}: expected the member-and-constraint message, got {body}"
                    );
                }
                Refusal::Boundary => {
                    assert_eq!(
                        (status, code.as_str()),
                        BOUNDARY_REFUSAL,
                        "case {case:?}: expected {BOUNDARY_REFUSAL:?}, got {status} {code:?} \
                         body {body}"
                    );
                    assert!(
                        message.contains(&format.member()),
                        "case {case:?}: the refusal must name the missing list, got {message:?}"
                    );
                }
            }

            // Nothing was stored: the id the refused create named does not exist.
            let described = call_raw(
                &router,
                "GET",
                &format!("/packaging_configurations/{id}"),
                None,
            )
            .await;
            let code = errortype(&described);
            let status = described.status().as_u16();
            assert_eq!(
                (status, code.as_str()),
                NOT_FOUND,
                "case {case:?}: after the refusal {id:?} must not exist, got {status} {code:?}"
            );
            refused_ids.push(id);
        }
    }

    // No listing reports a refused configuration either.
    let (status, listed) = call(&router, "GET", "/packaging_configurations", None).await;
    assert_eq!(status, 200, "ListPackagingConfigurations: body {listed}");
    let listed_ids: Vec<String> = listed["packagingConfigurations"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        listed_ids.is_empty(),
        "every create was refused, so the group holds no configuration, got {listed_ids:?}"
    );

    // And an asset in the group derives no egress endpoint, so no manifest name was
    // invented for a configuration that configured none.
    create_asset(&router, "a1", "g").await;
    let (status, asset) = call(&router, "GET", "/assets/a1", None).await;
    assert_eq!(status, 200, "DescribeAsset a1: body {asset}");
    let endpoints = endpoint_config_ids(&asset);
    assert!(
        endpoints.is_empty(),
        "a refused configuration must derive no endpoint, got {endpoints:?} for {refused_ids:?}"
    );
}

/// A block carrying one or more manifests is accepted, however many it carries, and
/// round-trips: `DescribePackagingConfiguration` reports the same manifests in the same
/// order, and an asset in the group derives one endpoint whose parent-manifest URL
/// names the configuration's first manifest and that format's manifest kind.
#[tokio::test]
async fn a_block_carrying_manifests_is_accepted_and_round_trips() {
    let router = router();

    for format in &FORMATS {
        for (index, (case, manifests, names)) in accepted_manifests().into_iter().enumerate() {
            let case = format!("{}/{case}", format.block);
            let group = format!("group-{}-{index}", format.block);
            let id = format!("cfg-{}-{index}", format.block);
            create_group(&router, &group, None).await;

            let (status, created) = call(
                &router,
                "POST",
                "/packaging_configurations",
                Some(format.body(&id, &group, json!({ format.list: manifests.clone() }))),
            )
            .await;
            assert_eq!(
                status, ACCEPTED,
                "case {case:?}: a block carrying {} manifests must be accepted, got {status} \
                 body {created}",
                names.len()
            );

            // Read back through the describe: the stored manifests are the ones sent.
            let (status, described) = call(
                &router,
                "GET",
                &format!("/packaging_configurations/{id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the configuration must read back, got {status} body {described}"
            );
            let stored = described[format.block][format.list]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                stored.len(),
                names.len(),
                "case {case:?}: expected {} stored manifests, got {:?}",
                names.len(),
                stored
            );
            let stored_names: Vec<String> = stored
                .iter()
                .map(|manifest| {
                    manifest["manifestName"]
                        .as_str()
                        .unwrap_or("<unset>")
                        .to_string()
                })
                .collect();
            let sent_names: Vec<String> = manifests
                .as_array()
                .into_iter()
                .flatten()
                .map(|manifest| {
                    manifest["manifestName"]
                        .as_str()
                        .unwrap_or("<unset>")
                        .to_string()
                })
                .collect();
            assert_eq!(
                stored_names, sent_names,
                "case {case:?}: the stored manifests must be the ones sent, got {stored_names:?}"
            );

            // The derived endpoint points at the parent manifest of the FIRST manifest
            // this configuration configures — a name the request supplied, or the
            // default for an entry that named none.
            let asset_id = format!("asset-{}-{index}", format.block);
            create_asset(&router, &asset_id, &group).await;
            let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeAsset must succeed, got {status} body {asset}"
            );
            assert_eq!(
                endpoint_config_ids(&asset),
                vec![id.clone()],
                "case {case:?}: the asset must derive exactly this configuration's endpoint, \
                 got {:?}",
                endpoint_config_ids(&asset)
            );
            let url = asset["egressEndpoints"][0]["url"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let expected_tail = format!("/{}.{}", names[0], format.suffix);
            assert!(
                url.ends_with(&expected_tail),
                "case {case:?}: the parent-manifest URL must end in {expected_tail:?}, got {url:?}"
            );
        }
    }
}
