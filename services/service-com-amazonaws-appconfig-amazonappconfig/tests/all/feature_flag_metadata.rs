// SPDX-License-Identifier: CC0-1.0

//! The feature-flags helper's `_createdAt` / `_updatedAt` stamps.
//!
//! RULE: a hosted configuration version written to a configuration profile whose `Type`
//! is `AWS.AppConfig.FeatureFlags` is stored as that profile's feature-flags helper
//! rewrites it — EVERY entry of the document's `flags` map and EVERY entry of its
//! `values` map carries `_createdAt` and `_updatedAt` as RFC 3339 instants, `_createdAt`
//! naming when that entry's key first appeared in the profile and `_updatedAt` when its
//! content last changed — and nothing else about the document changes. A profile of any
//! other `Type` stores the submitted bytes unchanged.
//!
//! The rows span what the rule generalises over: the number of entries in a document
//! (one, two, three), the form of the flag keys the schema's key pattern allows
//! (hyphens, underscores, digits), the presence and absence of the optional `values`
//! map, and what else a flag definition and a flag's values carry (name, description,
//! attributes, constraints, non-`enabled` value members). The flag key the recorded
//! defect cited is one row among them; the per-entry nature of the stamping is asserted
//! on the multi-flag rows, and the history half of the rule across successive versions
//! of one profile.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Map, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state::feature_flags::{
    CREATED_AT_KEY, STAMPED_MAPS, UPDATED_AT_KEY,
};
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    FEATURE_FLAGS_PROFILE_TYPE, FREEFORM_PROFILE_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::appconfig_workflow::{create_application, create_profile_from, send, Answer};

/// One document the rule must hold for.
struct Row {
    case: &'static str,
    document: Value,
}

/// The documents this suite spans. Each is a document the feature-flags schema accepts;
/// they differ in entry count, key form, whether `values` is present, and what the
/// entries carry.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one flag, name only",
            document: json!({
                "flags": { "sim2real-probe-appconfig-errflag": { "name": "errflag" } },
                "values": { "sim2real-probe-appconfig-errflag": { "enabled": true } },
                "version": "1",
            }),
        },
        Row {
            case: "one flag, no values map",
            document: json!({
                "flags": { "myflag": { "name": "myflag" } },
                "version": "1",
            }),
        },
        Row {
            case: "one flag, disabled with a description",
            document: json!({
                "flags": {
                    "rolloutflag": { "name": "rolloutflag", "description": "a rollout" },
                },
                "values": { "rolloutflag": { "enabled": false } },
                "version": "1",
            }),
        },
        Row {
            case: "two flags in one document",
            document: json!({
                "flags": {
                    "sim2real-probe-appconfig-isoflag1": { "name": "isoflag1" },
                    "sim2real-probe-appconfig-isoflag2": { "name": "isoflag2" },
                },
                "values": {
                    "sim2real-probe-appconfig-isoflag1": { "enabled": true },
                    "sim2real-probe-appconfig-isoflag2": { "enabled": false },
                },
                "version": "1",
            }),
        },
        Row {
            case: "three flags, one of them without a values entry",
            document: json!({
                "flags": {
                    "my-feature-flag": { "name": "my-feature-flag" },
                    "second_flag": { "name": "second_flag" },
                    "flag3": { "name": "flag3" },
                },
                "values": {
                    "my-feature-flag": { "enabled": true },
                    "flag3": { "enabled": true },
                },
                "version": "1",
            }),
        },
        Row {
            case: "flag with attributes, constraints and attribute values",
            document: json!({
                "flags": {
                    "sim2real-probe-appconfig-ovflag": {
                        "name": "ovflag",
                        "description": "a flag carrying attributes",
                        "attributes": {
                            "limit": { "constraints": { "type": "number" } },
                            "label": { "constraints": { "type": "string" } },
                            "allow_list": { "constraints": { "type": "array" } },
                        },
                    },
                },
                "values": {
                    "sim2real-probe-appconfig-ovflag": {
                        "enabled": true,
                        "limit": 7,
                        "label": "canary",
                        "allow_list": ["a", "b"],
                    },
                },
                "version": "1",
            }),
        },
        Row {
            case: "values entry without enabled",
            document: json!({
                "flags": { "sim2real-probe-appconfig-runflag": { "name": "runflag" } },
                "values": { "sim2real-probe-appconfig-runflag": { "limit": 1 } },
                "version": "1",
            }),
        },
    ]
}

