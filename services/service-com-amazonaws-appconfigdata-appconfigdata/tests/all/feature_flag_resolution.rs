// SPDX-License-Identifier: CC0-1.0

//! What a poll serves for a configuration profile whose `Type` AppConfig interprets.
//!
//! The rule under test, which is about the profile's `Type` and not about any one
//! document: the bytes a configuration version stores are the AUTHORING document, and
//! what the data plane serves is what that profile Type makes of them. A freeform
//! profile's content is the configuration itself, so its bytes are served unchanged; a
//! feature-flags profile's content is a flag-authoring document, and what is served is
//! the flag document resolved from it — the `values` half keyed by flag key at the top
//! level, with the `flags` definitions, the authoring schema `version` and the helper's
//! `_createdAt`/`_updatedAt` bookkeeping absent, and a flag that is off carrying nothing
//! but its `enabled`.
//!
//! The projection is computed from whatever version is in force, so it follows a later
//! deployment, and it never replaces the stored bytes: the authoring document a read of
//! the hosted version reports stays exactly as AppConfig stored it.
//!
//! Every case drives real requests through the same router harness the polling suite
//! uses ([`super::configuration_session_poll::Emulator`]), so the documents are written,
//! deployed and polled exactly as a client would.

use serde_json::{json, Value};
use service_com_amazonaws_appconfig_amazonappconfig::state::feature_flags::{
    CREATED_AT_KEY, ENABLED_KEY, FLAGS_PROPERTY, UPDATED_AT_KEY, VALUES_PROPERTY, VERSION_PROPERTY,
};
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    FEATURE_FLAGS_PROFILE_TYPE, FREEFORM_PROFILE_TYPE,
};

use super::configuration_session_poll::{Emulator, Hierarchy, Reply};

/// The MIME type a feature-flag document is written and served with.
const JSON: &str = "application/json";

/// One authoring document and the flag document a poll owes for it.
struct FlagCase {
    name: &'static str,
    /// The document a client writes to the feature-flags profile.
    authoring: &'static str,
    /// What a poll must serve for it, as JSON: the resolved flag document.
    served: &'static str,
}

