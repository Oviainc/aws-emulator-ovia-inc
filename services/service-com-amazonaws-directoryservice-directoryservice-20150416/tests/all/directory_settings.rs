// SPDX-License-Identifier: CC0-1.0

//! A directory's configurable settings — `DescribeSettings` — driven end to end over
//! HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.DescribeSettings` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the directory-lifecycle suite's,
//! reached as `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from
//! the state struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * the directory a read names is resolved in the store, and a well-formed id no
//!   directory is stored under is refused with this operation's OWN not-found class,
//!   `DirectoryDoesNotExistException` — which is a DIFFERENT class from the
//!   `EntityDoesNotExistException` the reads that resolve something WITHIN a directory
//!   answer for the same id, so the two families cannot be collapsed;
//! * a directory that carries settings reports one entry per setting, each with the value
//!   applied to the directory, the values it allows, its data type and its status — read
//!   back from the store rather than from the request, and identical across two
//!   directories of the same type;
//! * `Status` narrows the entries: the status every fresh setting is in reports them all,
//!   and every other value of the modelled `DirectoryConfigurationStatus` enum reports
//!   none;
//! * this operation issues no `NextToken`, so every token a request can carry is one it
//!   did not issue and is refused with `InvalidNextTokenException`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// Well-formed `DirectoryId`s no directory is ever created under, as `(id, class)`.
///
/// The form comes from the model: `DirectoryId` is `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="operation", operation="DescribeSettings")`), so every row here is
/// a value the boundary validator admits and the handler must resolve in the store.
const ABSENT_DIRECTORY_IDS: [(&str, &str); 4] = [
    ("d-0000000000", "all zeros, the low end of the alphabet"),
    ("d-ffffffffff", "all f, the high end of the alphabet"),
    ("d-abcdef0123", "every hex digit once"),
    ("d-da73f21c73", "digits and letters interleaved"),
];

/// The whole modelled `DirectoryConfigurationStatus` catalog
/// (`smithy_query(query="shape",shape="DirectoryConfigurationStatus")` answers
/// `["Requested", "Updating", "Updated", "Failed", "Default"]`) — every value a `Status`
/// filter can carry.
const STATUS_CATALOG: [&str; 5] = ["Requested", "Updating", "Updated", "Failed", "Default"];

/// The status a setting nothing has changed is in: the one value of that catalog a freshly
/// created directory's settings carry.
const STATUS_DEFAULT: &str = "Default";

/// The `SettingEntries` of an answer.
fn entries(body: &Value, case: &str) -> Vec<Value> {
    body["SettingEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected SettingEntries, got {body}"))
        .clone()
}

/// The settings of `directory_id`, with no filter.
async fn settings_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> Value {
    ok(
        router,
        "DescribeSettings",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await
}

/// RULE: the id a read names is resolved in the store, and an id no directory is stored
/// under is refused with the directory's OWN not-found class — distinct from the class the
/// reads that resolve an entity INSIDE a directory answer.
#[tokio::test]
async fn an_id_naming_no_directory_is_refused_with_the_directory_not_found_class() {
    let router = service();

    // One stored directory in the same store, so a refusal cannot come from an empty one.
    let managed = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("corp.example.com"),
        "the stored directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (absent, class) in ABSENT_DIRECTORY_IDS {
        let case = format!("DescribeSettings of {absent} ({class})");
        let (status, parsed) =
            call(&router, "DescribeSettings", json!({ "DirectoryId": absent })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("DirectoryDoesNotExistException"),
            "case {case:?}: expected DirectoryDoesNotExistException, got {parsed}"
        );
        // The two ds not-found families are distinct: a read that resolves an entity
        // INSIDE a directory answers the entity class for the very same id.
        let (_, inside) = call(
            &router,
            "DescribeTrusts",
            json!({ "DirectoryId": absent }),
        )
        .await;
        let inside_type = inside["__type"].as_str().unwrap_or_default();
        assert!(
            inside_type.contains("EntityDoesNotExistException"),
            "case {case:?}: expected the entity class from a read inside a directory, \
             got {inside}"
        );
        assert_ne!(
            wire_type, inside_type,
            "case {case:?}: expected the two not-found families to differ, got {wire_type}"
        );
        // The message names the id the request asked for and nothing else.
        let message = parsed["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(absent),
            "case {case:?}: expected the refusal to name the requested id, got {message:?}"
        );
    }

    // The refusals changed nothing: the stored directory still reports its settings.
    let answer = settings_of(&router, &managed, "after the refusals").await;
    assert!(
        !entries(&answer, "after the refusals").is_empty(),
        "case \"after the refusals\": expected the stored directory's settings, got {answer}"
    );
}

