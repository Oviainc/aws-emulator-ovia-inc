// SPDX-License-Identifier: CC0-1.0

//! Whether a reason this service reports fits the member it is reported on, end
//! to end over HTTP.
//!
//! RULE under test: a reason this service states on a modelled member is
//! rendered within that member's declared `@length` maximum. A reason that fits
//! is reported exactly as the service words it; one that would overrun — because
//! it names an identifier the model bounds as widely as, or more widely than,
//! the member it travels on — is reported at the bound, keeping its leading text
//! and its trailing clause so what it says about the request survives.
//!
//! The two maxima below are the model's, not this suite's: the per-entry
//! `errorMessage` of `BatchPutPropertyValues` is bounded at 256 characters while
//! the `propertyName` a reason names is bounded at 256, and every modelled
//! refusal of this service states its reason on the `ErrorMessage` shape,
//! bounded at 2048 — which a 2048-character `componentPath` overruns as soon as
//! any fixed text joins it.
//!
//! [`ERROR_MESSAGE_MAX`] and [`assert_within_bound`] are this crate's ONE
//! statement of that bound and of what the rule asserts about a reported
//! reason, so a suite about a refusal built some other way — one grown by the
//! caller rather than interpolating a single identifier
//! ([`crate::component_type_delete_in_use_message_bound`]) — asserts the rule
//! through them rather than restating it.
//!
//! The rows differ along what the rule generalises over: WHICH identifier the
//! reason interpolates (a `propertyName`, a `componentPath`), its SIZE (one
//! character, a value whose reason lands exactly on the member's maximum, one
//! past it, and the identifier's own modelled maximum), WHICH reason is reported
//! (a value of another data type than the property's, a value carrying no
//! instant, an instant that is no ISO 8601 DateTime, a path that resolves to
//! nothing) and WHICH operation reports it (the five operations that accept a
//! `componentPath`).

use serde_json::{json, Map, Value};

use crate::harness::{data_writer_functions, Emulator, Resp};

/// The greatest number of characters `BatchPutPropertyError$errorMessage`
/// admits: `@length(1..256)`, from
/// `smithy_query(query="shape",shape="BatchPutPropertyError")`.
const ENTRY_ERROR_MESSAGE_MAX: usize = 256;

/// The greatest number of characters the modelled `ErrorMessage` admits — the
/// member every exception of this service states its reason on:
/// `@length(0..2048)`, from `smithy_query(query="shape",shape="ErrorMessage")`.
pub(crate) const ERROR_MESSAGE_MAX: usize = 2048;

/// The greatest number of characters `EntityPropertyReference$componentPath`
/// admits (its target `ComponentPath` is `@length(1..2048)`), and
/// `EntityPropertyReference$propertyName` (its target `Name` is
/// `@length(1..256)`), from
/// `smithy_query(query="shape",shape="EntityPropertyReference")`.
const COMPONENT_PATH_MAX: usize = 2048;
/// See [`COMPONENT_PATH_MAX`].
const NAME_MAX: usize = 256;

/// How much of a reported reason's head and tail a case checks survived a cut.
/// Smaller than half of the smaller bound above, so it is a statement about
/// both ends of the message rather than about where exactly it was cut.
const SURVIVING: usize = 32;

/// The workspace every case below reads and writes.
const WORKSPACE: &str = "msgbounds";
/// The component type carrying every property named below.
const GAUGE: &str = "com.example.msgbounds";
/// The entity carrying a top-level [`GAUGE`] component named [`COMPONENT`].
const ENTITY: &str = "unit-1";
/// The component every reference below addresses.
const COMPONENT: &str = "Gauge";
/// The one property of [`GAUGE`] the readers below select.
const READING: &str = "Reading";
/// The instant every write below carries.
const T0: &str = "2026-04-01T00:00:00Z";

/// The reason `BatchPutPropertyValues` words for a value of another data type
/// than the property's, written out here rather than read from the service.
fn wrong_data_type(property_name: &str) -> String {
    format!(
        "the value written to property {property_name} is a STRING where that property's \
         dataType is DOUBLE"
    )
}

/// The reason it words for a value that names no instant at all.
fn no_instant(property_name: &str) -> String {
    format!("A value of property {property_name} carries neither time nor timestamp.")
}

