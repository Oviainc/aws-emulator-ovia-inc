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
//! the `propertyName` and `componentPath` a reason names are bounded at 256 and
//! 2048, and every modelled refusal of this service states its reason on the
//! `ErrorMessage` shape, bounded at 2048 — which a 2048-character
//! `componentPath` overruns as soon as any fixed text joins it.
//!
//! The rows differ along what the rule generalises over: WHICH identifier the
//! reason interpolates (a `propertyName`, a `componentPath`, an `entityId`), its
//! SIZE (one character, a value whose reason lands exactly on the member's
//! maximum, one past it, and the identifier's own modelled maximum), WHICH
//! reason is reported (a property that is not a time series, a property the
//! component does not have, a path that resolves to nothing, an entity the
//! workspace does not hold) and WHICH operation reports it (the five operations
//! that accept a `componentPath`).

use serde_json::{json, Map, Value};

use crate::harness::{Emulator, Resp};

/// The greatest number of characters `BatchPutPropertyError$errorMessage`
/// admits: `@length(1..256)`, from
/// `smithy_query(query="shape",shape="BatchPutPropertyError")`.
const ENTRY_ERROR_MESSAGE_MAX: usize = 256;

/// The greatest number of characters the modelled `ErrorMessage` admits — the
/// member every exception of this service states its reason on:
/// `@length(0..2048)`, from `smithy_query(query="shape",shape="ErrorMessage")`.
const ERROR_MESSAGE_MAX: usize = 2048;

/// The greatest number of characters `EntityPropertyReference$componentPath`
/// admits (its target `ComponentPath` is `@length(1..2048)`), and
/// `EntityPropertyReference$propertyName` (its target `Name` is
/// `@length(1..256)`), from
/// `smithy_query(query="shape",shape="EntityPropertyReference")`.
const COMPONENT_PATH_MAX: usize = 2048;
/// See [`COMPONENT_PATH_MAX`].
const NAME_MAX: usize = 256;
/// The greatest number of characters `EntityPropertyReference$entityId` admits
/// (its target `EntityId` is `@length(1..128)`), from the same shape.
const ENTITY_ID_MAX: usize = 128;

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
/// The one property [`GAUGE`] marks as a time series.
const READING: &str = "Reading";
/// The instant every write below carries.
const T0: &str = "2026-04-01T00:00:00Z";

/// The clause a refusal of a property that is not a time series ends in; the
/// reason the message gives, which a cut must not take away.
const NOT_A_TIME_SERIES: &str = " is not marked as a time-series property.";

/// The reason `BatchPutPropertyValues` words for a property that is not marked
/// as a time series, written out here rather than read from the service.
fn not_a_time_series(property_name: &str) -> String {
    format!("{property_name}{NOT_A_TIME_SERIES}")
}

/// The reason `BatchPutPropertyValues` words for a `componentPath` the entity
/// holds no component at.
fn entry_path_unresolved(component_path: &str) -> String {
    format!(
        "Entity {ENTITY} in workspace {WORKSPACE} has no component at componentPath \
         {component_path}."
    )
}

/// The reason the four request-shaped readers word for a `componentPath` the
/// entity holds no component at — one construction, shared by all of them.
fn reader_path_unresolved(component_path: &str) -> String {
    format!(
        "entity {ENTITY} of workspace {WORKSPACE} has no component at componentPath \
         {component_path}"
    )
}

