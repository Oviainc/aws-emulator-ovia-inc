// SPDX-License-Identifier: CC0-1.0

//! Revising a stored Realtime script's content, over the wire: "to update script content,
//! provide an updated zip file by pointing to either a local file or an Amazon S3 bucket
//! location. You can use either method regardless of how the original script was
//! uploaded."
//!
//! So an `UpdateScript` that names a content source writes the location and the size that
//! SOURCE decides onto the stored script — whichever of the two sources the original
//! upload used — while the script's identity (its id, its ARN and its creation time) and
//! every member the request omits keep the values the store already holds, and
//! `DescribeScript` serves that one stored record back whether it is selected by id or by
//! ARN.
//!
//! The harness (`GameLift`) is the one `hosting_loop.rs` defines; the script helpers are
//! the ones `scripts.rs` drives `CreateScript` with and the service-owned location is the
//! one `resource_updates.rs` names.

use serde_json::{json, Value};

use crate::hosting_loop::GameLift;
use crate::resource_updates::service_owned_script_location;
use crate::scripts::{caller_location, script_arn, script_id_of, zip_bytes, zip_file_member};

/// The modelled bounds of `UpdateScriptInput$ZipFile` (`smithy_query
/// operation=UpdateScript`: `ZipBlob`, len 0..5000000) — "maximum size of a zip file is
/// 5 MB".
const MIN_ZIP_BYTES: usize = 0;
/// See [`MIN_ZIP_BYTES`].
const MAX_ZIP_BYTES: usize = 5_000_000;

/// One of the two documented sources a script's zip file comes from.
#[derive(Clone, Copy)]
enum Source {
    /// A local zip file of this many bytes, sent in `ZipFile`.
    LocalZip(usize),
    /// An object at this key of the caller's own S3 bucket, named by `StorageLocation`.
    CallerBucket(&'static str),
}

impl Source {
    /// The request members that name this source.
    fn members(self) -> Value {
        match self {
            Source::LocalZip(len) => json!({"ZipFile": zip_file_member(&zip_bytes(len))}),
            Source::CallerBucket(key) => json!({"StorageLocation": caller_location(key)}),
        }
    }

