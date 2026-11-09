// SPDX-License-Identifier: CC0-1.0

//! The device-capability document `GetDevice` publishes, over the WHOLE inventory.
//!
//! The rule under test: a published device's `deviceCapabilities` document carries the
//! Braket device-schema properties a client introspects the device through — per
//! action type, the operations, result types and pragmas a program may use and the
//! schema versions the action accepts in their published `major.minor` form; and in
//! the `service` section, the shot range, what a run costs and where the device is
//! documented — each value taken from that device's own catalog row.
//!
//! The table is the seeded inventory itself: its rows differ in device type, status,
//! accepted action types, shot range, native gate set, result types, noise support and
//! cost unit, so a document that is a fixed stand-in rather than the device's own row
//! fails here. Every assertion goes over HTTP through the production router.

use serde_json::Value;

use crate::quantum_task_loop::{member, Braket};

/// The action types this inventory's devices accept, from
/// `service_com_amazonaws_braket_braket::state`.
use service_com_amazonaws_braket_braket::state::{ACTION_AHS, ACTION_JAQCD, ACTION_OPENQASM};

/// The GATE-MODEL program actions: only a program of gates names operations, result
/// types or pragmas, so only these entries publish them.
const GATE_MODEL_ACTIONS: &[&str] = &[ACTION_OPENQASM, ACTION_JAQCD];

/// The units a `deviceCost` may be charged per.
///
/// Source: the `deviceCost` property of the Braket device-service-properties schema,
/// whose unit is the pricing unit Amazon Braket's pricing page publishes — simulators
/// are charged per minute of simulation, QPUs per shot and per task.
const COST_UNITS: &[&str] = &["minute", "hour", "shot", "task"];

/// The prefix of the pragmas that ask for a result type.
const RESULT_TYPE_PRAGMA_PREFIX: &str = "braket_result_type_";
/// The prefix of the pragmas that apply a noise channel.
const NOISE_PRAGMA_PREFIX: &str = "braket_noise_";

/// A result-type name as the OpenQASM pragma that requests it: the Braket pragma
/// names are the result-type name in snake_case under [`RESULT_TYPE_PRAGMA_PREFIX`],
/// so `StateVector` is `braket_result_type_state_vector`. Derived here independently
/// of the service, so the service and the test cannot agree on a wrong name.
fn result_type_pragma(result_type: &str) -> String {
    let mut snake = String::with_capacity(result_type.len() + 4);
    for (index, ch) in result_type.chars().enumerate() {
        if ch.is_ascii_uppercase() && index > 0 {
            snake.push('_');
        }
        snake.push(ch.to_ascii_lowercase());
    }
    format!("{RESULT_TYPE_PRAGMA_PREFIX}{snake}")
}

/// The named member as an array, or a panic naming the case.
fn array<'v>(value: &'v Value, name: &str, case: &str) -> &'v Vec<Value> {
    value
        .get(name)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected an array member {name}, got {value}"))
}

/// Every element of the named array member, as strings.
fn string_array(value: &Value, name: &str, case: &str) -> Vec<String> {
    array(value, name, case)
        .iter()
        .map(|element| {
            element
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected {name} to hold strings, got {element}")
                })
                .to_string()
        })
        .collect()
}

/// Which key a device must publish its computing properties under, derived from the
/// action families its document accepts: a device that computes with gates or with an
/// analog Hamiltonian publishes a `paradigm`, and a device of any other family (an
/// annealer, a photonic processor) has no gate-model paradigm, so what it publishes is
/// its provider's own properties.
fn expected_properties_key(actions: &serde_json::Map<String, Value>) -> &'static str {
    let computes_in_a_paradigm = actions
        .keys()
        .any(|action| GATE_MODEL_ACTIONS.contains(&action.as_str()) || action == ACTION_AHS);
    if computes_in_a_paradigm {
        "paradigm"
    } else {
        "provider"
    }
}

