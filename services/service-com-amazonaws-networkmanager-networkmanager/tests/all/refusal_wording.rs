// SPDX-License-Identifier: CC0-1.0

//! The refusals this service answers with recorded AWS wording, and the two preconditions it
//! refuses on, driven over HTTP through the real router.
//!
//! The rules under test, one per table below:
//!
//! * `CreateConnection` ties two devices together ("Creates a connection between two
//!   devices", with `ConnectedDeviceId` "The ID of the second device in the connection":
//!   `smithy_query query="doc" shape="CreateConnection"`, `member="ConnectedDeviceId"`), so
//!   the two required device members naming ONE device is refused with HTTP 400
//!   `ValidationException` and stores nothing.
//! * `RestoreCoreNetworkPolicyVersion` re-submits a PREVIOUS version, so the version the
//!   `LIVE` alias points at is refused with HTTP 400 `ValidationException` and appends
//!   nothing.
//! * A request that is invalid against the state of the core-network policy it addresses —
//!   deleting the `LIVE` version, executing a change set that is not `READY_TO_EXECUTE` — is
//!   a TERMINAL HTTP 400 `ValidationException`, not the retryable 409 `ConflictException`
//!   this service keeps for a concurrent-state conflict.
//! * A delete of one of a global network's own OBJECTS (a device, a link, a site) refused
//!   because a dependent still references it names that object by its kind and the id the
//!   request addressed it with, and nothing about the dependent; a delete addressed at the
//!   CONTAINER keeps its own recorded template, which names the blocking kind instead.
//! * A `ResourceType` outside the set an operation reports on is refused with fixed prose
//!   naming only the member.
//! * A tag key carrying the prefix AWS reserves is refused with fixed prose naming only that
//!   prefix.
//!
//! Each table's rows differ along what its rule generalises over — which members name the
//! one device, where in a history the version in force sits, which kind of resource and
//! which kind of dependent block a delete, how a value falls outside a set, how the reserved
//! prefix is spelled — and each table carries the controls that must still succeed. Nothing
//! reaches into `crate::state`: every effect is read back through an operation.

use serde_json::{json, Value};

use crate::connection_lifecycle::{
    create_connection, create_connection_ok, fixture as connection_fixture, ids_in,
};
// The two expected strings this suite shares with the suites that already pin them, so one
// rule keeps one expected string: the refusal of an execute whose change set is not
// `READY_TO_EXECUTE`, and the refusal of a `ResourceType` outside the set reported on.
use crate::core_network_change_set_execution::CHANGE_SET_STATE_INVALID;
use crate::core_network_policy_versions::{
    append_versions, core_network_with_policy, document_of_version, get_policy, policy,
};
use crate::delete_live_reference_refusal::refusal_message;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator, PAST_SETTLE};
use crate::link_association_lifecycle::{associate_ok, disassociate};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::resource_tagging::{create_taggable, list_tags, tag, tags_uri, untag_uri, GLOBAL_NETWORK};
use crate::unsupported_resource_type_refusal::INVALID_RESOURCE_TYPE;

/// The recorded refusal of a connection whose two device members name one device: entirely
/// fixed text, carrying no id from the request.
const SAME_DEVICE: &str = "Connection cannot be created between the same device.";

/// The recorded refusal of a restore of the version in force: entirely fixed text.
const LIVE_NOT_RESTORABLE: &str = "Live policy cannot be restored.";

/// The recorded refusal of a delete of the version in force: entirely fixed text, naming
/// neither the version id nor the core network the request addressed.
const LIVE_NOT_DELETABLE: &str = "Cannot delete the live version of the core network ID.";

/// The recorded refusal of a tag key carrying the prefix AWS reserves: fixed text naming
/// only that prefix, with the offending key left out.
const RESERVED_TAG_PREFIX: &str = "Tag cannot start with aws:";

