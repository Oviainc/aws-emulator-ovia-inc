// SPDX-License-Identifier: CC0-1.0

//! `GetContainerAPIMetadata`: the container-API metadata Lightsail publishes,
//! driven over the wire through the same front door a client uses.
//!
//! The rule under test: `GetContainerAPIMetadata` reports the service's published
//! container-API metadata catalog — one entry per metadatum, in the catalog's own
//! order, each entry a map carrying exactly that metadatum's name under `name` and
//! its value under `value`, among them the current version of the Lightsail Control
//! (lightsailctl) plugin — and reporting it changes nothing a later caller sees.
//!
//! So the cases here are TABLES: one row per catalog the entry-list rule can be
//! asked about (empty, one metadatum, several, differing in count, order and value
//! form), and the published catalog's own invariants (unique names, non-empty
//! values, a `major.minor.patch` plugin version) asserted over whatever
//! [`CONTAINER_API_METADATA_SPECS`] holds rather than over one entry written out by
//! hand. The input shape declares no member, so the request itself has no dimension
//! to vary: what varies across the read-invariance case is when a read happens and
//! what the store holds when it does.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    container_api_metadata, container_api_metadata_from, CONTAINER_API_METADATA_NAME_KEY,
    CONTAINER_API_METADATA_SPECS, CONTAINER_API_METADATA_VALUE_KEY,
};

use crate::instance_lifecycle::Emulator;

/// The two keys one reported metadatum carries, in wire spelling: the metadatum's
/// name and its value.
///
/// SOURCE: the pair the documented way of reading this catalog selects on
/// (`aws lightsail get-container-api-metadata --query
/// "metadata[?name=='lightsailctlVersion'].value"`). Written out here independently
/// of the service's own constants, so these rows pin the published form rather than
/// restating whatever the code holds.
const ENTRY_KEYS: &[&str] = &["name", "value"];

/// The metadatum the modelled `metadata` member calls out: "the current version of
/// the Lightsail Control (lightsailctl) plugin".
const LIGHTSAILCTL_VERSION_NAME: &str = "lightsailctlVersion";

/// The metadata `GetContainerAPIMetadata` reports for `request`, keyed by the name
/// each entry carries.
async fn reported(emulator: &Emulator, request: Value) -> BTreeMap<String, String> {
    let case = request.to_string();
    let body = emulator.ok("GetContainerAPIMetadata", request).await;
    let entries = body["metadata"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a metadata array, got {body:?}"))
        .clone();

    let mut keyed = BTreeMap::new();
    for entry in entries {
        let members: BTreeSet<&str> = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: a metadata entry is an object, got {entry:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            ENTRY_KEYS.iter().copied().collect::<BTreeSet<_>>(),
            "case {case}: expected an entry carrying exactly the published keys, got {members:?}"
        );

        let name = entry["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: every entry names its metadatum, got {entry:?}"))
            .to_string();
        let value = entry["value"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: every entry carries a value, got {entry:?}"))
            .to_string();
        assert!(
            keyed.insert(name.clone(), value).is_none(),
            "case {case}: metadatum {name:?} was reported twice"
        );
    }
    keyed
}

/// Whether `version` is a `major.minor.patch` release version.
fn is_release_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// Every metadatum the catalog publishes comes back under its own name, and the
/// plugin version it calls out is a release version of that plugin.
#[tokio::test]
async fn every_published_metadatum_is_reported_under_its_own_name() {
    let emulator = Emulator::new();
    let metadata = reported(&emulator, json!({})).await;

    assert!(
        !metadata.is_empty(),
        "the service must publish at least one container-API metadatum"
    );

    // The reported set is exactly the published catalog, name for name and value
    // for value — so the rule holds whatever that catalog grows to hold.
    let expected: BTreeMap<String, String> = CONTAINER_API_METADATA_SPECS
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        metadata, expected,
        "case default request: expected exactly the published catalog, got {metadata:?}"
    );

    for (name, value) in &metadata {
        assert!(
            !name.is_empty() && !value.is_empty(),
            "case {name:?}: a metadatum names itself and carries a value, got {value:?}"
        );
    }

    let version = metadata
        .get(LIGHTSAILCTL_VERSION_NAME)
        .unwrap_or_else(|| {
            panic!(
                "case {LIGHTSAILCTL_VERSION_NAME}: expected the plugin version to be published, \
                 got {:?}",
                metadata.keys().collect::<Vec<_>>()
            )
        });
    assert!(
        is_release_version(version),
        "case {LIGHTSAILCTL_VERSION_NAME}: expected a major.minor.patch plugin version, \
         got {version:?}"
    );
}