/// Create a configuration profile of `profile_type` in the hosted store.
async fn create_typed_profile(
    router: &MultiServiceRouter,
    application: &str,
    name: &str,
    profile_type: &str,
) -> String {
    let created = create_profile_from(
        router,
        application,
        name,
        &json!({ "Name": name, "LocationUri": "hosted", "Type": profile_type }),
    )
    .await;
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateConfigurationProfile answered no Id"))
}

/// `POST` one hosted configuration version holding `document`.
async fn write_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    document: &Value,
) -> Answer {
    let content = serde_json::to_vec(document).expect("the document must serialize");
    send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content,
        &[],
    )
    .await
}

/// `GET` one hosted configuration version back.
async fn read_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    version: i64,
) -> Answer {
    send(
        router,
        "GET",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/{version}"
        ),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// The version the create response reports, from its bound `Version-Number` header.
fn version_number(answer: &Answer, case: &str) -> i64 {
    answer
        .header("Version-Number")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_else(|| panic!("case {case:?}: no Version-Number header on the create response"))
}

/// A 201 create response's document.
fn created_document(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateHostedConfigurationVersion expected 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// One entry of a stamped map in `document`, as an object.
fn entry<'a>(document: &'a Value, map: &str, key: &str, case: &str) -> &'a Map<String, Value> {
    document
        .get(map)
        .and_then(|entries| entries.get(key))
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: {map}.{key} is missing from {document}"))
}

/// The millisecond instant an RFC 3339 stamp names, parsed by the same date type the
/// wire serializer uses — so a stamp that is not an instant of that form fails here.
fn instant(stamp: &Value, what: &str, case: &str) -> i64 {
    let text = stamp
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {what} must be a string, got {stamp}"));
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: {what} {text:?} is not an RFC 3339 instant: {error}")
    });
    assert!(
        text.ends_with('Z'),
        "case {case:?}: {what} must be a UTC instant, got {text:?}"
    );
    // The stamps AppConfig writes carry a millisecond fraction, three digits wide
    // whatever the value (the recorded documents show `…:55.763Z`), so the width of a
    // stamp must not depend on its milliseconds.
    let fraction = text
        .rsplit_once('.')
        .map(|(_, fraction)| fraction.trim_end_matches('Z'))
        .unwrap_or_default();
    assert!(
        fraction.len() == 3 && fraction.bytes().all(|byte| byte.is_ascii_digit()),
        "case {case:?}: {what} must carry a three-digit millisecond fraction, got {text:?}"
    );
    parsed.secs() * 1_000 + i64::from(parsed.subsec_nanos() / 1_000_000)
}

/// The two stamps of one entry, as instants, asserting both keys are present.
fn stamps(document: &Value, map: &str, key: &str, case: &str) -> (i64, i64) {
    let entry = entry(document, map, key, case);
    let created = entry
        .get(CREATED_AT_KEY)
        .unwrap_or_else(|| panic!("case {case:?}: ${map}.{key}.{CREATED_AT_KEY} is missing"));
    let updated = entry
        .get(UPDATED_AT_KEY)
        .unwrap_or_else(|| panic!("case {case:?}: ${map}.{key}.{UPDATED_AT_KEY} is missing"));
    (
        instant(created, &format!("${map}.{key}.{CREATED_AT_KEY}"), case),
        instant(updated, &format!("${map}.{key}.{UPDATED_AT_KEY}"), case),
    )
}

/// The stamps of one entry, verbatim, so a later version can be compared against them.
fn raw_stamps(document: &Value, map: &str, key: &str, case: &str) -> (String, String) {
    let entry = entry(document, map, key, case);
    let read = |member: &str| {
        entry
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: ${map}.{key}.{member} is missing"))
            .to_string()
    };
    (read(CREATED_AT_KEY), read(UPDATED_AT_KEY))
}

/// `document` with the two metadata keys removed from every entry of every stamped map,
/// so what the caller wrote can be compared with what came back.
///
/// `pub(crate)` because `control_plane_fidelity` asserts the same round trip for the
/// documents its schema rows span.
pub(crate) fn without_stamps(document: &Value) -> Value {
    let mut document = document.clone();
    let Some(object) = document.as_object_mut() else {
        return document;
    };
    for map in STAMPED_MAPS {
        if let Some(entries) = object.get_mut(*map).and_then(Value::as_object_mut) {
            for (_, entry) in entries.iter_mut() {
                if let Some(entry) = entry.as_object_mut() {
                    entry.remove(CREATED_AT_KEY);
                    entry.remove(UPDATED_AT_KEY);
                }
            }
        }
    }
    document
}