/// Assert `body` is an HTTP 400 `ValidationException` whose `Message` is `expected` and which
/// faults no input member (`Reason` is the modelled catch-all, `Fields` is absent).
fn assert_state_refusal(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"],
        json!("Other"),
        "case {case:?}: no input member is at fault, so the reason is the modelled Other, got {body}"
    );
    assert!(
        body["Fields"].is_null(),
        "case {case:?}: the refusal blames no input member, got {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(expected),
        "case {case:?}: expected the message {expected:?}, got {body}"
    );
}

/// `GetConnections` of `network`, answering the `ConnectionId`s it lists.
async fn connection_ids(emulator: &Emulator, case: &str, network: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/connections?maxResults=50"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnections expected 200, got {status} with {body}"
    );
    ids_in(&body)
}

/// One row of the same-device table: how the request spells the two ends, and whether the
/// two members end up naming one device.
struct SameDeviceCase {
    name: &'static str,
    /// The request body, built from the fixture's ids.
    body: fn(&crate::connection_lifecycle::Fixture) -> Value,
    /// Whether the row must be refused.
    refused: bool,
}

/// The rows span the class: one device named at both ends with and without links, with a
/// description and with tags, and — as the controls — two different devices in either order,
/// which must still be accepted whatever else the request carries.
const SAME_DEVICE_CASES: [SameDeviceCase; 7] = [
    SameDeviceCase {
        name: "one device at both ends, naming no link",
        body: |seed| json!({ "DeviceId": seed.device, "ConnectedDeviceId": seed.device }),
        refused: true,
    },
    SameDeviceCase {
        name: "one device at both ends, over two different links",
        body: |seed| {
            json!({
                "DeviceId": seed.device,
                "ConnectedDeviceId": seed.device,
                "LinkId": seed.link,
                "ConnectedLinkId": seed.connected_link,
            })
        },
        refused: true,
    },
    SameDeviceCase {
        name: "one device at both ends, with a description and tags",
        body: |seed| {
            json!({
                "DeviceId": seed.device,
                "ConnectedDeviceId": seed.device,
                "Description": "a self-loop",
                "Tags": [{ "Key": "env", "Value": "test" }],
            })
        },
        refused: true,
    },
    SameDeviceCase {
        name: "the other device at both ends",
        body: |seed| {
            json!({
                "DeviceId": seed.connected_device,
                "ConnectedDeviceId": seed.connected_device,
            })
        },
        refused: true,
    },
    SameDeviceCase {
        name: "two different devices",
        body: |seed| json!({ "DeviceId": seed.device, "ConnectedDeviceId": seed.connected_device }),
        refused: false,
    },
    SameDeviceCase {
        name: "the same two devices, the other way round",
        body: |seed| json!({ "DeviceId": seed.connected_device, "ConnectedDeviceId": seed.device }),
        refused: false,
    },
    SameDeviceCase {
        name: "two different devices over one link at both ends",
        body: |seed| {
            json!({
                "DeviceId": seed.device,
                "ConnectedDeviceId": seed.connected_device,
                "LinkId": seed.link,
                "ConnectedLinkId": seed.link,
            })
        },
        refused: false,
    },
];

#[tokio::test]
async fn a_connection_between_one_device_and_itself_is_refused_and_stores_nothing() {
    for case in SAME_DEVICE_CASES.iter() {
        let name = case.name;
        let emulator = Emulator::new();
        let seed = connection_fixture(&emulator, name).await;

        let (status, body) = create_connection(&emulator, &seed.network, (case.body)(&seed)).await;

        if case.refused {
            assert_state_refusal(name, status, &body, SAME_DEVICE);
            // Nothing was minted: the refusal came before any state was written.
            assert!(
                connection_ids(&emulator, name, &seed.network).await.is_empty(),
                "case {name:?}: a refused create must store no connection"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {name:?}: a connection between two devices expected 200, got {status} with {body}"
        );
        // ROUND TRIP: the accepted connection is in the store, reported by its own reader.
        let connection = body["Connection"].clone();
        let id = connection["ConnectionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no ConnectionId in {body}"))
            .to_string();
        assert_ne!(
            connection["DeviceId"], connection["ConnectedDeviceId"],
            "case {name:?}: a stored connection names two different devices, got {connection}"
        );
        assert_eq!(
            connection_ids(&emulator, name, &seed.network).await,
            vec![id],
            "case {name:?}: GetConnections must list exactly the connection created"
        );
    }
}