/// The entry-list rule itself, over catalogs the published one does not (yet) have:
/// rows differ along how many metadata the catalog holds, their order, and the form
/// of their values.
///
/// This is what tells "one entry per metadatum, in catalog order, keyed by `name`
/// and `value`" apart from "whatever the published catalog happens to be".
#[test]
fn one_entry_per_metadatum_is_reported_in_the_catalogs_own_order() {
    /// One row: the case name, the catalog, and the `(name, value)` pairs the
    /// entry list must carry, in order.
    struct Row {
        case: &'static str,
        catalog: &'static [(&'static str, &'static str)],
    }

    let rows = [
        Row {
            case: "an empty catalog",
            catalog: &[],
        },
        Row {
            case: "one metadatum",
            catalog: &[("lightsailctlVersion", "1.0.6")],
        },
        Row {
            case: "several metadata",
            catalog: &[
                ("lightsailctlVersion", "1.0.6"),
                ("containerImageRegistry", "example-registry"),
            ],
        },
        Row {
            case: "the same metadata in the other order",
            catalog: &[
                ("containerImageRegistry", "example-registry"),
                ("lightsailctlVersion", "1.0.6"),
            ],
        },
        Row {
            case: "values of other forms",
            catalog: &[
                ("aVersion", "10.20.30"),
                ("aUrl", "https://example.invalid/plugin"),
                ("aSentence", "a value with spaces, commas and 0 digits"),
            ],
        },
    ];

    for row in &rows {
        let case = row.case;
        let entries = container_api_metadata_from(row.catalog);
        assert_eq!(
            entries.len(),
            row.catalog.len(),
            "case {case:?}: expected one entry per metadatum, got {entries:?}"
        );

        for (index, (name, value)) in row.catalog.iter().enumerate() {
            let entry = &entries[index];
            let members: BTreeSet<&str> = entry.keys().map(String::as_str).collect();
            assert_eq!(
                members,
                ENTRY_KEYS.iter().copied().collect::<BTreeSet<_>>(),
                "case {case:?} entry {index}: expected exactly the published keys, got {members:?}"
            );
            assert_eq!(
                entry.get(CONTAINER_API_METADATA_NAME_KEY).map(String::as_str),
                Some(*name),
                "case {case:?} entry {index}: expected the metadatum's name in catalog order, \
                 got {entry:?}"
            );
            assert_eq!(
                entry
                    .get(CONTAINER_API_METADATA_VALUE_KEY)
                    .map(String::as_str),
                Some(*value),
                "case {case:?} entry {index}: expected the metadatum's own value, got {entry:?}"
            );
        }
    }
}

/// Reporting the catalog is a read: it answers the same thing on every read,
/// whatever else the store holds, and two emulators publish the same catalog.
#[tokio::test]
async fn reporting_the_metadata_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    // The input shape declares no member, so there is exactly one request form; what
    // varies across these reads is only WHEN they happen and what the store holds.
    let first = reported(&emulator, json!({})).await;
    let again = reported(&emulator, json!({})).await;
    assert_eq!(
        again, first,
        "case a repeated read: two reads of the metadata must agree, got {again:?} after {first:?}"
    );

    // A mutation of unrelated state, then the same reads again: the published
    // metadata is not per-caller and does not depend on what the store holds.
    // `CreateContainerService` answers HTTP 201, its modelled success status.
    let (status, created) = emulator
        .call(
            "CreateContainerService",
            json!({
                "serviceName": "metadata-reader",
                "power": "nano",
                "scale": 1
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "case the unrelated mutation: expected HTTP 201, got {status} {created:?}"
    );

    let after_mutation = reported(&emulator, json!({})).await;
    assert_eq!(
        after_mutation, first,
        "case after an unrelated mutation: the published metadata must not depend on what the \
         store holds, got {after_mutation:?} after {first:?}"
    );

    // The container service the mutation created is still there: reading the
    // metadata consumed nothing.
    let service = emulator
        .ok(
            "GetContainerServices",
            json!({"serviceName": "metadata-reader"}),
        )
        .await;
    assert_eq!(
        service["containerServices"][0]["containerServiceName"].as_str(),
        Some("metadata-reader"),
        "case store unchanged: expected the created container service to survive the reads, \
         got {service:?}"
    );

    // A second emulator, with its own store, publishes the same catalog, and the
    // in-process catalog accessor agrees with what the wire reports.
    let other = Emulator::new();
    let elsewhere = reported(&other, json!({})).await;
    assert_eq!(
        elsewhere, first,
        "case a second emulator: the catalog must not depend on which emulator serves it, \
         got {elsewhere:?}"
    );

    let in_process: BTreeMap<String, String> = container_api_metadata()
        .into_iter()
        .map(|entry| {
            (
                entry[CONTAINER_API_METADATA_NAME_KEY].clone(),
                entry[CONTAINER_API_METADATA_VALUE_KEY].clone(),
            )
        })
        .collect();
    assert_eq!(
        in_process, first,
        "case the accessor and the wire: expected the same catalog, got {in_process:?}"
    );
}