/// The keys of a stamped map in the submitted document.
fn keys_of(document: &Value, map: &str) -> BTreeSet<String> {
    document
        .get(map)
        .and_then(Value::as_object)
        .map(|entries| entries.keys().cloned().collect())
        .unwrap_or_default()
}

/// The current instant in milliseconds, for bounding the stamps a write produces.
fn now_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_millis(),
    )
    .expect("the instant fits in i64")
}

/// A first version written to a feature-flags profile comes back with both stamps on
/// every entry of `flags` and of `values`, equal to each other and to the write instant,
/// with nothing else about the document changed — and the stamped document, not the
/// submitted one, is what the profile stores.
#[tokio::test]
async fn a_feature_flag_version_is_stamped_on_every_flag_and_value_entry() {
    let router = super::appconfig_workflow::router();
    let application = create_application(&router, "flag-metadata").await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let profile = create_typed_profile(
            &router,
            &application,
            &format!("flags-{index}"),
            FEATURE_FLAGS_PROFILE_TYPE,
        )
        .await;

        let before = now_millis();
        let created = write_version(&router, &application, &profile, &row.document).await;
        let document = created_document(&created, case);
        let after = now_millis();

        for map in STAMPED_MAPS {
            let keys = keys_of(&row.document, map);
            for key in &keys {
                let (created_at, updated_at) = stamps(&document, map, key, case);
                assert_eq!(
                    created_at, updated_at,
                    "case {case:?}: ${map}.{key} is new, so its {CREATED_AT_KEY} and \
                     {UPDATED_AT_KEY} must be the same instant, got {created_at} and {updated_at}"
                );
                assert!(
                    (before..=after).contains(&created_at),
                    "case {case:?}: ${map}.{key}.{CREATED_AT_KEY} must be the write instant \
                     (between {before} and {after}), got {created_at}"
                );
            }
            // Only the entries the document declares are stamped: the helper adds no
            // entry of its own.
            let returned = keys_of(&document, map);
            assert_eq!(
                returned, keys,
                "case {case:?}: the stored {map} map must hold exactly the submitted keys, got \
                 {returned:?} for {keys:?}"
            );
        }

        // Nothing but the stamps differs from what the caller wrote.
        assert_eq!(
            without_stamps(&document),
            row.document,
            "case {case:?}: apart from the two metadata keys the stored document must match the \
             submitted one, got {document}"
        );

        // The STORED version is the stamped one, byte for byte, and reading it twice
        // reports the same stamps: the instants are persisted, not minted per response.
        let version = version_number(&created, case);
        for read in 1..=2 {
            let fetched = read_version(&router, &application, &profile, version).await;
            assert_eq!(
                fetched.status,
                200,
                "case {case:?}: GetHostedConfigurationVersion expected 200, got {} / {}",
                fetched.status,
                String::from_utf8_lossy(&fetched.body)
            );
            assert_eq!(
                fetched.body.as_ref(),
                created.body.as_ref(),
                "case {case:?}: read {read} must return the stamped bytes the create answered, \
                 got {}",
                String::from_utf8_lossy(&fetched.body)
            );
        }
    }
}