/// `RestoreCoreNetworkPolicyVersion` of `version_id`.
async fn restore(emulator: &Emulator, core_network_id: &str, version_id: i32) -> (u16, Value) {
    emulator
        .post(
            &format!(
                "/core-networks/{core_network_id}/core-network-policy-versions/{version_id}/restore"
            ),
            json!({}),
        )
        .await
}

/// `DeleteCoreNetworkPolicyVersion` of `version_id`.
async fn delete_version(
    emulator: &Emulator,
    core_network_id: &str,
    version_id: i32,
) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/core-networks/{core_network_id}/core-network-policy-versions/{version_id}"
        ))
        .await
}

/// `ExecuteCoreNetworkChangeSet` of `version_id`'s change set.
async fn execute(emulator: &Emulator, core_network_id: &str, version_id: i32) -> (u16, Value) {
    emulator
        .post(
            &format!(
                "/core-networks/{core_network_id}/core-network-change-sets/{version_id}/execute"
            ),
            json!({}),
        )
        .await
}

/// The `PolicyVersionId`s `ListCoreNetworkPolicyVersions` reports for `core_network_id`, in
/// the order listed.
async fn listed_versions(emulator: &Emulator, case: &str, core_network_id: &str) -> Vec<i64> {
    let (status, body) = emulator
        .get(&format!(
            "/core-networks/{core_network_id}/core-network-policy-versions?maxResults=50"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCoreNetworkPolicyVersions expected 200, got {status} with {body}"
    );
    body["CoreNetworkPolicyVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkPolicyVersions in {body}"))
        .iter()
        .map(|version| {
            version["PolicyVersionId"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersionId in {version}"))
        })
        .collect()
}

/// The `PolicyVersionId` the `LIVE` alias selects.
async fn live_version(emulator: &Emulator, case: &str, core_network_id: &str) -> i64 {
    let (status, body) = get_policy(emulator, core_network_id, "?alias=LIVE").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkPolicy(LIVE) expected 200, got {status} with {body}"
    );
    policy(&body)["PolicyVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersionId in {body}"))
}

/// Build a core network whose history holds `1 + appended` versions and whose `LIVE` alias
/// has been moved onto `promote` when the row asks for it, answering its id.
///
/// The create executes the document it carries, so version 1 is in force to begin with;
/// promoting another version is one put's change set executed once the clock owes its
/// generation.
async fn history(
    emulator: &Emulator,
    case: &str,
    appended: i32,
    promote: Option<i32>,
) -> String {
    let core_network_id = core_network_with_policy(emulator, case, &document_of_version(1)).await;
    append_versions(emulator, case, &core_network_id, appended).await;
    if let Some(version_id) = promote {
        tokio::time::sleep(PAST_SETTLE).await;
        let (status, body) = execute(emulator, &core_network_id, version_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: executing version {version_id} expected 200, got {status} with {body}"
        );
        tokio::time::sleep(PAST_SETTLE).await;
    }
    core_network_id
}

/// One row of the policy-history table: how wide the history is and which version the `LIVE`
/// alias points at once it is built.
struct HistoryCase {
    name: &'static str,
    /// Versions put on top of the version the create executed.
    appended: i32,
    /// The version promoted to `LIVE` after the puts, if any.
    promote: Option<i32>,
    /// The version the `LIVE` alias then points at.
    live: i32,
    /// A version the history holds that is NOT in force, used as the control.
    draft: i32,
}