/// Rows differ along what the rule generalises over: which flags are on and off, how
/// many flags the document declares, which attribute value types a flag carries,
/// whether the entry names `enabled` at all, and whether the document has a `values`
/// half or configures every flag it declares.
///
/// The attribute types in the "every attribute value type" row are the `type` values the
/// feature-flags Type reference lists for an attribute's `constraints` (string, number,
/// boolean, array).
const FLAG_CASES: &[FlagCase] = &[
    FlagCase {
        name: "one flag on with an attribute value, one flag off",
        authoring: r#"{"version":"1",
                       "flags":{"alpha":{"name":"alpha",
                                         "attributes":{"limit":{"constraints":{"type":"number"}}}},
                                "beta":{"name":"beta"}},
                       "values":{"alpha":{"enabled":true,"limit":7},
                                 "beta":{"enabled":false}}}"#,
        served: r#"{"alpha":{"enabled":true,"limit":7},"beta":{"enabled":false}}"#,
    },
    FlagCase {
        name: "a flag that is off drops the attribute values it was authored with",
        authoring: r#"{"version":"1",
                       "flags":{"ratio":{"name":"ratio","description":"a weighting",
                                         "attributes":{"weight":{"constraints":{"type":"number"}},
                                                       "label":{"constraints":{"type":"string"}}}}},
                       "values":{"ratio":{"enabled":false,"weight":3,"label":"off"}}}"#,
        served: r#"{"ratio":{"enabled":false}}"#,
    },
    FlagCase {
        name: "every attribute value type survives on a flag that is on",
        authoring: r#"{"version":"1",
                       "flags":{"mixed":{"name":"mixed",
                                         "attributes":{"note":{"constraints":{"type":"string"}},
                                                       "count":{"constraints":{"type":"number"}},
                                                       "loud":{"constraints":{"type":"boolean"}},
                                                       "regions":{"constraints":{"type":"array"}}}}},
                       "values":{"mixed":{"enabled":true,"note":"hello","count":0,"loud":false,
                                          "regions":["us-east-1","eu-west-1"]}}}"#,
        served: r#"{"mixed":{"enabled":true,"note":"hello","count":0,"loud":false,
                             "regions":["us-east-1","eu-west-1"]}}"#,
    },
    FlagCase {
        name: "a values entry that names no enabled is served off",
        authoring: r#"{"version":"1",
                       "flags":{"later":{"name":"later"}},
                       "values":{"later":{}}}"#,
        served: r#"{"later":{"enabled":false}}"#,
    },
    FlagCase {
        name: "a document with no values half serves an empty flag document",
        authoring: r#"{"version":"1","flags":{"alpha":{"name":"alpha"}}}"#,
        served: "{}",
    },
    FlagCase {
        name: "a declared flag the document configures no values for is not served",
        authoring: r#"{"version":"1",
                       "flags":{"alpha":{"name":"alpha"},"beta":{"name":"beta"}},
                       "values":{"alpha":{"enabled":true}}}"#,
        served: r#"{"alpha":{"enabled":true}}"#,
    },
    FlagCase {
        name: "four flags, on and off, with and without attribute values",
        authoring: r#"{"version":"1",
                       "flags":{"one":{"name":"one",
                                       "attributes":{"size":{"constraints":{"type":"number"}}}},
                                "two":{"name":"two"},
                                "three":{"name":"three",
                                         "attributes":{"mode":{"constraints":{"type":"string"}}}},
                                "four":{"name":"four"}},
                       "values":{"one":{"enabled":true,"size":1024},
                                 "two":{"enabled":false},
                                 "three":{"enabled":false,"mode":"shadow"},
                                 "four":{"enabled":true}}}"#,
        served: r#"{"one":{"enabled":true,"size":1024},
                    "two":{"enabled":false},
                    "three":{"enabled":false},
                    "four":{"enabled":true}}"#,
    },
];

/// Parse a document a case states, or a response body.
fn document(label: &str, bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|error| {
        panic!(
            "{label}: expected a JSON document, got {:?} ({error})",
            String::from_utf8_lossy(bytes)
        )
    })
}

/// The top-level key set of a JSON object, sorted.
fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// Read one hosted configuration version back, as `GetHostedConfigurationVersion`
/// reports it: the bytes AppConfig stored.
async fn stored_version(
    emulator: &Emulator,
    hierarchy: &Hierarchy,
    profile_id: &str,
    version: i32,
) -> Reply {
    let application_id = &hierarchy.application_id;
    emulator
        .send(
            "GET",
            &format!(
                "/applications/{application_id}/configurationprofiles/{profile_id}\
                 /hostedconfigurationversions/{version}"
            ),
            JSON,
            &[],
            bytes::Bytes::new(),
        )
        .await
}

/// Assert that `stored` is still the authoring document `authoring`, with the
/// feature-flags helper's stamps on every entry it stamps and nothing else changed.
fn assert_authoring_document_preserved(case: &str, authoring: &Value, stored: &Value) {
    assert_eq!(
        keys(stored),
        keys(authoring),
        "case {case}: the stored document must keep the authoring envelope, got {stored}"
    );
    for map_name in [FLAGS_PROPERTY, VALUES_PROPERTY] {
        let Some(entries) = authoring.get(map_name).and_then(Value::as_object) else {
            continue;
        };
        for (key, entry) in entries {
            let stored_entry = stored
                .get(map_name)
                .and_then(|map| map.get(key))
                .and_then(Value::as_object)
                .unwrap_or_else(|| {
                    panic!("case {case}: no stored {map_name}.{key} entry in {stored}")
                });
            for stamp in [CREATED_AT_KEY, UPDATED_AT_KEY] {
                assert!(
                    stored_entry.get(stamp).and_then(Value::as_str).is_some(),
                    "case {case}: stored {map_name}.{key} must carry {stamp}, got {:?}",
                    Value::Object(stored_entry.clone())
                );
            }
            let mut body = stored_entry.clone();
            body.remove(CREATED_AT_KEY);
            body.remove(UPDATED_AT_KEY);
            assert_eq!(
                Value::Object(body),
                *entry,
                "case {case}: the stored {map_name}.{key} entry must be the authored one with \
                 only the stamps added, got {:?}",
                Value::Object(stored_entry.clone())
            );
        }
    }
}