/// The device's own computing-properties section, under the key its family publishes it
/// as: a `paradigm` for a gate-model or analog device, the provider's own properties for
/// a family that has no gate-model paradigm.
fn device_properties<'v>(document: &'v Value, case: &str) -> &'v Value {
    document
        .get("paradigm")
        .or_else(|| document.get("provider"))
        .unwrap_or_else(|| {
            panic!("case {case}: the document publishes no device properties: {document}")
        })
}

/// The parsed capability document of one device, read back over HTTP.
async fn capabilities(braket: &Braket, device_arn: &str) -> Value {
    let reply = braket.get_device(device_arn).await;
    assert_eq!(
        reply.status, 200,
        "case device {device_arn}: GetDevice must answer 200, got {} ({:?}) {}",
        reply.status, reply.error, reply.body
    );
    let document = member(&reply.body, "deviceCapabilities").to_string();
    serde_json::from_str(&document).unwrap_or_else(|e| {
        panic!("case device {device_arn}: deviceCapabilities must be JSON ({e}): {document}")
    })
}

/// Every published device's `service` section carries the shot range, the cost and the
/// documentation properties a client reads, each from that device's catalog row.
#[tokio::test]
async fn the_service_section_publishes_the_cost_and_documentation_properties() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;
    assert!(
        inventory.len() >= 2,
        "the seeded inventory must hold at least two devices to span the rule, got {}",
        inventory.len()
    );

    let mut prices: Vec<String> = Vec::new();
    let mut summaries: Vec<String> = Vec::new();

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let case = format!("device {arn}");
        let document = capabilities(&braket, arn).await;
        let service = document
            .get("service")
            .unwrap_or_else(|| panic!("case {case}: no service section: {document}"));

        assert_eq!(
            member(
                service
                    .get("braketSchemaHeader")
                    .unwrap_or_else(|| panic!("case {case}: no service schema header: {service}")),
                "name"
            ),
            "braket.device_schema.device_service_properties",
            "case {case}: the service section names its own schema, got {service}"
        );

        assert!(
            !array(service, "executionWindows", &case).is_empty(),
            "case {case}: expected at least one execution window, got {service}"
        );

        let shots = array(service, "shotsRange", &case);
        let bounds: Vec<i64> = shots.iter().filter_map(Value::as_i64).collect();
        assert_eq!(
            bounds.len(),
            2,
            "case {case}: shotsRange is [min, max] of integers, got {shots:?}"
        );
        assert!(
            bounds[0] <= bounds[1],
            "case {case}: expected min <= max in shotsRange, got {bounds:?}"
        );

        // deviceCost: a price and the unit it is charged per.
        let cost = service
            .get("deviceCost")
            .unwrap_or_else(|| panic!("case {case}: no deviceCost: {service}"));
        let price = cost
            .get("price")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: deviceCost.price must be a number: {cost}"));
        assert!(
            price > 0.0 && price.is_finite(),
            "case {case}: expected a positive finite price, got {price}"
        );
        let unit = member(cost, "unit");
        assert!(
            COST_UNITS.contains(&unit),
            "case {case}: expected a published cost unit from {COST_UNITS:?}, got {unit:?}"
        );
        prices.push(format!("{price}/{unit}"));

        // deviceDocumentation: the summary plus both urls.
        let documentation = service
            .get("deviceDocumentation")
            .unwrap_or_else(|| panic!("case {case}: no deviceDocumentation: {service}"));
        let text = member(documentation, "summary");
        assert!(
            !text.trim().is_empty(),
            "case {case}: expected a non-empty documentation summary, got {text:?}"
        );
        summaries.push(text.to_string());
        for url_member in ["imageUrl", "externalDocumentationUrl"] {
            let url = member(documentation, url_member);
            assert!(
                url.starts_with("https://"),
                "case {case}: expected {url_member} to be an https url, got {url:?}"
            );
        }

        // updatedAt: when the catalog entry a caller is reading was published.
        let updated_at = member(service, "updatedAt");
        assert!(
            updated_at.len() >= "2024-01-01T00:00:00Z".len()
                && updated_at.contains('T')
                && updated_at.ends_with('Z'),
            "case {case}: expected updatedAt as an ISO-8601 instant, got {updated_at:?}"
        );
    }

    // A document built from the device's own row, not one stand-in served for all:
    // the catalog's costs and summaries differ across the inventory.
    summaries.sort();
    summaries.dedup();
    assert_eq!(
        summaries.len(),
        inventory.len(),
        "each device documents itself: expected {} distinct summaries, got {}",
        inventory.len(),
        summaries.len()
    );
    prices.sort();
    prices.dedup();
    assert!(
        prices.len() >= 2,
        "the catalog prices devices differently: expected at least two distinct \
         deviceCost values across the inventory, got {prices:?}"
    );
}

