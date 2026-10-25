// SPDX-License-Identifier: CC0-1.0

//! The ARN-naming rule of this service, and the ARN -> resource resolution the
//! `/tags/{ResourceArn}` operations depend on.
//!
//! The rule under test: a resource that exists only inside a global network — a site, a
//! link, a device — is NAMED under that global network, so its ARN reads
//! `<kind>/<globalNetworkId>/<id>`, while a resource identified account-wide (a global
//! network, a core network) reads `<kind>/<id>`. The resource's own id is the last path
//! segment either way, so the ARN a create mints is exactly the ARN every reader reports
//! and the key `TagResource` / `UntagResource` / `ListTagsForResource` resolve back to the
//! stored resource.
//!
//! Every call goes through the real `MultiServiceRouter` and nothing reaches into
//! `crate::state`: a resource is only ever created and read through an operation.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, expected_arn, expected_scoped_arn, Emulator};
use crate::resource_tagging::{tag, tags_uri, untag_uri};

/// The reader that reports a minted resource back, and the members to read it from.
struct Reader {
    uri: String,
    list: &'static str,
    id_member: &'static str,
    arn_member: &'static str,
}

/// One minted resource: what it is, the ARN the create answered, and the global network
/// the ARN has to name ahead of its id (`None` for a kind identified account-wide).
struct Minted {
    case: &'static str,
    kind: &'static str,
    id: String,
    arn: String,
    scope: Option<String>,
    reader: Option<Reader>,
}

impl Minted {
    /// The ARN this resource must carry, built from its kind, its scope and its id.
    fn expected(&self) -> String {
        match &self.scope {
            Some(scope) => expected_scoped_arn(self.kind, scope, &self.id),
            None => expected_arn(self.kind, &self.id),
        }
    }
}

/// The string member `member` of `value`, or a panic naming what was there instead.
fn string_member(value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string {member} in {value}"))
        .to_string()
}