/// RULE: a directory that carries settings reports one entry per setting, each read back
/// from the store with the value applied to the directory, the values it allows, its data
/// type and its status.
#[tokio::test]
async fn a_directorys_settings_are_reported_from_persisted_state() {
    let router = service();

    // Two directories of the type that carries settings, created differently (one minimal,
    // one with an edition, a short name and a network type), so a per-directory answer
    // cannot come from the request.
    let first = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("first.example.com"),
        "the minimal directory",
    )
    .await;
    let second = create_microsoft_ad(
        &router,
        json!({
            "Name": "second.example.com",
            "Password": crate::directory_lifecycle::PASSWORD,
            "Edition": "Standard",
            "ShortName": "SECOND",
            "NetworkType": "IPv4",
            "VpcSettings": {
                "VpcId": "vpc-11223344",
                "SubnetIds": ["subnet-11223344", "subnet-55667788"]
            }
        }),
        "the Standard directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let mut catalogues: Vec<Vec<String>> = Vec::new();
    for directory in [&first, &second] {
        let case = format!("the settings of {directory}");
        let answer = settings_of(&router, directory, &case).await;
        assert_eq!(
            answer["DirectoryId"],
            json!(directory),
            "case {case:?}: expected the answer to name the directory read, got {answer}"
        );
        let found = entries(&answer, &case);
        assert!(
            !found.is_empty(),
            "case {case:?}: expected at least one setting entry, got {answer}"
        );
        let mut names: Vec<String> = Vec::new();
        for entry in &found {
            let name = entry["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Name, got {entry}"));
            names.push(name.to_string());
            // Every member a stored setting is rendered from, asserted per entry rather
            // than per directory, so an entry missing one cannot pass.
            for member in ["Type", "AllowedValues", "AppliedValue", "DataType"] {
                assert!(
                    entry[member].as_str().is_some_and(|value| !value.is_empty()),
                    "case {case:?}/{name}: expected a non-empty {member}, got {entry}"
                );
            }
            // The value last requested for a setting nothing has requested anything for
            // is the value applied to it, and the status is the one a fresh setting is in.
            assert_eq!(
                entry["RequestedValue"], entry["AppliedValue"],
                "case {case:?}/{name}: expected the requested value to be the applied \
                 one, got {entry}"
            );
            assert_eq!(
                entry["RequestStatus"],
                json!(STATUS_DEFAULT),
                "case {case:?}/{name}: expected the status of an unchanged setting, \
                 got {entry}"
            );
            // The applied value is one of the values the entry says it allows, which the
            // member carries as a JSON array of strings.
            let allowed: BTreeSet<String> = serde_json::from_str::<Vec<String>>(
                entry["AllowedValues"].as_str().unwrap_or_default(),
            )
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}/{name}: expected AllowedValues to decode as a JSON array \
                     of strings, got {entry} ({error})"
                )
            })
            .into_iter()
            .collect();
            let applied = entry["AppliedValue"].as_str().unwrap_or_default();
            assert!(
                allowed.contains(applied),
                "case {case:?}/{name}: expected the applied value {applied:?} among the \
                 allowed ones {allowed:?}"
            );
            assert!(
                entry["LastUpdatedDateTime"].is_number(),
                "case {case:?}/{name}: expected a LastUpdatedDateTime, got {entry}"
            );
            // Nothing has requested an update, so the members that describe one stay
            // absent rather than reporting a request nobody made.
            for member in [
                "RequestDetailedStatus",
                "RequestStatusMessage",
                "LastRequestedDateTime",
            ] {
                assert_eq!(
                    entry[member],
                    json!(null),
                    "case {case:?}/{name}: expected no {member} on a setting no update \
                     was requested for, got {entry}"
                );
            }
        }
        // One entry per setting: no name repeats.
        let distinct: BTreeSet<&String> = names.iter().collect();
        assert_eq!(
            distinct.len(),
            names.len(),
            "case {case:?}: expected one entry per setting, got {names:?}"
        );
        catalogues.push(names);
    }

    // The settings a directory carries are the service's own, so two directories of one
    // type carry the same ones.
    assert_eq!(
        catalogues[0], catalogues[1],
        "case \"two directories of one type\": expected the same settings, got {catalogues:?}"
    );

    // A directory type that carries no settings is refused rather than served an empty
    // list, the way every other read of a type-specific feature of this service refuses.
    let simple = create_directory(
        &router,
        minimal_create("simple.example.com"),
        "the Simple AD directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    refused(
        &router,
        "DescribeSettings",
        json!({ "DirectoryId": &simple }),
        "UnsupportedOperationException",
        "DescribeSettings of a Simple AD directory",
    )
    .await;
}