/// A feature-flags profile serves the resolved flag document, and keeps the authoring
/// document it stored.
#[tokio::test]
async fn a_feature_flags_profile_serves_the_resolved_flag_document() {
    for case in FLAG_CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let hierarchy = emulator.create_hierarchy("MyApp", "Prod", "Freeform").await;
        let profile_id = emulator
            .create_typed_profile(
                &hierarchy.application_id,
                "Flags",
                Some(FEATURE_FLAGS_PROFILE_TYPE),
            )
            .await;
        let authoring = document(name, case.authoring.as_bytes());
        let expected = document(name, case.served.as_bytes());

        emulator
            .deploy(&hierarchy, &profile_id, case.authoring.as_bytes(), JSON, None)
            .await;

        let token = emulator
            .session_token(
                &hierarchy.application_name,
                &hierarchy.environment_name,
                "Flags",
            )
            .await;
        let poll = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            poll.status,
            200,
            "case {name}: expected 200, got {} body {}",
            poll.status,
            poll.text()
        );
        let served = document(name, &poll.body);
        assert_eq!(
            served, expected,
            "case {name}: expected the resolved flag document {expected}, got {served}"
        );
        // The served document carries no part of the authoring envelope, whatever the
        // flag keys happen to be.
        for property in [FLAGS_PROPERTY, VALUES_PROPERTY, VERSION_PROPERTY] {
            assert!(
                authoring.get(property).is_none() || served.get(property).is_none(),
                "case {name}: the served document must not carry the authoring property \
                 {property}, got {served}"
            );
        }
        for stamp in [CREATED_AT_KEY, UPDATED_AT_KEY] {
            assert!(
                !poll.text().contains(stamp),
                "case {name}: the served document must carry no {stamp} bookkeeping, got {served}"
            );
        }
        // A flag that is off is served as nothing but its switch.
        for (flag, entry) in served.as_object().expect("the served document is an object") {
            if entry.get(ENABLED_KEY) == Some(&json!(false)) {
                assert_eq!(
                    keys(entry),
                    vec![ENABLED_KEY.to_string()],
                    "case {name}: flag {flag} is off, so it must be served as only {ENABLED_KEY}, \
                     got {entry}"
                );
            }
        }
        assert_eq!(
            poll.header("content-type"),
            Some(JSON),
            "case {name}: expected the version's content type, got {:?}",
            poll.header("content-type")
        );

        // The stored bytes are untouched: resolution happens on the way out.
        let stored = stored_version(&emulator, &hierarchy, &profile_id, 1).await;
        assert_eq!(
            stored.status,
            200,
            "case {name}: GetHostedConfigurationVersion expected 200, got {} body {}",
            stored.status,
            stored.text()
        );
        assert_authoring_document_preserved(name, &authoring, &document(name, &stored.body));
    }
}

