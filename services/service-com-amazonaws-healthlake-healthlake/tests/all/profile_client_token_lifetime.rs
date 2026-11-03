// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of a `CreateDataTransformationProfile` `ClientToken`
//! ACROSS the delete of the profile it minted, over the real awsJson1_0 wire.
//!
//! The rule under test: an idempotency token record never outlives the resource it
//! minted, so removing the resource drops every token that names it — a later
//! create carrying such a token is a FRESH request (neither replayed with the gone
//! record nor refused as a reuse), and the token's slot in the bounded token store
//! is given back. While the profile is still held the token keeps its full
//! idempotency force: a create carrying it is ignored and answered with the
//! profile it minted, whether or not that create repeats the original request.
//!
//! The rows differ along what the rule generalises over: the source format, how many
//! mapping entries and optional members the minting request carried, the token's own
//! form at both ends of the modelled `ClientToken` length range (1..64 characters,
//! `^[a-zA-Z0-9-]+$`), and whether the create that reuses the token after the delete
//! repeats the original request byte for byte or differs from it.
//!
//! Every call below goes through the shared [`Profiles`] harness, i.e.
//! `MultiServiceRouter::dispatch` — the same entry point `core/router` serves — so
//! routing, deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    MAX_PROFILE_CLIENT_TOKENS, profile_version_not_found_message,
};

use crate::data_transformation_profile::{
    assert_well_formed_profile_id, mapping, mapping_of, modelled_message, profile_mapping_source,
    reported_mapping, Profiles,
};
use crate::fhir_datastore_lifecycle::string_at;

/// One row: the request a token mints its profile from, and the token's form.
struct Row {
    case: &'static str,
    /// The `ClientToken` the minting create carries, without the per-probe suffix
    /// the two probes below append so each probe gets a token of its own.
    token: String,
    source_format: &'static str,
    profile_name: &'static str,
    initial: BTreeMap<String, String>,
    /// The optional members the minting create carries, merged into the request.
    optional: Value,
}

/// The table. The modelled bounds the rows sit on come from the operation's own
/// shapes: `ClientToken` is len 1..64 matching `^[a-zA-Z0-9-]+$`, `ProfileName` is
/// len 1..256, `ProfileDescription` len 0..1000 and `ProfileMapping` holds 1..500
/// entries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "CSV, one mapping entry, a token at the modelled 1-character minimum",
            token: "t".to_string(),
            source_format: "CSV",
            profile_name: "token-minimal",
            initial: mapping(&[("patients.csv.map", "Patient.id <- patient_id")]),
            optional: json!({}),
        },
        Row {
            case: "CCDA, several entries, every optional member, a hyphenated token",
            token: "tok-with-hyphens-0001".to_string(),
            source_format: "CCDA",
            profile_name: "token-full",
            initial: mapping(&[
                ("sections/problems.map", "Condition.code <- problem/value"),
                ("sections/vitals.map", "Observation.value[x] <- vital/value"),
            ]),
            optional: json!({
                "ProfileDescription": "C-CDA sections to FHIR R4",
                "KmsKeyId": "alias/healthlake-profiles",
                "Tags": { "team": "clinical-data", "env": "test" },
            }),
        },
        Row {
            // 64 characters, the modelled maximum, minus the 2-character probe
            // suffix appended below.
            case: "CSV, many mapping entries, a token at the modelled 64-character maximum",
            token: format!("{}{}", "a".repeat(31), "9".repeat(31)),
            source_format: "CSV",
            profile_name: "token-at-maximum",
            initial: mapping_of(24),
            optional: json!({ "Tags": { "only": "" } }),
        },
    ]
}

/// The create request one row mints its profile from, under `profile_name`.
fn create_request(row: &Row, token: &str, profile_name: &str) -> Value {
    let mut request = json!({
        "ProfileName": profile_name,
        "SourceFormat": row.source_format,
        "ClientToken": token,
        "Source": profile_mapping_source(&row.initial),
    });
    let object = request.as_object_mut().expect("the request is an object");
    for (member, value) in row.optional.as_object().expect("the members are an object") {
        object.insert(member.clone(), value.clone());
    }
    request
}

/// Read a profile's DRAFT back, or `None` when no record holds that id — asserting
/// that a missing one is refused with the version this read NAMED, named in the
/// sentence beside the profile.
async fn read_draft(healthlake: &Profiles, case: &str, profile_id: &str) -> Option<Value> {
    let (status, payload) = healthlake
        .call(
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "ProfileVersion": 0 }),
        )
        .await;
    if (200..300).contains(&status) {
        return Some(payload);
    }
    let expected = profile_version_not_found_message(profile_id, 0);
    assert_eq!(
        (status, modelled_message(&payload)),
        (wire_http_status(404), Some(expected.as_str())),
        "case {case:?}: a profile no record holds is refused with the sentence naming \
         it, got {payload}"
    );
    None
}

/// Mint one row's profile with `token`, check the token's idempotency force WHILE the
/// profile is held, and return the `ProfileId` it minted.
async fn mint_and_check_live_token(
    healthlake: &Profiles,
    row: &Row,
    token: &str,
    profile_name: &str,
) -> String {
    let case = row.case;
    let request = create_request(row, token, profile_name);
    let first = healthlake
        .ok(case, "CreateDataTransformationProfile", &request)
        .await;
    let profile_id = string_at(&first, "ProfileId");
    assert_well_formed_profile_id(case, &profile_id);

    // While the record is held the token replays it...
    let repeat = healthlake
        .ok(case, "CreateDataTransformationProfile", &request)
        .await;
    assert_eq!(
        repeat, first,
        "case {case:?}: while the profile is held, the token replays what the first \
         call answered"
    );
    // ...and the same token on a DIFFERENT request is ignored and answered with
    // the profile that token minted, as the modelled `ClientToken` documents
    // ("the service ignores the request but does not return an error").
    let mut altered = request.clone();
    altered["ProfileName"] = json!(format!("{profile_name}-renamed"));
    let ignored = healthlake
        .ok(case, "CreateDataTransformationProfile", &altered)
        .await;
    assert_eq!(
        ignored, first,
        "case {case:?}: while the profile is held, a create under its token answers \
         what the first call answered"
    );

    profile_id
}