/// The rows differ along where in the history the version in force sits (the first version,
/// a middle one, the newest) and how many versions the history holds — what both rules
/// generalise over.
const HISTORY_CASES: [HistoryCase; 4] = [
    HistoryCase {
        name: "the first version of a two-version history is in force",
        appended: 1,
        promote: None,
        live: 1,
        draft: 2,
    },
    HistoryCase {
        name: "the first version of a four-version history is in force",
        appended: 3,
        promote: None,
        live: 1,
        draft: 3,
    },
    HistoryCase {
        name: "a middle version has been promoted",
        appended: 3,
        promote: Some(2),
        live: 2,
        draft: 1,
    },
    HistoryCase {
        name: "the newest version has been promoted",
        appended: 2,
        promote: Some(3),
        live: 3,
        draft: 2,
    },
];

#[tokio::test]
async fn restoring_the_version_in_force_is_refused_and_appends_nothing() {
    for case in HISTORY_CASES.iter() {
        let name = case.name;
        let emulator = Emulator::new();
        let core_network_id = history(&emulator, name, case.appended, case.promote).await;
        assert_eq!(
            live_version(&emulator, name, &core_network_id).await,
            i64::from(case.live),
            "case {name:?}: the row must leave version {} in force",
            case.live
        );
        let before = listed_versions(&emulator, name, &core_network_id).await;

        let (status, body) = restore(&emulator, &core_network_id, case.live).await;
        assert_state_refusal(name, status, &body, LIVE_NOT_RESTORABLE);
        assert_eq!(
            listed_versions(&emulator, name, &core_network_id).await,
            before,
            "case {name:?}: a refused restore must append no version"
        );

        // The control: a version the alias does NOT point at is restorable, and the restore
        // appends exactly one version, which the history then reports.
        let (status, body) = restore(&emulator, &core_network_id, case.draft).await;
        assert_eq!(
            status, 200,
            "case {name:?}: restoring version {} expected 200, got {status} with {body}",
            case.draft
        );
        let mut expected = before.clone();
        expected.push(
            policy(&body)["PolicyVersionId"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {name:?}: no PolicyVersionId in {body}")),
        );
        assert_eq!(
            listed_versions(&emulator, name, &core_network_id).await,
            expected,
            "case {name:?}: the restore must append exactly the version it answered"
        );
        // And the restore put nothing in force: the alias still selects the same version.
        assert_eq!(
            live_version(&emulator, name, &core_network_id).await,
            i64::from(case.live),
            "case {name:?}: a restore must leave the policy in force where it was"
        );
    }
}

#[tokio::test]
async fn deleting_the_version_in_force_is_a_terminal_validation_failure() {
    for case in HISTORY_CASES.iter() {
        let name = case.name;
        let emulator = Emulator::new();
        let core_network_id = history(&emulator, name, case.appended, case.promote).await;
        let before = listed_versions(&emulator, name, &core_network_id).await;

        let (status, body) = delete_version(&emulator, &core_network_id, case.live).await;
        assert_state_refusal(name, status, &body, LIVE_NOT_DELETABLE);
        assert_eq!(
            listed_versions(&emulator, name, &core_network_id).await,
            before,
            "case {name:?}: a refused delete must remove no version"
        );

        // The control: a version the alias does NOT point at is deletable, and the delete
        // removes exactly it.
        let (status, body) = delete_version(&emulator, &core_network_id, case.draft).await;
        assert_eq!(
            status, 200,
            "case {name:?}: deleting version {} expected 200, got {status} with {body}",
            case.draft
        );
        let remaining: Vec<i64> = before
            .iter()
            .copied()
            .filter(|version| *version != i64::from(case.draft))
            .collect();
        assert_eq!(
            listed_versions(&emulator, name, &core_network_id).await,
            remaining,
            "case {name:?}: the delete must remove exactly version {}",
            case.draft
        );
    }
}

#[tokio::test]
async fn executing_a_change_set_that_is_not_ready_is_a_terminal_validation_failure() {
    let case = "a put executed, then executed again";
    let emulator = Emulator::new();
    let core_network_id =
        core_network_with_policy(&emulator, case, &document_of_version(1)).await;
    append_versions(&emulator, case, &core_network_id, 1).await;

    // A change set still being generated has no settled set of differences to apply, so
    // executing it is refused — and the refusal is terminal, not a retryable conflict.
    let (status, body) = execute(&emulator, &core_network_id, 2).await;
    assert_state_refusal(
        "a change set still generating",
        status,
        &body,
        CHANGE_SET_STATE_INVALID,
    );

    tokio::time::sleep(PAST_SETTLE).await;
    let (status, body) = execute(&emulator, &core_network_id, 2).await;
    assert_eq!(
        status, 200,
        "case {case:?}: executing a ready change set expected 200, got {status} with {body}"
    );
    tokio::time::sleep(PAST_SETTLE).await;
    assert_eq!(
        live_version(&emulator, case, &core_network_id).await,
        2,
        "case {case:?}: the executed version must be the one in force"
    );

    // Executing the same version again is refused: its change set has already been applied,
    // and no retry of this call can change that.
    let (status, body) = execute(&emulator, &core_network_id, 2).await;
    assert_state_refusal(
        "a change set already applied",
        status,
        &body,
        CHANGE_SET_STATE_INVALID,
    );
    assert_eq!(
        live_version(&emulator, case, &core_network_id).await,
        2,
        "case {case:?}: the refusal must leave the policy in force where it was"
    );

    // A freshly put version on the same core network is still executable afterwards, so the
    // refusal was about the version addressed and not about the core network.
    append_versions(&emulator, case, &core_network_id, 1).await;
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, body) = execute(&emulator, &core_network_id, 3).await;
    assert_eq!(
        status, 200,
        "case {case:?}: executing the next version expected 200, got {status} with {body}"
    );
}