/// The reason it words for an instant it cannot read.
fn unreadable_instant(property_name: &str, time: &str) -> String {
    format!(
        "The time {time} of a value of property {property_name} is not an ISO8601 DateTime."
    )
}

/// The reason the property data plane words for a `componentPath` the entity
/// holds no component at — one construction, shared by every operation of it.
fn component_not_found(component_path: &str) -> String {
    format!(
        "Component {component_path} of Entity {ENTITY} in workspace {WORKSPACE} not found."
    )
}

/// The reason the two listings word for the same path — their own construction,
/// on the same bounded member.
fn listing_path_unresolved(component_path: &str) -> String {
    format!(
        "entity {ENTITY} of workspace {WORKSPACE} has no component at componentPath \
         {component_path}"
    )
}

/// The TIME SERIES property names a value-level reason interpolates, each sized
/// so that the reason naming it lands on a different side of
/// [`ENTRY_ERROR_MESSAGE_MAX`].
fn series_names() -> Vec<(String, String)> {
    // The fixed text of the reason, so a row's size is derived from the bound
    // rather than counted by hand.
    let fixed = wrong_data_type("").chars().count();
    vec![
        (
            "a propertyName of the one character the modelled Name admits".to_string(),
            "L".to_string(),
        ),
        (
            "a propertyName whose reason renders exactly the characters the member admits"
                .to_string(),
            "a".repeat(ENTRY_ERROR_MESSAGE_MAX - fixed),
        ),
        (
            "a propertyName whose reason renders one character past the member's maximum"
                .to_string(),
            "b".repeat(ENTRY_ERROR_MESSAGE_MAX - fixed + 1),
        ),
        (
            "a propertyName of the characters the modelled Name caps at".to_string(),
            "c".repeat(NAME_MAX),
        ),
    ]
}