/// Each row's token is dropped with the profile it minted: after the delete the same
/// token mints a NEW profile, whether the create that reuses it repeats the original
/// request or differs from it.
#[tokio::test]
async fn a_deleted_profiles_client_token_is_free_again() {
    let healthlake = Profiles::new();

    for row in rows() {
        let case = row.case;
        // `repeated` reuses the token with the byte-for-byte original request;
        // `altered` reuses it with a different one. Each probe gets its own token so
        // the two cannot interfere.
        for (probe, suffix) in [("the same request", "-r"), ("a different request", "-a")] {
            let token = format!("{}{suffix}", row.token);
            let name = format!("{}{suffix}", row.profile_name);
            let minted = mint_and_check_live_token(&healthlake, &row, &token, &name).await;

            let deleted = healthlake
                .ok(
                    case,
                    "DeleteDataTransformationProfile",
                    &json!({ "ProfileId": minted }),
                )
                .await;
            assert_eq!(
                deleted["ProfileId"].as_str(),
                Some(minted.as_str()),
                "case {case:?} / {probe}: the delete reports the profile it removed, \
                 got {deleted}"
            );
            assert!(
                read_draft(&healthlake, case, &minted).await.is_none(),
                "case {case:?} / {probe}: the deleted profile {minted} is gone"
            );

            // The reuse: a create carrying the token of a profile that no longer
            // exists is a fresh request, so it is neither refused nor answered with
            // the gone record.
            let reuse_name = match probe {
                "the same request" => name.clone(),
                _ => format!("{name}-after-delete"),
            };
            let reuse = create_request(&row, &token, &reuse_name);
            let (status, payload) = healthlake
                .call("CreateDataTransformationProfile", &reuse)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} / {probe}: a create carrying the token of a deleted \
                 profile is accepted, got {status}: {payload}"
            );
            let reminted = string_at(&payload, "ProfileId");
            assert_well_formed_profile_id(case, &reminted);
            assert_ne!(
                reminted, minted,
                "case {case:?} / {probe}: the reuse mints a NEW profile rather than \
                 resolving to the deleted {minted}"
            );
            assert_eq!(
                payload["ProfileName"].as_str(),
                Some(reuse_name.as_str()),
                "case {case:?} / {probe}: the new profile carries the reusing \
                 request's name, got {payload}"
            );

            // The new profile is a real record: its DRAFT reads back with the content
            // the reusing request carried, and the token now names IT.
            let draft = read_draft(&healthlake, case, &reminted)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?} / {probe}: the new profile {reminted} is readable")
                });
            assert_eq!(
                reported_mapping(case, &draft),
                row.initial,
                "case {case:?} / {probe}: the new profile's DRAFT holds the content \
                 the reusing create carried"
            );
            let replay = healthlake
                .ok(case, "CreateDataTransformationProfile", &reuse)
                .await;
            assert_eq!(
                replay["ProfileId"].as_str(),
                Some(reminted.as_str()),
                "case {case:?} / {probe}: the token now replays the profile it minted \
                 second, got {replay}"
            );
        }
    }
}

/// A deleted profile gives its TOKEN slot back too: a create/delete loop as long as
/// the bounded token store cannot evict a live profile's token, so that profile's own
/// token still replays afterwards.
#[tokio::test]
async fn a_create_delete_loop_does_not_evict_a_live_profiles_token() {
    let healthlake = Profiles::new();
    let case = "a create/delete loop as long as the token store's bound";
    let keeper = json!({
        "ProfileName": "the-live-profile",
        "SourceFormat": "CCDA",
        "ClientToken": "keeper-token",
        "Source": profile_mapping_source(&mapping(&[(
            "sections/problems.map",
            "Condition.code <- problem/value",
        )])),
    });
    let kept = healthlake
        .ok(case, "CreateDataTransformationProfile", &keeper)
        .await;
    let kept_id = string_at(&kept, "ProfileId");

    // Every cycle mints a profile under its own token and deletes it again, so the
    // store never holds more than two profiles while the loop runs.
    for index in 0..MAX_PROFILE_CLIENT_TOKENS {
        let created = healthlake
            .ok(
                case,
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": format!("transient-{index:04}"),
                    "SourceFormat": "CSV",
                    "ClientToken": format!("transient-token-{index:04}"),
                    "Source": profile_mapping_source(&mapping(&[(
                        "patients.csv.map",
                        "Patient.id <- patient_id",
                    )])),
                }),
            )
            .await;
        let transient_id = string_at(&created, "ProfileId");
        healthlake
            .ok(
                case,
                "DeleteDataTransformationProfile",
                &json!({ "ProfileId": transient_id }),
            )
            .await;
    }

    let replay = healthlake
        .ok(case, "CreateDataTransformationProfile", &keeper)
        .await;
    assert_eq!(
        replay, kept,
        "case {case:?}: the live profile's token survived {MAX_PROFILE_CLIENT_TOKENS} \
         create/delete cycles and still replays the profile it minted"
    );
    let draft = read_draft(&healthlake, case, &kept_id)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the live profile {kept_id} is still readable"));
    assert_eq!(
        draft["ProfileId"].as_str(),
        Some(kept_id.as_str()),
        "case {case:?}: the replay answered with the live profile, got {draft}"
    );
}