/// Which resource a delete-blocked row addresses, and what is left referencing it.
#[derive(Clone, Copy, Debug)]
enum Blocked {
    /// A site still holding a device assigned to it.
    SiteHoldingDevice,
    /// A site still holding a link created at it.
    SiteHoldingLink,
    /// A device still associated with a link.
    DeviceAssociatedWithLink,
    /// A link still associated with a device.
    LinkAssociatedWithDevice,
    /// A device a connection still names at one of its ends.
    DeviceNamedByConnection,
}

/// The rows span the class the one template covers: the three kinds of object a global
/// network holds (`DeleteGlobalNetwork`'s documentation groups them as "all global network
/// objects (devices, links, and sites)"), each blocked by a different kind of dependent — a
/// child resource, a site assignment, an association with no id of its own, and a resource
/// with an id of its own — so no row can pass by naming the dependent.
const BLOCKED_CASES: [(&str, Blocked); 5] = [
    ("a site still holding a device", Blocked::SiteHoldingDevice),
    ("a site still holding a link", Blocked::SiteHoldingLink),
    (
        "a device still associated with a link",
        Blocked::DeviceAssociatedWithLink,
    ),
    (
        "a link still associated with a device",
        Blocked::LinkAssociatedWithDevice,
    ),
    (
        "a device a connection names",
        Blocked::DeviceNamedByConnection,
    ),
];

/// What a delete of the CONTAINER answers instead: its own recorded template, which names
/// the global network and the KIND of dependent that blocks it rather than an id. Asserted
/// here so the boundary between the two templates is pinned by the same suite.
const CONTAINER_BLOCKED: &str =
    "Global Network cannot be deleted due to an existing site associated with it.";