/// The workspace, component type and entity every case below uses.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let mut definitions = Map::new();
    definitions.insert(
        READING.to_string(),
        json!({"dataType": {"type": "DOUBLE"}, "isTimeSeries": true}),
    );
    for (_, name) in series_names() {
        definitions.insert(
            name,
            json!({"dataType": {"type": "DOUBLE"}, "isTimeSeries": true}),
        );
    }
    let created = emulator
        .create_component_type(
            WORKSPACE,
            GAUGE,
            json!({
                "propertyDefinitions": Value::Object(definitions),
                // The write connector a value is set THROUGH, so a write reaches
                // the per-entry channel this suite is about.
                "functions": data_writer_functions(),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType {GAUGE:?}: expected 200, got {created:?}"
    );

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": ENTITY,
                "entityName": ENTITY,
                "components": {COMPONENT: {"componentTypeId": GAUGE}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateEntity {ENTITY:?}: expected 200, got {created:?}"
    );
}

/// The last `count` characters of `text`.
pub(crate) fn tail(text: &str, count: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    characters[characters.len() - count.min(characters.len())..]
        .iter()
        .collect()
}

/// The first `count` characters of `text`.
pub(crate) fn head(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

/// Assert the rule over ONE reported message: `reported` is what the service
/// states on a member bounded at `max_chars` characters, where `whole` is the
/// reason the service words for this case.
pub(crate) fn assert_within_bound(case: &str, reported: &str, whole: &str, max_chars: usize) {
    let length = reported.chars().count();
    assert!(
        length <= max_chars && length >= 1,
        "case {case:?}: a reason is reported within the {max_chars}-character maximum of the \
         member it travels on, got {length} characters: {reported:?}"
    );
    if whole.chars().count() <= max_chars {
        assert_eq!(
            reported, whole,
            "case {case:?}: a reason that fits the member is reported whole, expected {whole:?}, \
             got {reported:?}"
        );
        return;
    }
    assert_eq!(
        length, max_chars,
        "case {case:?}: a reason too long for the member is reported at the bound, expected \
         {max_chars} characters, got {length}: {reported:?}"
    );
    assert_ne!(
        reported, whole,
        "case {case:?}: a reason too long for the member cannot be reported whole, got \
         {reported:?}"
    );
    assert!(
        reported.starts_with(&head(whole, SURVIVING)),
        "case {case:?}: a cut reason keeps its leading text, expected it to start with {:?}, got \
         {reported:?}",
        head(whole, SURVIVING)
    );
    assert!(
        reported.ends_with(&tail(whole, SURVIVING)),
        "case {case:?}: a cut reason keeps its trailing clause, expected it to end with {:?}, \
         got {reported:?}",
        tail(whole, SURVIVING)
    );
}

/// Every reported `BatchPutPropertyError` of a response, in order.
fn entry_errors(response: &Resp) -> Vec<Value> {
    response
        .member("errorEntries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|entry| entry["errors"].as_array().cloned().unwrap_or_default())
        .collect()
}

/// One refused entry of a batch write: the values it carries for a resolvable
/// reference, and the reason the service words for them.
#[derive(Debug)]
struct EntryCase {
    case: String,
    property_name: String,
    values: Value,
    whole: String,
}

/// The entries the rule is asserted over: each names a `propertyName` of a
/// different size, and the rows differ along the reason the entry's values earn.
fn entry_cases() -> Vec<EntryCase> {
    let mut cases: Vec<EntryCase> = series_names()
        .into_iter()
        .map(|(case, name)| EntryCase {
            case,
            property_name: name.clone(),
            values: json!([{"time": T0, "value": {"stringValue": "written"}}]),
            whole: wrong_data_type(&name),
        })
        .collect();

    // The other two value-level reasons, each on a `propertyName` at the
    // modelled maximum — the longest of the names the component type declares —
    // so both are past the member's own bound.
    let longest = "c".repeat(NAME_MAX);
    cases.push(EntryCase {
        case: "a value carrying no instant, on a propertyName of the modelled maximum".to_string(),
        property_name: longest.clone(),
        values: json!([{"value": {"doubleValue": 1.0}}]),
        whole: no_instant(&longest),
    });
    let unreadable = "not-a-timestamp-at-all";
    cases.push(EntryCase {
        case: "an unreadable instant, on a propertyName of the modelled maximum".to_string(),
        property_name: longest.clone(),
        values: json!([{"time": unreadable, "value": {"doubleValue": 1.0}}]),
        whole: unreadable_instant(&longest, unreadable),
    });

    cases
}

/// RULE: every `errorMessage` `BatchPutPropertyValues` reports on an entry whose
/// values it cannot record is within the 256 characters the model bounds that
/// member at, whatever the entry named — reported whole where it fits, and at the
/// bound with its leading text and trailing clause kept where it does not.
#[tokio::test]
async fn a_batch_entry_error_message_fits_its_modelled_member() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for entry_case in entry_cases() {
        let case = entry_case.case.as_str();
        // Every row's reference RESOLVES — the property is one the component
        // has, and its component type declares the write connector — so what
        // fails is the entry's values, which is the failure this operation
        // reports per entry.
        let written = emulator
            .batch_put_property_values(
                WORKSPACE,
                json!([{
                    "entityPropertyReference": {
                        "entityId": ENTITY,
                        "componentName": COMPONENT,
                        "propertyName": entry_case.property_name,
                    },
                    "propertyValues": entry_case.values,
                }]),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: an entry's values are refused in errorEntries, not by status: \
             got {written:?}"
        );
        let errors = entry_errors(&written);
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected one reported entry error, got {written:?}"
        );
        assert_eq!(
            errors[0]["errorCode"].as_str().unwrap_or_default(),
            "ValidationException",
            "case {case:?}: expected the ValidationException code, got {written:?}"
        );
        let reported = errors[0]["errorMessage"].as_str().unwrap_or_default();
        assert_within_bound(case, reported, &entry_case.whole, ENTRY_ERROR_MESSAGE_MAX);
    }
}

/// One operation that resolves a `componentPath` and refuses a path the entity
/// holds no component at, stating its reason on the modelled `ErrorMessage`.
#[derive(Debug, Clone, Copy)]
enum Reader {
    ListProperties,
    ListComponents,
    GetPropertyValue,
    GetPropertyValueHistory,
    BatchPutPropertyValues,
}

/// The five operations the rule is asserted over: the whole set that takes a
/// `componentPath` and answers an unresolvable one with an exception.
const READERS: [Reader; 5] = [
    Reader::ListProperties,
    Reader::ListComponents,
    Reader::GetPropertyValue,
    Reader::GetPropertyValueHistory,
    Reader::BatchPutPropertyValues,
];

impl Reader {
    /// Drive this operation with `component_path`, each request carrying the
    /// members its own model binds.
    async fn send(self, emulator: &Emulator, component_path: &str) -> Resp {
        match self {
            Reader::ListProperties => {
                emulator
                    .list_properties(
                        WORKSPACE,
                        json!({"entityId": ENTITY, "componentPath": component_path}),
                    )
                    .await
            }
            Reader::ListComponents => {
                emulator
                    .list_components(WORKSPACE, ENTITY, json!({"componentPath": component_path}))
                    .await
            }
            Reader::GetPropertyValue => {
                emulator
                    .get_property_value(
                        WORKSPACE,
                        json!({
                            "entityId": ENTITY,
                            "componentPath": component_path,
                            "selectedProperties": [READING],
                        }),
                    )
                    .await
            }
            Reader::GetPropertyValueHistory => {
                emulator
                    .get_property_value_history(
                        WORKSPACE,
                        json!({
                            "entityId": ENTITY,
                            "componentPath": component_path,
                            "selectedProperties": [READING],
                        }),
                    )
                    .await
            }
            Reader::BatchPutPropertyValues => {
                emulator
                    .batch_put_property_values(
                        WORKSPACE,
                        json!([{
                            "entityPropertyReference": {
                                "entityId": ENTITY,
                                "componentPath": component_path,
                                "propertyName": READING,
                            },
                            "propertyValues": [{"time": T0, "value": {"doubleValue": 1.0}}],
                        }]),
                    )
                    .await
            }
        }
    }

    /// The status and error code this operation answers an unresolvable
    /// `componentPath` with: the two listings report the component they hold
    /// nothing at as a missing resource, while the property data plane reads it
    /// as the bad REFERENCE it is.
    fn refusal(self) -> (u16, &'static str) {
        match self {
            Reader::ListProperties | Reader::ListComponents => {
                (404, "ResourceNotFoundException")
            }
            Reader::GetPropertyValue
            | Reader::GetPropertyValueHistory
            | Reader::BatchPutPropertyValues => (400, "ValidationException"),
        }
    }

    /// The reason this operation words for the path, in full — before the bound
    /// of the member it travels on is applied.
    fn whole(self, component_path: &str) -> String {
        match self {
            Reader::ListProperties | Reader::ListComponents => {
                listing_path_unresolved(component_path)
            }
            Reader::GetPropertyValue
            | Reader::GetPropertyValueHistory
            | Reader::BatchPutPropertyValues => component_not_found(component_path),
        }
    }
}

/// RULE: every operation that resolves a `componentPath` states its refusal
/// within the 2048 characters the modelled `ErrorMessage` admits, whatever path
/// the request carried.
#[tokio::test]
async fn a_reader_refusal_message_fits_its_modelled_member() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for reader in READERS {
        // The sizes are derived from the bound the reason travels on and from
        // the path's own modelled maximum, so a row is a statement about the
        // bound rather than a literal.
        let fixed = reader.whole("").chars().count();
        let paths: Vec<(String, String)> = vec![
            (
                "a path short enough for the reason to fit".to_string(),
                "Missing".to_string(),
            ),
            (
                "a path whose reason renders exactly the characters the member admits".to_string(),
                "g".repeat(ERROR_MESSAGE_MAX - fixed),
            ),
            (
                "a path whose reason renders one character past the member's maximum".to_string(),
                "h".repeat(ERROR_MESSAGE_MAX - fixed + 1),
            ),
            (
                "a path of the characters the modelled ComponentPath caps at".to_string(),
                "i".repeat(COMPONENT_PATH_MAX),
            ),
        ];

        for (size_case, path) in &paths {
            let case = format!("{reader:?} with {size_case}");
            let (status, error) = reader.refusal();
            let refused = reader.send(&emulator, path).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (status, Some(error)),
                "case {case:?}: expected {status} {error}, got {refused:?}"
            );
            let reported = refused.member("message").as_str().unwrap_or_default().to_string();
            assert_within_bound(&case, &reported, &reader.whole(path), ERROR_MESSAGE_MAX);
        }
    }
}