/// RULE: `Status` narrows the entries to the settings in that status, and a token this
/// operation did not issue is refused.
#[tokio::test]
async fn the_status_filter_and_an_unissued_next_token() {
    let router = service();

    let managed = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("filtered.example.com"),
        "the directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let all = entries(
        &settings_of(&router, &managed, "unfiltered").await,
        "unfiltered",
    );

    // Every value of the modelled catalog: the status a fresh setting is in reports every
    // entry, and every other status reports none — an empty list, not a refusal, because
    // the member is a filter.
    for status in STATUS_CATALOG {
        let case = format!("Status={status}");
        let answer = ok(
            &router,
            "DescribeSettings",
            json!({ "DirectoryId": &managed, "Status": status }),
            &case,
        )
        .await;
        let found = entries(&answer, &case);
        let expected = if status == STATUS_DEFAULT { all.len() } else { 0 };
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} entries, got {found:?}"
        );
        // No answer carries a page token: the whole catalogue fits in one page.
        assert_eq!(
            answer["NextToken"],
            json!(null),
            "case {case:?}: expected no NextToken, got {answer}"
        );
    }

    // No token this operation issued exists, so every token a request can carry is one it
    // did not issue — whatever shape it takes.
    for (token, class) in [
        ("", "the empty token"),
        ("not-a-token", "text that is not hex"),
        ("6e6f70", "hex that decodes to something else"),
        (
            "76313a73657474696e67733a30",
            "a token shaped like this service's own cursor",
        ),
    ] {
        let case = format!("NextToken {class}");
        refused(
            &router,
            "DescribeSettings",
            json!({ "DirectoryId": &managed, "NextToken": token }),
            "InvalidNextTokenException",
            &case,
        )
        .await;
    }
}