#[tokio::test]
async fn a_delete_blocked_by_a_dependent_names_the_resource_it_addressed_and_no_dependent() {
    for (name, blocked) in BLOCKED_CASES {
        let emulator = Emulator::new();
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, name).await;

        // The resource the delete addresses, the kind its refusal spells, and the URI the
        // delete is sent to.
        let (kind, id, uri, untie): (&str, String, String, Option<(String, String)>) = match blocked
        {
            Blocked::SiteHoldingDevice => {
                create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
                (
                    "Site",
                    site.clone(),
                    format!("/global-networks/{network}/sites/{site}"),
                    None,
                )
            }
            Blocked::SiteHoldingLink => {
                create_link_ok(&emulator, &network, &site, &plain_link()).await;
                (
                    "Site",
                    site.clone(),
                    format!("/global-networks/{network}/sites/{site}"),
                    None,
                )
            }
            Blocked::DeviceAssociatedWithLink => {
                let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
                let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
                associate_ok(&emulator, &network, &device, &link).await;
                (
                    "Device",
                    device.clone(),
                    format!("/global-networks/{network}/devices/{device}"),
                    Some((device, link)),
                )
            }
            Blocked::LinkAssociatedWithDevice => {
                let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
                let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
                associate_ok(&emulator, &network, &device, &link).await;
                (
                    "Link",
                    link.clone(),
                    format!("/global-networks/{network}/links/{link}"),
                    Some((device, link)),
                )
            }
            Blocked::DeviceNamedByConnection => {
                let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
                let partner =
                    create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
                create_connection_ok(
                    &emulator,
                    &network,
                    json!({ "DeviceId": partner, "ConnectedDeviceId": device }),
                )
                .await;
                (
                    "Device",
                    device.clone(),
                    format!("/global-networks/{network}/devices/{device}"),
                    None,
                )
            }
        };

        let (status, body) = emulator.delete(&uri).await;
        let expected = refusal_message(kind, &id);
        assert_state_refusal(name, status, &body, &expected);
        // The id is the only variable part, so the refusal cannot have been built from the
        // dependent: the kinds that could have blocked it are nowhere in what the handler
        // answered around the id.
        let answered = body["Message"].as_str().unwrap_or_default();
        let prose = answered.replace(&id, "");
        for dependent in ["device", "link association", "site", "connection"] {
            assert!(
                !prose.contains(dependent),
                "case {name:?}: the refusal must name no dependent kind, yet it names a \
                 {dependent}: {answered:?}"
            );
        }

        // The control: with the reference gone the identical call succeeds, so the refusal
        // tracked stored state rather than the shape of the request. Only the association
        // rows can be untied without deleting the resource under test.
        if let Some((device, link)) = untie {
            let (status, untied) = disassociate(&emulator, &network, &device, &link).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DisassociateLink expected 200, got {status} with {untied}"
            );
            let (status, deleted) = emulator.delete(&uri).await;
            assert_eq!(
                status, 200,
                "case {name:?}: with the association gone the same delete must succeed, got \
                 {status} with {deleted}"
            );
        }
    }
}

#[tokio::test]
async fn the_delete_of_the_container_holding_them_keeps_its_own_recorded_template() {
    // The boundary between the two recorded templates: a delete addressed at the global
    // network itself names the KIND that blocks it and carries no id, which is what the
    // recorded refusal of a global network that still holds one of its objects spells. The
    // template the objects above answer therefore reaches the objects and not every delete.
    let case = "a global network still holding a site";
    let emulator = Emulator::new();
    let network = emulator.create_global_network(case).await;
    let site = emulator.create_site(&network, case).await;

    let (status, body) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_state_refusal(case, status, &body, CONTAINER_BLOCKED);

    // And with the object gone the identical call succeeds, so the refusal tracked stored
    // state rather than the shape of the request.
    let (status, removed) = emulator
        .delete(&format!("/global-networks/{network}/sites/{site}"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteSite expected 200, got {status} with {removed}"
    );
    let (status, deleted) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: with the site gone the same delete must succeed, got {status} with {deleted}"
    );
}