/// Every action entry carries its action type and the `major.minor` schema versions it
/// accepts, and a gate-model entry carries the operations, result types and pragmas a
/// program may use — while an entry of any other action family, whose problems and
/// programs name no gates or pragmas, carries none of them.
#[tokio::test]
async fn every_action_entry_publishes_the_properties_its_schema_defines() {
    let braket = Braket::new();
    let inventory = braket.inventory().await;

    let mut native_gate_sets: Vec<String> = Vec::new();
    let mut gate_model_entries = 0usize;
    let mut other_family_entries = 0usize;
    let mut declared_schemas: Vec<String> = Vec::new();

    for summary in &inventory {
        let arn = member(summary, "deviceArn");
        let is_qpu = member(summary, "deviceType") == "QPU";
        let document = capabilities(&braket, arn).await;
        let actions = document
            .get("action")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case device {arn}: no action map: {document}"));
        assert!(
            !actions.is_empty(),
            "case device {arn}: expected at least one action type, got {document}"
        );

        // The properties go under the key the device's FAMILY publishes them as, and
        // under that key only: a gate-model device publishing a provider section, or an
        // annealer publishing a paradigm, is not the document its schema defines.
        let expected_key = expected_properties_key(actions);
        let other_key = if expected_key == "paradigm" {
            "provider"
        } else {
            "paradigm"
        };
        assert!(
            document.get(expected_key).is_some() && document.get(other_key).is_none(),
            "case device {arn}: the action types {:?} are published under {expected_key}, \
             and not under {other_key}, got {document}",
            actions.keys().collect::<Vec<_>>()
        );

        // Each section of the document names the schema it is an instance of, so a
        // client knows which device-schema model to read it with.
        for (section, header) in [
            ("document", document.get("braketSchemaHeader")),
            (
                "device properties",
                device_properties(&document, &format!("device {arn}")).get("braketSchemaHeader"),
            ),
        ] {
            let header = header.unwrap_or_else(|| {
                panic!("case device {arn}: the {section} names no schema: {document}")
            });
            let name = member(header, "name").to_string();
            assert!(
                name.starts_with("braket.device_schema."),
                "case device {arn} {section}: expected a braket device schema name, got {name:?}"
            );
            if section == "document" {
                declared_schemas.push(name);
            }
        }

        // deviceParameters is the schema a client validates its task parameters
        // against, so it is a schema document rather than an empty object.
        let parameters = document
            .get("deviceParameters")
            .unwrap_or_else(|| panic!("case device {arn}: no deviceParameters: {document}"));
        assert!(
            parameters.get("$schema").and_then(Value::as_str).is_some()
                && parameters.get("type").and_then(Value::as_str) == Some("object"),
            "case device {arn}: expected deviceParameters to be a JSON Schema object, \
             got {parameters}"
        );

        let paradigm = device_properties(&document, &format!("device {arn}"));
        let native_gates: Vec<String> = paradigm
            .get("nativeGateSet")
            .map(|_| string_array(paradigm, "nativeGateSet", &format!("device {arn}")))
            .unwrap_or_default();
        native_gate_sets.push(native_gates.join(","));

        for (action_type, entry) in actions {
            let case = format!("device {arn} action {action_type}");
            assert_eq!(
                member(entry, "actionType"),
                action_type,
                "case {case}: the entry names the action type it is keyed by, got {entry}"
            );

            // The action `version` is the LIST of program schema versions the action
            // accepts, each a `major.minor` BraketSchemas version.
            let versions = string_array(entry, "version", &case);
            assert!(
                !versions.is_empty(),
                "case {case}: expected at least one accepted schema version, got {entry}"
            );
            for version in &versions {
                let parts: Vec<&str> = version.split('.').collect();
                assert!(
                    parts.len() == 2
                        && parts
                            .iter()
                            .all(|part| !part.is_empty()
                                && part.chars().all(|c| c.is_ascii_digit())),
                    "case {case}: expected a major.minor schema version, got {version:?}"
                );
            }

            if !GATE_MODEL_ACTIONS.contains(&action_type.as_str()) {
                other_family_entries += 1;
                for absent in [
                    "supportedOperations",
                    "supportedPragmas",
                    "forbiddenPragmas",
                    "supportedResultTypes",
                ] {
                    assert!(
                        entry.get(absent).is_none(),
                        "case {case}: a program that names no gates has no pragmas or result \
                         types either, so {absent} must be absent, got {entry}"
                    );
                }
                continue;
            }

            gate_model_entries += 1;
            let operations = string_array(entry, "supportedOperations", &case);
            assert!(
                !operations.is_empty(),
                "case {case}: expected the operations a program may use, got {entry}"
            );
            for gate in &native_gates {
                assert!(
                    operations.contains(gate),
                    "case {case}: the native gate {gate:?} must be an operation a program \
                     may use, but supportedOperations is {operations:?}"
                );
            }

            // supportedResultTypes: what the device computes, and the shots each needs.
            let result_types = array(entry, "supportedResultTypes", &case).clone();
            let mut names: Vec<String> = Vec::new();
            for result_type in &result_types {
                let name = member(result_type, "name").to_string();
                let min_shots = result_type
                    .get("minShots")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| {
                        panic!("case {case}: result type {name} needs minShots: {result_type}")
                    });
                let max_shots = result_type
                    .get("maxShots")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| {
                        panic!("case {case}: result type {name} needs maxShots: {result_type}")
                    });
                assert!(
                    min_shots <= max_shots && min_shots >= 0,
                    "case {case} result type {name}: expected 0 <= minShots <= maxShots, \
                     got {min_shots}..{max_shots}"
                );
                let device_max = array(
                    document.get("service").expect("a service section"),
                    "shotsRange",
                    &case,
                )[1]
                .as_i64()
                .expect("an integer shot bound");
                assert!(
                    max_shots <= device_max,
                    "case {case} result type {name}: a result type cannot need more shots \
                     than the device accepts ({device_max}), got {max_shots}"
                );
                names.push(name);
            }

            if action_type != ACTION_OPENQASM {
                // Pragmas are an OpenQASM concept; the legacy action declares none.
                continue;
            }

            let supported = string_array(entry, "supportedPragmas", &case);
            let forbidden = string_array(entry, "forbiddenPragmas", &case);
            assert!(
                !supported.is_empty() && !forbidden.is_empty(),
                "case {case}: expected both pragma lists to be published, got \
                 supported {supported:?} forbidden {forbidden:?}"
            );
            for pragma in &supported {
                assert!(
                    !forbidden.contains(pragma),
                    "case {case}: {pragma:?} cannot be both supported and forbidden \
                     (forbidden is {forbidden:?})"
                );
            }

            // A result-type pragma is supported exactly where the device publishes the
            // result type it asks for.
            for name in &names {
                let pragma = result_type_pragma(name);
                assert!(
                    supported.contains(&pragma),
                    "case {case}: result type {name} is published, so {pragma:?} must be \
                     a supported pragma, got {supported:?}"
                );
            }
            for pragma in &forbidden {
                if let Some(suffix) = pragma.strip_prefix(RESULT_TYPE_PRAGMA_PREFIX) {
                    let forbidden_type = names
                        .iter()
                        .find(|name| result_type_pragma(name) == *pragma)
                        .map(String::as_str);
                    assert!(
                        forbidden_type.is_none(),
                        "case {case}: {pragma:?} is forbidden, so the result type it asks \
                         for ({suffix}) must not be published, got {names:?}"
                    );
                }
            }

            // The noise pragmas are an all-or-nothing family: a device either models
            // noise or it does not.
            let noise_supported = supported
                .iter()
                .filter(|pragma| pragma.starts_with(NOISE_PRAGMA_PREFIX))
                .count();
            let noise_forbidden = forbidden
                .iter()
                .filter(|pragma| pragma.starts_with(NOISE_PRAGMA_PREFIX))
                .count();
            assert!(
                (noise_supported == 0) != (noise_forbidden == 0),
                "case {case}: the noise pragmas belong to one list or the other, got \
                 {noise_supported} supported and {noise_forbidden} forbidden"
            );

            // The array limits the OpenQASM action schema defines.
            for limit in ["maximumQubitArrays", "maximumClassicalArrays"] {
                let value = entry
                    .get(limit)
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("case {case}: {limit} must be a number: {entry}"));
                assert!(
                    value >= 1,
                    "case {case}: expected {limit} to admit at least one array, got {value}"
                );
            }
            assert!(
                !string_array(entry, "forbiddenArrayOperations", &case).is_empty(),
                "case {case}: expected the forbidden array operations, got {entry}"
            );

            // The hardware constraints a client checks before it builds a program —
            // booleans, and coherent with the device type and the pragmas the same
            // response publishes.
            for property in [
                "supportPhysicalQubits",
                "requiresAllQubitsMeasurement",
                "requiresContiguousQubitIndices",
                "disabledQubitRewiringSupported",
                "supportsUnassignedMeasurements",
                "supportsPartialVerbatimBox",
            ] {
                assert!(
                    entry.get(property).and_then(Value::as_bool).is_some(),
                    "case {case}: expected {property} as a boolean, got {entry}"
                );
            }
            assert_eq!(
                entry.get("supportPhysicalQubits").and_then(Value::as_bool),
                Some(is_qpu),
                "case {case}: a physical qubit can only be addressed on hardware \
                 (deviceType QPU: {is_qpu}), got {entry}"
            );
            assert_eq!(
                entry
                    .get("supportsPartialVerbatimBox")
                    .and_then(Value::as_bool),
                Some(supported.iter().any(|pragma| pragma == "verbatim")),
                "case {case}: a partial verbatim box is supported exactly where the \
                 verbatim pragma is (supported pragmas {supported:?}), got {entry}"
            );
        }
    }

    declared_schemas.sort();
    declared_schemas.dedup();
    assert!(
        declared_schemas.len() >= 2,
        "a device declares the schema of its OWN family: expected at least two distinct \
         document schemas across the inventory, got {declared_schemas:?}"
    );

    assert!(
        gate_model_entries >= 2 && other_family_entries >= 1,
        "the inventory must span both action families: got {gate_model_entries} gate-model \
         and {other_family_entries} non-gate-model action entries"
    );

    // The device properties report each device's OWN native gates, not one fixed set.
    native_gate_sets.sort();
    native_gate_sets.dedup();
    assert!(
        native_gate_sets.len() >= 3,
        "the catalog's devices run different native gates: expected at least three \
         distinct native gate sets across the inventory, got {native_gate_sets:?}"
    );
}

/// Reading a device twice reports the same document: a published catalog entry does
/// not change because somebody looked at it.
#[tokio::test]
async fn reading_a_device_twice_publishes_the_same_document() {
    let braket = Braket::new();
    for summary in braket.inventory().await {
        let arn = member(&summary, "deviceArn");
        let first = member(&braket.get_device(arn).await.body, "deviceCapabilities").to_string();
        let second = member(&braket.get_device(arn).await.body, "deviceCapabilities").to_string();
        assert_eq!(
            first, second,
            "case device {arn}: the capability document must be stable across reads"
        );
    }
}