/// The one entry of `entries` named `name`.
fn entry_named(found: &[Value], name: &str, case: &str) -> Value {
    found
        .iter()
        .find(|entry| entry["Name"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: expected an entry named {name}, got {found:?}"))
        .clone()
}

/// The whole `UnsupportedSettingsException` diagnostic for a setting name the catalogue
/// does not publish: the clause naming what the request sent, restated behind the inner
/// exception that raised it.
fn unsupported_setting_message(name: &str) -> String {
    let stated = format!("Setting name: {name} is not currently supported");
    format!("{stated} with Inner Exception: UnsupportedSettingException: {stated}")
}

/// The whole `InvalidParameterException` diagnostic for a value a setting does not admit,
/// stated the same way: the pair the request sent, restated behind the inner exception.
fn invalid_setting_value_message(name: &str, value: &str) -> String {
    let stated =
        format!("Invalid setting value found for Setting name: {name} and value: {value}");
    format!("{stated} with Inner Exception: InvalidParameterException: {stated}")
}

/// RULE: `UpdateSettings` requests a value a setting admits — the value becomes the one
/// last REQUESTED at once and the one APPLIED to the directory when the transition the
/// update owes falls due — and it touches only the settings the request named.
#[tokio::test]
async fn an_update_requests_a_value_the_setting_admits_and_leaves_its_neighbours_alone() {
    let router = service();
    let managed = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("updated.example.com"),
        "the directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let before = entries(
        &settings_of(&router, &managed, "before any update").await,
        "before any update",
    );
    assert!(
        before.len() > 1,
        "case \"before any update\": expected more than one setting so an update has a \
         neighbour to leave alone, got {before:?}"
    );

    // Every setting the catalogue publishes, updated one at a time to every value it
    // admits BESIDES the one it carries, so the rows span the settings and their values
    // rather than one recorded pair.
    for subject in &before {
        let name = subject["Name"].as_str().unwrap_or_default().to_string();
        let applied = subject["AppliedValue"].as_str().unwrap_or_default().to_string();
        let allowed: Vec<String> = serde_json::from_str(
            subject["AllowedValues"].as_str().unwrap_or_default(),
        )
        .unwrap_or_default();
        for value in allowed.iter().filter(|value| **value != applied) {
            let case = format!("{name} -> {value}");
            // What every setting of the directory looks like just before this update, so
            // the neighbours are compared against the configuration this row started from.
            let baseline = entries(
                &settings_of(&router, &managed, &case).await,
                &case,
            );
            let answer = ok(
                &router,
                "UpdateSettings",
                json!({
                    "DirectoryId": &managed,
                    "Settings": [{ "Name": &name, "Value": value }],
                }),
                &case,
            )
            .await;
            assert_eq!(
                answer["DirectoryId"],
                json!(&managed),
                "case {case:?}: expected the answer to name the directory updated, got {answer}"
            );

            // Read back straight away: the directory has ACCEPTED the request and not
            // carried it out yet, so the value requested is the new one while the value
            // applied is still the old one.
            let requested = entry_named(
                &entries(&settings_of(&router, &managed, &case).await, &case),
                &name,
                &case,
            );
            assert_eq!(
                requested["RequestedValue"],
                json!(value),
                "case {case:?}: expected the requested value to be stored, got {requested}"
            );
            assert_eq!(
                requested["AppliedValue"],
                json!(&applied),
                "case {case:?}: expected the applied value to wait for the transition, got \
                 {requested}"
            );
            assert_eq!(
                requested["RequestStatus"],
                json!("Requested"),
                "case {case:?}: expected the status of an accepted request, got {requested}"
            );
            assert!(
                requested["LastRequestedDateTime"].is_number(),
                "case {case:?}: expected the instant the request was submitted, got {requested}"
            );

            // Once the transition the update owes has fallen due, the value requested is
            // the value applied — and no later call was needed to make it so.
            tokio::time::sleep(PAST_ONE_SPAN).await;
            let settled_in = entries(&settings_of(&router, &managed, &case).await, &case);
            let settled = entry_named(&settled_in, &name, &case);
            assert_eq!(
                settled["AppliedValue"],
                json!(value),
                "case {case:?}: expected the requested value to be applied, got {settled}"
            );
            assert_eq!(
                settled["RequestStatus"],
                json!("Updated"),
                "case {case:?}: expected the status of a carried-out request, got {settled}"
            );

            // The settings the request did not name are untouched: same value, same
            // status, and the same request instant they had before this update.
            for neighbour in &settled_in {
                if neighbour["Name"] == json!(&name) {
                    continue;
                }
                let other = neighbour["Name"].as_str().unwrap_or_default();
                let was = entry_named(&baseline, other, &case);
                assert_eq!(
                    neighbour, &was,
                    "case {case:?}: expected {other} to be untouched, got {neighbour}"
                );
            }

            // Put the setting back, so the next row starts from the catalogue's default.
            ok(
                &router,
                "UpdateSettings",
                json!({
                    "DirectoryId": &managed,
                    "Settings": [{ "Name": &name, "Value": &applied }],
                }),
                &format!("{case} (restored)"),
            )
            .await;
            tokio::time::sleep(PAST_ONE_SPAN).await;
        }
    }
}

/// RULE: an update is refused — and changes NOTHING — when it names a setting this service
/// publishes none of, or a value outside the ones that setting admits, or a directory that
/// cannot carry settings or is not in service.
#[tokio::test]
async fn an_update_this_service_cannot_carry_out_is_refused_and_persists_nothing() {
    let router = service();
    let managed = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("refused.example.com"),
        "the directory",
    )
    .await;
    // A second directory still being built by the transition its create owes.
    let pending = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("pending.example.com"),
        "the directory still being built",
    )
    .await;
    let (_, pending_refusal) = call(
        &router,
        "UpdateSettings",
        json!({
            "DirectoryId": &pending,
            "Settings": [{ "Name": "TLS_1_0", "Value": "Disable" }],
        }),
    )
    .await;
    assert!(
        pending_refusal["__type"]
            .as_str()
            .unwrap_or_default()
            .contains("DirectoryUnavailableException"),
        "case \"a directory still being built\": expected the unavailable refusal, got \
         {pending_refusal}"
    );
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let before = entries(
        &settings_of(&router, &managed, "before the refusals").await,
        "before the refusals",
    );
    let name = before[0]["Name"].as_str().unwrap_or_default().to_string();
    let applied = before[0]["AppliedValue"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let other = before[1]["Name"].as_str().unwrap_or_default().to_string();

    // (case, the payload, the modelled error it is refused with, the whole diagnostic it
    // states). The unsupported names span a name of another shape, a name whose spelling
    // differs from a published one only in case, and a name that extends one — none of them
    // is in the catalogue `DescribeSettings` reports.
    let rows: Vec<(String, Value, &str, String)> = vec![
        (
            "a setting name of another shape".to_string(),
            json!([{ "Name": "NOT_A_SETTING", "Value": "Enable" }]),
            "UnsupportedSettingsException",
            unsupported_setting_message("NOT_A_SETTING"),
        ),
        (
            format!("{name} in another case"),
            json!([{ "Name": name.to_lowercase(), "Value": "Enable" }]),
            "UnsupportedSettingsException",
            unsupported_setting_message(&name.to_lowercase()),
        ),
        (
            format!("a name extending {name}"),
            json!([{ "Name": format!("{name}_1"), "Value": "Enable" }]),
            "UnsupportedSettingsException",
            unsupported_setting_message(&format!("{name}_1")),
        ),
        (
            "a value outside the ones the setting admits".to_string(),
            json!([{ "Name": &name, "Value": "Maybe" }]),
            "InvalidParameterException",
            invalid_setting_value_message(&name, "Maybe"),
        ),
        (
            "an admitted value in another case".to_string(),
            json!([{ "Name": &name, "Value": applied.to_uppercase() }]),
            "InvalidParameterException",
            invalid_setting_value_message(&name, &applied.to_uppercase()),
        ),
        (
            "one admissible setting beside one that is not".to_string(),
            json!([
                { "Name": &other, "Value": "Disable" },
                { "Name": "NOT_A_SETTING", "Value": "Enable" },
            ]),
            "UnsupportedSettingsException",
            unsupported_setting_message("NOT_A_SETTING"),
        ),
        (
            "one admissible setting beside a value that is not".to_string(),
            json!([
                { "Name": &other, "Value": "Disable" },
                { "Name": &name, "Value": "Perhaps" },
            ]),
            "InvalidParameterException",
            invalid_setting_value_message(&name, "Perhaps"),
        ),
    ];

    for (case, settings, expected, diagnostic) in &rows {
        let (status, parsed) = call(
            &router,
            "UpdateSettings",
            json!({ "DirectoryId": &managed, "Settings": settings }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains(expected),
            "case {case:?}: expected {expected}, got {parsed}"
        );
        let message = parsed["Message"].as_str().unwrap_or_default();
        let request_id = parsed["RequestId"].as_str().unwrap_or_default();
        assert!(
            !request_id.is_empty(),
            "case {case:?}: expected a RequestId, got {parsed}"
        );
        assert_eq!(
            message,
            format!("{diagnostic} : RequestId: {request_id}"),
            "case {case:?}: expected the diagnostic to name what the request sent, got \
             {message:?}"
        );
    }

    // No refused payload wrote anything — not even the admissible half of one.
    let after = entries(
        &settings_of(&router, &managed, "after the refusals").await,
        "after the refusals",
    );
    assert_eq!(
        after, before,
        "case \"after the refusals\": expected every setting untouched, got {after:?}"
    );

    // A directory whose type carries no settings cannot have them updated either.
    let simple = create_directory(
        &router,
        minimal_create("no-settings.example.com"),
        "the Simple AD directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    refused(
        &router,
        "UpdateSettings",
        json!({
            "DirectoryId": &simple,
            "Settings": [{ "Name": &name, "Value": "Disable" }],
        }),
        "UnsupportedOperationException",
        "UpdateSettings on a Simple AD directory",
    )
    .await;

    // And an id no directory is stored under answers the directory's own not-found class.
    for (absent, class) in ABSENT_DIRECTORY_IDS {
        refused(
            &router,
            "UpdateSettings",
            json!({
                "DirectoryId": absent,
                "Settings": [{ "Name": &name, "Value": "Disable" }],
            }),
            "DirectoryDoesNotExistException",
            &format!("UpdateSettings of {absent} ({class})"),
        )
        .await;
    }
}