#[tokio::test]
async fn an_unsupported_resource_type_is_refused_with_fixed_prose_naming_only_the_member() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("resource type").await;

    // The rows differ along HOW the value falls outside the set: an invented name, a
    // documented name in the wrong case, one with the wrong separator, a prefix of one, and
    // an id rather than a type. The refusal is the same prose for all of them, because it
    // echoes neither the value nor the set.
    for (case, resource_type) in [
        ("an invented type", "not-a-real-type"),
        ("a documented type in the wrong case", "SITE"),
        ("an underscore instead of a hyphen", "core_network"),
        ("a prefix of a documented type", "transit"),
        ("a resource id instead of a type", "site-00000000000000001"),
    ] {
        let (status, body) = emulator
            .get(&format!(
                "/global-networks/{network}/network-resource-count?resourceType={resource_type}"
            ))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(INVALID_RESOURCE_TYPE),
            "case {case:?}: expected the message {INVALID_RESOURCE_TYPE:?}, got {body}"
        );
        assert_eq!(
            body["Fields"],
            json!([{ "Name": "ResourceType", "Message": INVALID_RESOURCE_TYPE }]),
            "case {case:?}: expected one Fields entry naming the member and carrying the \
             top-level message, got {body}"
        );
    }

    // The control: a value the member's documentation lists is answered, so the refusals
    // above are the member check and not the operation refusing its own catalogue.
    for resource_type in ["site", "device", "connection"] {
        let (status, body) = emulator
            .get(&format!(
                "/global-networks/{network}/network-resource-count?resourceType={resource_type}"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case a supported type ({resource_type}): expected 200, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_with_fixed_prose_naming_only_the_prefix() {
    // The rows differ along how the reserved prefix is spelled and what follows it — the bare
    // prefix, an AWS-generated key, the prefix in other cases — plus the controls, which are
    // ordinary keys that only look reserved.
    for (case, key, refused) in [
        ("the bare reserved prefix", "aws:", true),
        ("an AWS-generated key", "aws:cloudformation:stack-name", true),
        ("the prefix in upper case", "AWS:Region", true),
        ("the prefix in mixed case", "Aws:service", true),
        ("a key that merely begins with aws", "awsome", false),
        ("the prefix away from the start", "my-aws:key", false),
    ] {
        let emulator = Emulator::new();
        let resource = create_taggable(&emulator, GLOBAL_NETWORK, &Value::Null).await;

        let (status, body) = emulator
            .post(&tags_uri(&resource.arn), json!({ "Tags": [tag(key, "x")] }))
            .await;
        if refused {
            assert_eq!(
                status, 400,
                "case {case:?}: TagResource({key:?}) expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(RESERVED_TAG_PREFIX),
                "case {case:?}: expected the message {RESERVED_TAG_PREFIX:?}, got {body}"
            );
            // The refusal stored nothing.
            let (_, listed) = list_tags(&emulator, &resource.arn).await;
            assert_eq!(
                listed,
                json!([]),
                "case {case:?}: a refused TagResource must store no tag, got {listed}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: TagResource({key:?}) expected 200, got {status} with {body}"
            );
            let (_, listed) = list_tags(&emulator, &resource.arn).await;
            assert_eq!(
                listed,
                json!([tag(key, "x")]),
                "case {case:?}: the applied tag must be readable, got {listed}"
            );
        }

        // The same rule reaches the key list a removal carries and the `Tags` a create
        // carries, and each reports the one message.
        let (status, body) = emulator.delete(&untag_uri(&resource.arn, &[key])).await;
        if refused {
            assert_eq!(
                status, 400,
                "case {case:?}: UntagResource({key:?}) expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(RESERVED_TAG_PREFIX),
                "case {case:?}: UntagResource must report the one message, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: UntagResource({key:?}) expected 200, got {status} with {body}"
            );
        }

        let (status, body) = emulator
            .post(
                "/global-networks",
                json!({ "Description": case, "Tags": [tag(key, "x")] }),
            )
            .await;
        if refused {
            assert_eq!(
                status, 400,
                "case {case:?}: CreateGlobalNetwork({key:?}) expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(RESERVED_TAG_PREFIX),
                "case {case:?}: a create must report the one message, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: CreateGlobalNetwork({key:?}) expected 200, got {status} with {body}"
            );
            assert!(
                ids_of(
                    &emulator.get("/global-networks").await.1,
                    "GlobalNetworks",
                    "GlobalNetworkId"
                )
                .len()
                    >= 2,
                "case {case:?}: the accepted create must be stored"
            );
        }
    }
}