/// Create a global network, a site, a link and a device in it, plus a core network, and
/// report each as a row of the table.
///
/// `label` distinguishes the rows of one call from another's, and every row of one call
/// shares the one global network, so a row's scope is a value the test did not choose.
async fn mint_in_one_global_network(emulator: &Emulator, label: &'static str) -> Vec<Minted> {
    let (status, created) = emulator
        .post("/global-networks", json!({ "Description": label }))
        .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateGlobalNetwork expected 200, got {status} with {created}"
    );
    let network = string_member(&created["GlobalNetwork"], "GlobalNetworkId");
    let mut rows = vec![Minted {
        case: label,
        kind: "global-network",
        id: network.clone(),
        arn: string_member(&created["GlobalNetwork"], "GlobalNetworkArn"),
        scope: None,
        reader: Some(Reader {
            uri: "/global-networks".to_string(),
            list: "GlobalNetworks",
            id_member: "GlobalNetworkId",
            arn_member: "GlobalNetworkArn",
        }),
    }];

    let (status, created) = emulator
        .post(
            &format!("/global-networks/{network}/sites"),
            json!({ "Description": label }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateSite expected 200, got {status} with {created}"
    );
    let site = string_member(&created["Site"], "SiteId");
    rows.push(Minted {
        case: label,
        kind: "site",
        id: site.clone(),
        arn: string_member(&created["Site"], "SiteArn"),
        scope: Some(network.clone()),
        reader: Some(Reader {
            uri: format!("/global-networks/{network}/sites"),
            list: "Sites",
            id_member: "SiteId",
            arn_member: "SiteArn",
        }),
    });

    let (status, created) = emulator
        .post(
            &format!("/global-networks/{network}/links"),
            json!({
                "SiteId": site,
                "Bandwidth": { "UploadSpeed": 10, "DownloadSpeed": 20 },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateLink expected 200, got {status} with {created}"
    );
    rows.push(Minted {
        case: label,
        kind: "link",
        id: string_member(&created["Link"], "LinkId"),
        arn: string_member(&created["Link"], "LinkArn"),
        scope: Some(network.clone()),
        reader: Some(Reader {
            uri: format!("/global-networks/{network}/links"),
            list: "Links",
            id_member: "LinkId",
            arn_member: "LinkArn",
        }),
    });

    let (status, created) = emulator
        .post(
            &format!("/global-networks/{network}/devices"),
            json!({ "SiteId": site, "Type": "router" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateDevice expected 200, got {status} with {created}"
    );
    rows.push(Minted {
        case: label,
        kind: "device",
        id: string_member(&created["Device"], "DeviceId"),
        arn: string_member(&created["Device"], "DeviceArn"),
        scope: Some(network.clone()),
        reader: Some(Reader {
            uri: format!("/global-networks/{network}/devices"),
            list: "Devices",
            id_member: "DeviceId",
            arn_member: "DeviceArn",
        }),
    });
    let device = string_member(&created["Device"], "DeviceId");

    // A connection ties two devices of the global network together, so a second device
    // is minted for it.
    let (status, created) = emulator
        .post(
            &format!("/global-networks/{network}/devices"),
            json!({ "SiteId": site, "Type": "router" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label}: the second CreateDevice expected 200, got {status} with {created}"
    );
    let connected_device = string_member(&created["Device"], "DeviceId");

    let (status, created) = emulator
        .post(
            &format!("/global-networks/{network}/connections"),
            json!({ "DeviceId": device, "ConnectedDeviceId": connected_device }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateConnection expected 200, got {status} with {created}"
    );
    rows.push(Minted {
        case: label,
        kind: "connection",
        id: string_member(&created["Connection"], "ConnectionId"),
        arn: string_member(&created["Connection"], "ConnectionArn"),
        scope: Some(network.clone()),
        reader: Some(Reader {
            uri: format!("/global-networks/{network}/connections"),
            list: "Connections",
            id_member: "ConnectionId",
            arn_member: "ConnectionArn",
        }),
    });

    let (status, created) = emulator
        .post("/core-networks", json!({ "GlobalNetworkId": network }))
        .await;
    assert_eq!(
        status, 200,
        "case {label}: CreateCoreNetwork expected 200, got {status} with {created}"
    );
    rows.push(Minted {
        case: label,
        kind: "core-network",
        id: string_member(&created["CoreNetwork"], "CoreNetworkId"),
        arn: string_member(&created["CoreNetwork"], "CoreNetworkArn"),
        scope: None,
        reader: None,
    });

    rows
}

/// The ARN each reader reports for `row`, read back through an operation.
async fn arn_through_reader(emulator: &Emulator, row: &Minted) -> Option<String> {
    let reader = row.reader.as_ref()?;
    let (status, body) = emulator.get(&reader.uri).await;
    assert_eq!(
        status, 200,
        "case {} ({}): the reader expected 200, got {status} with {body}",
        row.case, row.kind
    );
    let item = body[reader.list]
        .as_array()
        .unwrap_or_else(|| panic!("expected a {} array in {body}", reader.list))
        .iter()
        .find(|item| item[reader.id_member] == json!(row.id))
        .unwrap_or_else(|| panic!("expected {} in {body}", row.id))
        .clone();
    Some(string_member(&item, reader.arn_member))
}

#[tokio::test]
async fn an_arn_names_the_global_network_of_every_resource_that_exists_only_inside_one() {
    let emulator = Emulator::new();
    // Two global networks, so each row's scope is a different minted id: an ARN built
    // from the wrong one, or from none, cannot satisfy both halves of the table.
    let mut rows = mint_in_one_global_network(&emulator, "the first global network").await;
    rows.extend(mint_in_one_global_network(&emulator, "the second global network").await);

    for row in &rows {
        let want = row.expected();
        assert_eq!(
            row.arn, want,
            "case {} ({}): expected the create to mint {want}, got {}",
            row.case, row.kind, row.arn
        );

        // The resource path of the ARN: one segment per level of naming, so a scoped
        // kind carries its owner's id and an account-wide one does not.
        let path = row
            .arn
            .rsplit_once(':')
            .map(|(_, resource)| resource.to_string())
            .unwrap_or_else(|| panic!("case {}: {} is not an ARN", row.case, row.arn));
        let segments: Vec<&str> = path.split('/').collect();
        let wanted_segments = if row.scope.is_some() { 3 } else { 2 };
        assert_eq!(
            segments.len(),
            wanted_segments,
            "case {} ({}): expected {wanted_segments} path segments, got {segments:?}",
            row.case,
            row.kind
        );
        assert_eq!(
            segments.last().copied(),
            Some(row.id.as_str()),
            "case {} ({}): the resource's own id must be the last segment, got {segments:?}",
            row.case,
            row.kind
        );

        // Every reader reports the ARN the create minted — not a second rendering of it.
        if let Some(through_reader) = arn_through_reader(&emulator, row).await {
            assert_eq!(
                through_reader, want,
                "case {} ({}): the reader must report {want}, got {through_reader}",
                row.case, row.kind
            );
        }
    }
}

#[tokio::test]
async fn the_arn_a_create_mints_is_the_one_every_tag_operation_resolves() {
    let emulator = Emulator::new();
    let rows = mint_in_one_global_network(&emulator, "tagging through minted ARNs").await;

    for row in &rows {
        let case = row.case;
        let kind = row.kind;

        // TagResource: an empty 200 on an existing resource, whatever its naming depth.
        let (status, body) = emulator
            .post(&tags_uri(&row.arn), json!({ "Tags": [tag("purpose", "probe")] }))
            .await;
        assert_eq!(
            status, 200,
            "case {case} ({kind}): TagResource expected 200, got {status} with {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case} ({kind}): TagResource answers no members, got {body}"
        );

        // ListTagsForResource: the tags just stored, read back through the same ARN.
        let (status, listed) = emulator.get(&tags_uri(&row.arn)).await;
        assert_eq!(
            status, 200,
            "case {case} ({kind}): ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed["TagList"],
            json!([tag("purpose", "probe")]),
            "case {case} ({kind}): expected the stored tag list, got {listed}"
        );

        // UntagResource: an empty 200, and the tag is gone from the stored list.
        let (status, body) = emulator
            .delete(&untag_uri(&row.arn, &["purpose"]))
            .await;
        assert_eq!(
            status, 200,
            "case {case} ({kind}): UntagResource expected 200, got {status} with {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case} ({kind}): UntagResource answers no members, got {body}"
        );
        let (status, listed) = emulator.get(&tags_uri(&row.arn)).await;
        assert_eq!(
            status, 200,
            "case {case} ({kind}): ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed["TagList"],
            json!([]),
            "case {case} ({kind}): the removed key must be gone, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_arn_whose_naming_does_not_match_a_stored_resource_is_refused() {
    let emulator = Emulator::new();
    let rows = mint_in_one_global_network(&emulator, "the addressed global network").await;
    let other = mint_in_one_global_network(&emulator, "a sibling global network").await;
    let site = rows
        .iter()
        .find(|row| row.kind == "site")
        .expect("the table mints a site");
    let network = site
        .scope
        .clone()
        .expect("a site is named under its global network");
    let sibling_network = other
        .iter()
        .find(|row| row.kind == "global-network")
        .expect("the table mints a global network")
        .id
        .clone();

    // Each row is a different way of mis-naming the SAME stored site. A well-formed ARN
    // of this service that names no stored resource is not found (404); a string that is
    // not an ARN of this service in a form it names resources by is a bad request (400).
    let cases: Vec<(&str, String, u16)> = vec![
        ("the ARN the create minted", site.arn.clone(), 200),
        (
            "the site under another existing global network",
            expected_scoped_arn("site", &sibling_network, &site.id),
            404,
        ),
        (
            "the site under a global network that was never minted",
            expected_scoped_arn("site", "global-network-000000000000000ff", &site.id),
            404,
        ),
        (
            "the site named without its global network",
            expected_arn("site", &site.id),
            404,
        ),
        (
            "the global network's id in the place of the site's",
            expected_scoped_arn("site", &site.id, &network),
            404,
        ),
        (
            "a site id under the site's own kind twice over",
            expected_arn("site", &format!("{network}/{}/{}", site.id, site.id)),
            400,
        ),
    ];

    for (case, arn, want) in &cases {
        let (status, body) = emulator.get(&tags_uri(arn)).await;
        assert_eq!(
            status, *want,
            "case {case}: ListTagsForResource({arn}) expected {want}, got {status} with {body}"
        );
        let (status, body) = emulator
            .post(&tags_uri(arn), json!({ "Tags": [tag("env", "test")] }))
            .await;
        assert_eq!(
            status, *want,
            "case {case}: TagResource({arn}) expected {want}, got {status} with {body}"
        );
        let (status, body) = emulator.delete(&untag_uri(arn, &["env"])).await;
        assert_eq!(
            status, *want,
            "case {case}: UntagResource({arn}) expected {want}, got {status} with {body}"
        );

        if *want == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case}: wrong error code in {body}"
            );
            assert_eq!(
                body["ResourceId"].as_str(),
                Some(arn.rsplit('/').next().unwrap_or_default()),
                "case {case}: the refusal must name the id the ARN's last segment carries, got {body}"
            );
        } else if *want == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: wrong error code in {body}"
            );
        }
    }

    // None of those refusals touched the stored site: its own ARN still reads its tags.
    let (status, listed) = emulator.get(&tags_uri(&site.arn)).await;
    assert_eq!(
        status, 200,
        "the stored site must still be readable, got {status} with {listed}"
    );
    assert_eq!(
        listed["TagList"],
        json!([]),
        "no refused call may have stored a tag on the site, got {listed}"
    );
}