/// The names [`GAUGE`] declares as NOT time-series properties, each sized so
/// that the reason naming it lands on a different side of
/// [`ENTRY_ERROR_MESSAGE_MAX`].
fn non_series_names() -> Vec<(String, String)> {
    // The fixed text of the reason, so a row's size is derived from the bound
    // rather than counted by hand.
    let fixed = NOT_A_TIME_SERIES.chars().count();
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
    for (_, name) in non_series_names() {
        definitions.insert(name, json!({"dataType": {"type": "STRING"}}));
    }
    let created = emulator
        .create_component_type(
            WORKSPACE,
            GAUGE,
            json!({"propertyDefinitions": Value::Object(definitions)}),
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
fn tail(text: &str, count: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    characters[characters.len() - count.min(characters.len())..]
        .iter()
        .collect()
}

/// The first `count` characters of `text`.
fn head(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

/// Assert the rule over ONE reported message: `reported` is what the service
/// states on a member bounded at `max_chars` characters, where `whole` is the
/// reason the service words for this case.
fn assert_within_bound(case: &str, reported: &str, whole: &str, max_chars: usize) {
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

/// One refused entry of a batch write: the reference it carries, the code it
/// earns and the reason the service words for it.
#[derive(Debug)]
struct EntryCase {
    case: String,
    reference: Value,
    code: &'static str,
    whole: String,
}

/// The entries the rule is asserted over: each names an identifier of a
/// different kind and size, and earns a different reason.
fn entry_cases() -> Vec<EntryCase> {
    let mut cases: Vec<EntryCase> = non_series_names()
        .into_iter()
        .map(|(case, name)| EntryCase {
            case,
            reference: json!({
                "entityId": ENTITY,
                "componentName": COMPONENT,
                "propertyName": name,
            }),
            code: "ValidationException",
            whole: not_a_time_series(&name),
        })
        .collect();

    // A property the component does not have at all, named at the modelled
    // maximum: a second construction over the same bound.
    let absent_property = "d".repeat(NAME_MAX);
    cases.push(EntryCase {
        case: "a propertyName the component does not have, of the modelled maximum".to_string(),
        reference: json!({
            "entityId": ENTITY,
            "componentName": COMPONENT,
            "propertyName": absent_property.clone(),
        }),
        code: "ValidationException",
        whole: format!(
            "Property {absent_property} is not a property of component {COMPONENT} of entity \
             {ENTITY}."
        ),
    });

    // A componentPath that resolves to nothing: short enough to fit, and at the
    // modelled maximum of the path itself.
    for (case, path) in [
        (
            "a componentPath the entity holds no component at, short enough to fit".to_string(),
            "Missing".to_string(),
        ),
        (
            "a componentPath the entity holds no component at, of the modelled maximum"
                .to_string(),
            "e".repeat(COMPONENT_PATH_MAX),
        ),
    ] {
        cases.push(EntryCase {
            case,
            reference: json!({
                "entityId": ENTITY,
                "componentPath": path,
                "propertyName": READING,
            }),
            code: "ResourceNotFoundException",
            whole: entry_path_unresolved(&path),
        });
    }

    // An entityId the workspace does not hold, at ITS modelled maximum: the
    // reason fits, so it is reported whole.
    let absent_entity = "f".repeat(ENTITY_ID_MAX);
    cases.push(EntryCase {
        case: "an entityId the workspace does not hold, of the modelled maximum".to_string(),
        reference: json!({
            "entityId": absent_entity.clone(),
            "componentName": COMPONENT,
            "propertyName": READING,
        }),
        code: "ResourceNotFoundException",
        whole: format!("Entity {absent_entity} in workspace {WORKSPACE} not found."),
    });

    cases
}

/// RULE: every `errorMessage` `BatchPutPropertyValues` reports on a refused
/// entry is within the 256 characters the model bounds that member at, whatever
/// the entry named — reported whole where it fits, and at the bound with its
/// leading text and trailing clause kept where it does not.
#[tokio::test]
async fn a_batch_entry_error_message_fits_its_modelled_member() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for entry_case in entry_cases() {
        let case = entry_case.case.as_str();
        let written = emulator
            .batch_put_property_values(
                WORKSPACE,
                json!([{
                    "entityPropertyReference": entry_case.reference,
                    "propertyValues": [{"time": T0, "value": {"stringValue": "written"}}],
                }]),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: an entry is refused in errorEntries, not by status: got {written:?}"
        );
        let errors = entry_errors(&written);
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected one reported entry error, got {written:?}"
        );
        assert_eq!(
            errors[0]["errorCode"].as_str().unwrap_or_default(),
            entry_case.code,
            "case {case:?}: expected the {} code, got {written:?}",
            entry_case.code
        );
        let reported = errors[0]["errorMessage"].as_str().unwrap_or_default();
        assert_within_bound(case, reported, &entry_case.whole, ENTRY_ERROR_MESSAGE_MAX);
    }
}

/// One request-shaped operation that resolves a `componentPath` and refuses a
/// path the entity holds no component at.
#[derive(Debug, Clone, Copy)]
enum Reader {
    ListProperties,
    ListComponents,
    GetPropertyValue,
    GetPropertyValueHistory,
}

/// The four readers the rule is asserted over: the whole set of operations that
/// take a `componentPath` and answer with an exception rather than with a
/// per-entry error.
const READERS: [Reader; 4] = [
    Reader::ListProperties,
    Reader::ListComponents,
    Reader::GetPropertyValue,
    Reader::GetPropertyValueHistory,
];

impl Reader {
    /// Drive this reader with `component_path`, each request carrying the
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
        }
    }
}

/// RULE: every reader that resolves a `componentPath` states its refusal within
/// the 2048 characters the modelled `ErrorMessage` admits, whatever path the
/// request carried.
#[tokio::test]
async fn a_reader_refusal_message_fits_its_modelled_member() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The sizes are derived from the bound and from the path's own modelled
    // maximum, so a row is a statement about the bound rather than a literal.
    let fixed = reader_path_unresolved("").chars().count();
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

    for reader in READERS {
        for (size_case, path) in &paths {
            let case = format!("{reader:?} with {size_case}");
            let refused = reader.send(&emulator, path).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected 404 ResourceNotFoundException, got {refused:?}"
            );
            let reported = refused.member("message").as_str().unwrap_or_default().to_string();
            assert_within_bound(
                &case,
                &reported,
                &reader_path_unresolved(path),
                ERROR_MESSAGE_MAX,
            );
        }
    }
}