    /// The `StorageLocation` and `SizeOnDisk` a script whose content came from this source
    /// reports: a local upload is held at the service-owned location for that script and
    /// its size is known, while for a file left in the caller's own bucket the location is
    /// the one the request named and the size "remains at 0".
    fn reported(self, script_id: &str) -> Vec<(&'static str, Value)> {
        match self {
            Source::LocalZip(len) => vec![
                ("StorageLocation", service_owned_script_location(script_id)),
                ("SizeOnDisk", json!(len)),
            ],
            Source::CallerBucket(key) => vec![
                ("StorageLocation", caller_location(key)),
                ("SizeOnDisk", json!(0)),
            ],
        }
    }
}

/// `before` with every member of `changes` replaced by the value supplied for it.
fn with_changes(before: &Value, changes: &[(&str, Value)]) -> Value {
    let mut expected = before.clone();
    for (member, value) in changes {
        expected[*member] = value.clone();
    }
    expected
}

/// The `Script` a describe reports for `selector`, a script id or a script ARN.
async fn describe_script(gamelift: &GameLift, selector: &str) -> Value {
    gamelift
        .ok("DescribeScript", json!({"ScriptId": selector}))
        .await["Script"]
        .clone()
}

/// Create one script from `source` carrying the metadata the revisions below change, and
/// report its id.
async fn create_script(gamelift: &GameLift, name: &str, source: Source) -> String {
    let mut request = json!({"Name": name, "Version": "1.0.0"});
    for (member, value) in source.members().as_object().expect("source members") {
        request[member] = value.clone();
    }
    let created = gamelift.ok("CreateScript", request).await;
    script_id_of(&created["Script"])
}

/// An `UpdateScript` reports — and a later `DescribeScript` serves — the content source the
/// UPDATE named, whichever source the create used, with the metadata the update supplied
/// written on and everything else, including the script's identity, left as stored.
///
/// The rows differ along what the rule generalises over: which documented source the
/// original upload used, which source the revision names (including a revision that names
/// none at all), the size of a revised local zip file at and between the modelled bounds,
/// and which metadata members the request supplies.
#[tokio::test]
async fn a_revision_reports_the_source_it_named_whatever_the_create_uploaded() {
    struct Case {
        label: &'static str,
        created: Source,
        /// The content source the revision names, or `None` for a metadata-only revision.
        revised: Option<Source>,
        /// The metadata members the revision supplies.
        metadata: Value,
    }

    let cases = [
        Case {
            label: "a local upload revised by another local upload",
            created: Source::LocalZip(256),
            revised: Some(Source::LocalZip(1_024)),
            metadata: json!({"Version": "1.0.1"}),
        },
        Case {
            label: "a local upload revised from the caller's own bucket",
            created: Source::LocalZip(256),
            revised: Some(Source::CallerBucket("scripts/revised.zip")),
            metadata: json!({"Name": "revised-from-s3", "Version": "2.0.0"}),
        },
        Case {
            label: "a script in the caller's bucket revised by a local upload",
            created: Source::CallerBucket("scripts/original.zip"),
            revised: Some(Source::LocalZip(64)),
            metadata: json!({}),
        },
        Case {
            label: "a script in the caller's bucket revised at another key of it",
            created: Source::CallerBucket("scripts/original.zip"),
            revised: Some(Source::CallerBucket("scripts/second.zip")),
            metadata: json!({"Name": "re-keyed"}),
        },
        Case {
            label: "a local upload revised by a zip file at the modelled minimum size",
            created: Source::LocalZip(256),
            revised: Some(Source::LocalZip(MIN_ZIP_BYTES)),
            metadata: json!({}),
        },
        Case {
            label: "a local upload revised by a zip file at the modelled maximum size",
            created: Source::LocalZip(1),
            revised: Some(Source::LocalZip(MAX_ZIP_BYTES)),
            metadata: json!({"Version": "5-megabytes"}),
        },
        Case {
            label: "a script in the caller's bucket whose metadata alone is revised",
            created: Source::CallerBucket("scripts/original.zip"),
            revised: None,
            metadata: json!({"Name": "metadata-only", "Version": "3.0.0"}),
        },
        Case {
            label: "a local upload whose version alone is revised",
            created: Source::LocalZip(512),
            revised: None,
            metadata: json!({"Version": "3.1.0"}),
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let script_id = create_script(&gamelift, &format!("revised-{index}"), case.created).await;
        let before = describe_script(&gamelift, &script_id).await;

        let mut request = case.metadata.clone();
        request["ScriptId"] = json!(&script_id);
        if let Some(revised) = case.revised {
            for (member, value) in revised.members().as_object().expect("source members") {
                request[member] = value.clone();
            }
        }
        let reported = gamelift.ok("UpdateScript", request).await["Script"].clone();

        // The update reports the STORED record: the describe that follows it, by id and by
        // the ARN that names the same script, can only agree with it.
        for selector in [script_id.clone(), script_arn(&script_id)] {
            let described = describe_script(&gamelift, &selector).await;
            assert_eq!(
                described, reported,
                "case {label:?}: the describe naming {selector:?} must serve the record the \
                 update reported ({reported}), got {described}"
            );
        }

        // Every member the update supplied is written on; the content source it named
        // decides the location and the size; everything else — including the identity the
        // create minted — is left exactly as stored.
        let mut changes: Vec<(&str, Value)> = case
            .metadata
            .as_object()
            .expect("the metadata members")
            .iter()
            .map(|(member, value)| (member.as_str(), value.clone()))
            .collect();
        if let Some(revised) = case.revised {
            changes.extend(revised.reported(&script_id));
        }
        let expected = with_changes(&before, &changes);
        assert_eq!(
            reported, expected,
            "case {label:?}: the revision must change only {:?}, got {reported}",
            changes.iter().map(|(member, _)| *member).collect::<Vec<_>>()
        );
        for member in ["ScriptId", "ScriptArn", "CreationTime"] {
            assert_eq!(
                reported[member], before[member],
                "case {label:?}: a revision must not change {member}, got {}",
                reported[member]
            );
        }
    }
}

/// Successive revisions accumulate on the one stored record — each writes what it supplies
/// and keeps what earlier revisions wrote — and a revision that supplies exactly what the
/// store already holds leaves the record as it is.
#[tokio::test]
async fn successive_revisions_accumulate_on_the_one_stored_script() {
    let gamelift = GameLift::new();
    let script_id = create_script(&gamelift, "accumulating", Source::LocalZip(128)).await;

    /// One step of the sequence: the members the revision supplies, and the members the
    /// record must carry after it.
    struct Step {
        label: &'static str,
        update: Value,
        expected: Vec<(&'static str, Value)>,
    }

    let steps = [
        Step {
            label: "the version alone",
            update: json!({"Version": "1.1.0"}),
            expected: vec![("Name", json!("accumulating")), ("Version", json!("1.1.0"))],
        },
        Step {
            label: "the name alone, keeping the version the step before wrote",
            update: json!({"Name": "renamed"}),
            expected: vec![("Name", json!("renamed")), ("Version", json!("1.1.0"))],
        },
        Step {
            label: "a revised local zip file, keeping both metadata members",
            update: json!({"ZipFile": zip_file_member(&zip_bytes(777))}),
            expected: vec![
                ("Name", json!("renamed")),
                ("Version", json!("1.1.0")),
                ("SizeOnDisk", json!(777)),
            ],
        },
        Step {
            label: "that same revised zip file again",
            update: json!({"ZipFile": zip_file_member(&zip_bytes(777))}),
            expected: vec![
                ("Name", json!("renamed")),
                ("Version", json!("1.1.0")),
                ("SizeOnDisk", json!(777)),
            ],
        },
        Step {
            label: "the content moved to the caller's own bucket",
            update: json!({"StorageLocation": caller_location("scripts/moved.zip")}),
            expected: vec![
                ("Name", json!("renamed")),
                ("Version", json!("1.1.0")),
                // "When files are uploaded from an S3 location, this value remains at 0."
                ("SizeOnDisk", json!(0)),
                ("StorageLocation", caller_location("scripts/moved.zip")),
            ],
        },
        Step {
            label: "the version alone again, keeping the content in the caller's bucket",
            update: json!({"Version": "1.2.0"}),
            expected: vec![
                ("Name", json!("renamed")),
                ("Version", json!("1.2.0")),
                ("SizeOnDisk", json!(0)),
                ("StorageLocation", caller_location("scripts/moved.zip")),
            ],
        },
    ];

    let created = describe_script(&gamelift, &script_id).await;
    let mut previous = created.clone();
    for step in &steps {
        let label = step.label;
        let mut request = step.update.clone();
        request["ScriptId"] = json!(&script_id);
        let reported = gamelift.ok("UpdateScript", request).await["Script"].clone();
        let stored = describe_script(&gamelift, &script_id).await;
        assert_eq!(
            stored, reported,
            "case {label:?}: the revision must report the record the store holds, got {reported}"
        );
        for (member, value) in &step.expected {
            assert_eq!(
                stored[*member], *value,
                "case {label:?}: after the revision {member} must read {value}, got {}",
                stored[*member]
            );
        }
        assert_eq!(
            stored["ScriptId"], created["ScriptId"],
            "case {label:?}: the sequence must revise the one created script, got {stored}"
        );
        if step.label == "that same revised zip file again" {
            assert_eq!(
                stored, previous,
                "case {label:?}: a revision supplying what the store already holds must leave \
                 the record as it is, got {stored}"
            );
        }
        previous = stored;
    }
}