/// The same documents in a FREEFORM profile are served byte for byte: the projection is
/// owed by the profile's `Type`, not by the shape of the content.
#[tokio::test]
async fn a_freeform_profile_serves_its_stored_bytes_unchanged() {
    for case in FLAG_CASES {
        let name = case.name;
        let emulator = Emulator::new();
        let hierarchy = emulator.create_hierarchy("MyApp", "Prod", "Other").await;
        for (label, declared_type) in [
            ("declared freeform", Some(FREEFORM_PROFILE_TYPE)),
            ("no declared type", None),
        ] {
            let profile = format!("Plain-{}", label.replace(' ', "-"));
            let profile_id = emulator
                .create_typed_profile(&hierarchy.application_id, &profile, declared_type)
                .await;
            emulator
                .deploy(&hierarchy, &profile_id, case.authoring.as_bytes(), JSON, None)
                .await;
            let token = emulator
                .session_token(
                    &hierarchy.application_name,
                    &hierarchy.environment_name,
                    &profile,
                )
                .await;
            let poll = emulator.get_latest_configuration(&token).await;
            assert_eq!(
                poll.status,
                200,
                "case {name} ({label}): expected 200, got {} body {}",
                poll.status,
                poll.text()
            );
            assert_eq!(
                poll.body.as_ref(),
                case.authoring.as_bytes(),
                "case {name} ({label}): a freeform profile serves its stored bytes, got {}",
                poll.text()
            );
        }
    }
}

/// The resolved document is recomputed per deployed version rather than frozen at the
/// first serve: deploying a version that flips the flags changes what a poll serves, and
/// a flag that has become disabled loses the attribute values it still carries in the
/// authoring document.
#[tokio::test]
async fn the_resolved_flag_document_follows_the_deployed_version() {
    let first = r#"{"version":"1",
                    "flags":{"alpha":{"name":"alpha",
                                      "attributes":{"limit":{"constraints":{"type":"number"}}}},
                             "beta":{"name":"beta"}},
                    "values":{"alpha":{"enabled":true,"limit":7},"beta":{"enabled":false}}}"#;
    let second = r#"{"version":"1",
                     "flags":{"alpha":{"name":"alpha",
                                       "attributes":{"limit":{"constraints":{"type":"number"}}}},
                              "beta":{"name":"beta"}},
                     "values":{"alpha":{"enabled":false,"limit":9},"beta":{"enabled":true}}}"#;
    let expected: [(&str, &str); 2] = [
        (first, r#"{"alpha":{"enabled":true,"limit":7},"beta":{"enabled":false}}"#),
        (second, r#"{"alpha":{"enabled":false},"beta":{"enabled":true}}"#),
    ];

    let emulator = Emulator::new();
    let hierarchy = emulator.create_hierarchy("MyApp", "Prod", "Freeform").await;
    let profile_id = emulator
        .create_typed_profile(
            &hierarchy.application_id,
            "Flags",
            Some(FEATURE_FLAGS_PROFILE_TYPE),
        )
        .await;

    for (index, (authoring, served)) in expected.iter().enumerate() {
        let case = format!("deployed version {}", index + 1);
        emulator
            .deploy(&hierarchy, &profile_id, authoring.as_bytes(), JSON, None)
            .await;
        // A FRESH session each time, as a client that has been served nothing yet polls.
        let token = emulator
            .session_token(
                &hierarchy.application_name,
                &hierarchy.environment_name,
                "Flags",
            )
            .await;
        let poll = emulator.get_latest_configuration(&token).await;
        assert_eq!(
            poll.status,
            200,
            "case {case}: expected 200, got {} body {}",
            poll.status,
            poll.text()
        );
        let actual = document(&case, &poll.body);
        let expected = document(&case, served.as_bytes());
        assert_eq!(
            actual, expected,
            "case {case}: expected the resolved flag document {expected}, got {actual}"
        );

        // And each version's own authoring document is still what AppConfig stored.
        let version = i32::try_from(index + 1).expect("the version number is small");
        let stored = stored_version(&emulator, &hierarchy, &profile_id, version).await;
        assert_eq!(
            stored.status,
            200,
            "case {case}: GetHostedConfigurationVersion expected 200, got {} body {}",
            stored.status,
            stored.text()
        );
        assert_authoring_document_preserved(
            &case,
            &document(&case, authoring.as_bytes()),
            &document(&case, &stored.body),
        );
    }
}