/// The stamps are a property of the entry's history in the profile, not of the call: a
/// later version keeps `_createdAt` for a key it already held, moves `_updatedAt` only
/// for an entry whose content changed, and stamps a newly added key with the new write's
/// instant.
#[tokio::test]
async fn later_versions_keep_created_at_and_move_updated_at_only_on_a_change() {
    let router = super::appconfig_workflow::router();
    let application = create_application(&router, "flag-history").await;
    let profile =
        create_typed_profile(&router, &application, "history", FEATURE_FLAGS_PROFILE_TYPE).await;

    let first = json!({
        "flags": {
            "keptflag": { "name": "keptflag" },
            "changedflag": { "name": "changedflag" },
        },
        "values": {
            "keptflag": { "enabled": true },
            "changedflag": { "enabled": true },
        },
        "version": "1",
    });
    let created = write_version(&router, &application, &profile, &first).await;
    let version_one = created_document(&created, "version 1");

    // Version 2 leaves `keptflag` exactly as it was and changes `changedflag` in both
    // maps, so the rule's two halves are exercised side by side in one write.
    let second = json!({
        "flags": {
            "keptflag": { "name": "keptflag" },
            "changedflag": { "name": "changedflag", "description": "now described" },
        },
        "values": {
            "keptflag": { "enabled": true },
            "changedflag": { "enabled": false },
        },
        "version": "1",
    });
    let before_second = now_millis();
    let created = write_version(&router, &application, &profile, &second).await;
    let version_two = created_document(&created, "version 2");

    for map in STAMPED_MAPS {
        let case = format!("{map}, unchanged entry");
        assert_eq!(
            raw_stamps(&version_two, map, "keptflag", &case),
            raw_stamps(&version_one, map, "keptflag", &case),
            "case {case:?}: an entry whose content did not change must keep both instants"
        );

        let case = format!("{map}, changed entry");
        let (first_created, first_updated) = stamps(&version_one, map, "changedflag", &case);
        let (next_created, next_updated) = stamps(&version_two, map, "changedflag", &case);
        assert_eq!(
            next_created, first_created,
            "case {case:?}: {CREATED_AT_KEY} must survive a change, got {next_created} for \
             {first_created}"
        );
        assert!(
            next_updated >= before_second && next_updated >= first_updated,
            "case {case:?}: {UPDATED_AT_KEY} must move to the new write's instant (at least \
             {before_second}), got {next_updated} after {first_updated}"
        );
    }

    // Version 3 adds a key and drops none: the new entry is stamped with this write's
    // instant while the entries that were already there keep theirs.
    let third = json!({
        "flags": {
            "keptflag": { "name": "keptflag" },
            "changedflag": { "name": "changedflag", "description": "now described" },
            "addedflag": { "name": "addedflag" },
        },
        "values": {
            "keptflag": { "enabled": true },
            "changedflag": { "enabled": false },
            "addedflag": { "enabled": true },
        },
        "version": "1",
    });
    let before_third = now_millis();
    let created = write_version(&router, &application, &profile, &third).await;
    let version_three = created_document(&created, "version 3");

    for map in STAMPED_MAPS {
        let case = format!("{map}, added entry");
        let (added_created, added_updated) = stamps(&version_three, map, "addedflag", &case);
        assert_eq!(
            added_created, added_updated,
            "case {case:?}: a key the profile never held is new, so both instants must match, \
             got {added_created} and {added_updated}"
        );
        assert!(
            added_created >= before_third,
            "case {case:?}: {CREATED_AT_KEY} must be this write's instant (at least \
             {before_third}), got {added_created}"
        );
        for key in ["keptflag", "changedflag"] {
            let case = format!("{map}, {key} beside an added entry");
            assert_eq!(
                raw_stamps(&version_three, map, key, &case),
                raw_stamps(&version_two, map, key, &case),
                "case {case:?}: adding another flag must not restamp an entry that did not change"
            );
        }
    }

    // Re-submitting the document the service returned — stamps and all — changes
    // nothing: the instants come from the profile's history, never from the request.
    let echoed = write_version(&router, &application, &profile, &version_three).await;
    let version_four = created_document(&echoed, "resubmitted stamped document");
    assert_eq!(
        version_four, version_three,
        "case \"resubmitted stamped document\": a caller's own stamps must not be stored, got \
         {version_four}"
    );
}

/// The other half of the rule: a profile that is not of the feature-flags `Type` stores
/// the submitted bytes unchanged, even when the content happens to be a feature-flag
/// document.
#[tokio::test]
async fn a_profile_of_another_type_stores_the_bytes_unchanged() {
    let router = super::appconfig_workflow::router();
    let application = create_application(&router, "freeform-content").await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let profile = create_typed_profile(
            &router,
            &application,
            &format!("freeform-{index}"),
            FREEFORM_PROFILE_TYPE,
        )
        .await;
        let submitted = serde_json::to_vec(&row.document).expect("the document must serialize");
        let created = write_version(&router, &application, &profile, &row.document).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert_eq!(
            created.body.as_ref(),
            submitted.as_slice(),
            "case {case:?}: a {FREEFORM_PROFILE_TYPE} profile must store the bytes unchanged, got \
             {}",
            String::from_utf8_lossy(&created.body)
        );
    }
}
