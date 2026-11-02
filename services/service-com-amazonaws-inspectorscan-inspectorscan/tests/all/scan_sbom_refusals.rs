// SPDX-License-Identifier: CC0-1.0

//! The WIRE CONTRACT of a `ScanSbom` input refusal, end-to-end over HTTP.
//!
//! One rule, asserted over every class of input defect this operation refuses:
//! a bad input SBOM is answered HTTP 400 `ValidationException` whose body
//! carries `message` ALONE — the fixed text `Input SBOM failed validation: `
//! followed by a clause naming the CLASS of defect (never an index, a member
//! path, or a value read back out of the submitted document).
//!
//! The table below is that rule's cases, grouped by class. Each class is
//! exercised in several forms — an absent member and a wrongly-valued one, a
//! top-level member and the same member nested inside an assembly, the bound of
//! a limit and one past it — so a row is one instance of the rule and not the
//! rule itself. Two documents defective the same way must be refused with the
//! same sentence, which is what the repeated `expect` values pin.
//!
//! Shares the HTTP helpers of `scan_sbom.rs` through the `super` path the test
//! crate root gives it.

use super::scan_sbom::{bom, component, emulator, post_scan};
use serde_json::{json, Value};

/// The fixed text every input-SBOM refusal opens with.
const PREFIX: &str = "Input SBOM failed validation: ";

/// The component ceiling the handler enforces (`MAX_COMPONENTS` in
/// `src/ops/scan_sbom.rs`, from the `sbom` member's own modeled documentation).
/// The limit clause states this same number, which the bound cases below check
/// by accepting exactly this many components and refusing one more.
const COMPONENT_LIMIT: usize = 2000;

/// One row: a request's `sbom` value and the whole refusal message it must be
/// answered with.
#[derive(Debug)]
struct Case {
    what: &'static str,
    sbom: Value,
    expect: String,
}

/// `PREFIX` plus the clause for a document that does not parse.
fn cannot_parse() -> String {
    format!("{PREFIX}Invalid json")
}

/// `PREFIX` plus the clause for a CycloneDX member that is absent where the
/// specification requires it, or carries a value it does not allow there.
///
/// `member` is the member's name as the CycloneDX 1.5 specification spells it —
/// the same member names `src/ops/scan_sbom.rs` validates: `bomFormat`,
/// `specVersion`, `components`, `type`, `name`, `purl`, `bom-ref`.
fn invalid_value(member: &str) -> String {
    format!("{PREFIX}Invalid value for \"{member}\"")
}

/// `PREFIX` plus the clause for a document carrying more components than one
/// scan accepts; the limit is the clause's only variable part.
fn too_many_components(limit: usize) -> String {
    format!("{PREFIX}Max number of components exceeded ({limit})")
}

/// A JSON document nested `levels` arrays deep: more than this operation can
/// take in, so it does not parse as a document at all.
fn deeply_nested(levels: usize) -> Value {
    let mut value = json!({ "bomFormat": "CycloneDX", "specVersion": "1.5" });
    for _ in 0..levels {
        value = Value::Array(vec![value]);
    }
    value
}

/// An assembly whose `components[]` nests `levels` deep: deeper than a scan
/// resolves, so the `components` member is one it cannot read.
fn deep_assembly(levels: usize) -> Value {
    let mut entry = component("ref-leaf", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20");
    for level in 0..levels {
        entry = json!({
            "type": "application",
            "bom-ref": format!("ref-assembly-{level}"),
            "name": "app",
            "version": "1.0.0",
            "components": [entry],
        });
    }
    entry
}

/// `count` distinct components, each a clean coordinate.
fn many_components(count: usize) -> Vec<Value> {
    (0..count)
        .map(|i| {
            component(
                &format!("ref-{i}"),
                "left-pad",
                "1.0.0",
                "pkg:npm/left-pad@1.0.0",
            )
        })
        .collect()
}

fn cases() -> Vec<Case> {
    let mut cases = vec![
        // A `sbom` that is not a CycloneDX document at all: the class is "it
        // does not parse", whatever JSON type was sent instead.
        Case {
            what: "a string document",
            sbom: json!("this-is-not-a-cyclonedx-sbom-just-a-string"),
            expect: cannot_parse(),
        },
        Case {
            what: "an array document",
            sbom: json!([1, 2, 3]),
            expect: cannot_parse(),
        },
        Case {
            what: "a number document",
            sbom: json!(7),
            expect: cannot_parse(),
        },
        Case {
            what: "a boolean document",
            sbom: json!(true),
            expect: cannot_parse(),
        },
        Case {
            what: "a document nested deeper than it can be parsed",
            sbom: deeply_nested(96),
            expect: cannot_parse(),
        },
        // `bomFormat`: absent and wrongly-valued are one class.
        Case {
            what: "an empty document, carrying no bomFormat",
            sbom: json!({}),
            expect: invalid_value("bomFormat"),
        },
        Case {
            what: "a document carrying specVersion but no bomFormat",
            sbom: json!({ "specVersion": "1.5", "components": [] }),
            expect: invalid_value("bomFormat"),
        },
        Case {
            what: "a non-CycloneDX bomFormat",
            sbom: json!({ "bomFormat": "SPDX", "specVersion": "1.5", "components": [] }),
            expect: invalid_value("bomFormat"),
        },
        Case {
            // CycloneDX fixes `bomFormat` to the literal "CycloneDX".
            what: "a bomFormat in the wrong case",
            sbom: json!({ "bomFormat": "cyclonedx", "specVersion": "1.5", "components": [] }),
            expect: invalid_value("bomFormat"),
        },
        Case {
            what: "a bomFormat that is not a string",
            sbom: json!({ "bomFormat": 1, "specVersion": "1.5", "components": [] }),
            expect: invalid_value("bomFormat"),
        },
        // `specVersion`: the revision either side of the one a scan reads, and
        // the member absent or of the wrong type.
        Case {
            what: "a missing specVersion",
            sbom: json!({ "bomFormat": "CycloneDX", "components": [] }),
            expect: invalid_value("specVersion"),
        },
        Case {
            what: "an earlier CycloneDX spec version",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.4", "components": [] }),
            expect: invalid_value("specVersion"),
        },
        Case {
            what: "a later CycloneDX spec version",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.6", "components": [] }),
            expect: invalid_value("specVersion"),
        },
        Case {
            what: "a specVersion that is not a string",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": 1.5, "components": [] }),
            expect: invalid_value("specVersion"),
        },
        // `components`: the member itself, its entries, and the same member one
        // level down inside an assembly.
        Case {
            what: "components that are not an array",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.5", "components": {} }),
            expect: invalid_value("components"),
        },
        Case {
            what: "components sent as a string",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.5", "components": "none" }),
            expect: invalid_value("components"),
        },
        Case {
            what: "a component entry that is not an object",
            sbom: bom(vec![json!("lodash@4.17.20")]),
            expect: invalid_value("components"),
        },
        Case {
            what: "a nested components member that is not an array",
            sbom: bom(vec![json!({
                "type": "application",
                "bom-ref": "ref-app",
                "name": "app",
                "version": "1.0.0",
                "components": { "name": "lodash" },
            })]),
            expect: invalid_value("components"),
        },
        Case {
            what: "a nested component entry that is not an object",
            sbom: bom(vec![json!({
                "type": "application",
                "bom-ref": "ref-app",
                "name": "app",
                "version": "1.0.0",
                "components": ["lodash@4.17.20"],
            })]),
            expect: invalid_value("components"),
        },
        Case {
            what: "a component tree nested deeper than a scan resolves",
            sbom: bom(vec![deep_assembly(20)]),
            expect: invalid_value("components"),
        },
        // A component's own required members.
        Case {
            what: "a component carrying no type",
            sbom: bom(vec![json!({ "name": "lodash", "version": "4.17.20" })]),
            expect: invalid_value("type"),
        },
        Case {
            what: "a component whose type is not a CycloneDX component type",
            sbom: bom(vec![json!({
                "type": "package",
                "name": "lodash",
                "version": "4.17.20",
            })]),
            expect: invalid_value("type"),
        },
        Case {
            what: "a component carrying no name",
            sbom: bom(vec![json!({
                "type": "library",
                "version": "4.17.20",
                "purl": "pkg:npm/lodash@4.17.20",
            })]),
            expect: invalid_value("name"),
        },
        Case {
            what: "a component whose name is empty",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "",
                "version": "4.17.20",
            })]),
            expect: invalid_value("name"),
        },
        // `purl`: every form that is not a package URL is one class.
        Case {
            what: "a purl carrying no pkg: scheme",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "lodash@4.17.20",
            })]),
            expect: invalid_value("purl"),
        },
        Case {
            what: "a purl whose scheme is not the lowercase \"pkg\"",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "PKG:npm/lodash@4.17.20",
            })]),
            expect: invalid_value("purl"),
        },
        Case {
            what: "a purl naming a type but no package",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "pkg:npm",
            })]),
            expect: invalid_value("purl"),
        },
        Case {
            what: "a purl whose type is outside the purl grammar",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "pkg:np m/lodash@4.17.20",
            })]),
            expect: invalid_value("purl"),
        },
        // `bom-ref`: CycloneDX requires it to be unique within a document, and
        // a scan bounds its length.
        Case {
            what: "two components sharing one bom-ref",
            sbom: bom(vec![
                component("same", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20"),
                component("same", "minimist", "1.2.5", "pkg:npm/minimist@1.2.5"),
            ]),
            expect: invalid_value("bom-ref"),
        },
        Case {
            what: "a bom-ref longer than a scan resolves",
            sbom: bom(vec![json!({
                "type": "library",
                "bom-ref": "r".repeat(513),
                "name": "lodash",
                "version": "4.17.20",
            })]),
            expect: invalid_value("bom-ref"),
        },
    ];

    // One past the component ceiling, flat and through an assembly: every entry
    // of the tree counts, so the same total is refused either way.
    cases.push(Case {
        what: "one component more than a scan accepts",
        sbom: bom(many_components(COMPONENT_LIMIT + 1)),
        expect: too_many_components(COMPONENT_LIMIT),
    });
    let assembly = json!({
        "type": "application",
        "bom-ref": "ref-assembly",
        "name": "assembly",
        "version": "1.0.0",
        "components": many_components(COMPONENT_LIMIT),
    });
    cases.push(Case {
        what: "an assembly whose tree is one component over the ceiling",
        sbom: bom(vec![assembly]),
        expect: too_many_components(COMPONENT_LIMIT),
    });
    cases
}

#[tokio::test]
async fn every_class_of_input_defect_is_refused_with_the_services_own_sentence() {
    let (_engine, router) = emulator();
    for case in cases() {
        let (status, response) = post_scan(&router, &json!({ "sbom": case.sbom })).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status}: {response}",
            case.what
        );
        assert_eq!(
            response["__type"].as_str(),
            Some("ValidationException"),
            "case {:?}: expected the ValidationException error code, got {response}",
            case.what
        );
        assert_eq!(
            response["message"].as_str(),
            Some(case.expect.as_str()),
            "case {:?}: expected message {:?}, got {response}",
            case.what,
            case.expect
        );
        // The body carries `message` alone: the service populates neither
        // `reason` nor `fields` on this refusal, so a client must not find
        // them. `__type` is the protocol's error discriminator, not a member of
        // the shape.
        let members: Vec<&String> = response
            .as_object()
            .map(|body| body.keys().filter(|k| *k != "__type").collect())
            .unwrap_or_default();
        assert_eq!(
            members,
            vec!["message"],
            "case {:?}: the refusal body must carry message alone, got {response}",
            case.what
        );
    }
}

/// The limit the clause states is the limit that is enforced: exactly
/// [`COMPONENT_LIMIT`] components are scanned, and one more is refused naming
/// that same number.
#[tokio::test]
async fn the_component_limit_clause_states_the_limit_that_is_enforced() {
    let (_engine, router) = emulator();
    for (count, expected_status) in [
        (1_usize, 200_u16),
        (COMPONENT_LIMIT - 1, 200),
        (COMPONENT_LIMIT, 200),
        (COMPONENT_LIMIT + 1, 400),
        (COMPONENT_LIMIT + 500, 400),
    ] {
        let (status, response) =
            post_scan(&router, &json!({ "sbom": bom(many_components(count)) })).await;
        assert_eq!(
            status, expected_status,
            "case {count} components: expected HTTP {expected_status}, got {status}: {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                response["message"].as_str(),
                Some(too_many_components(COMPONENT_LIMIT).as_str()),
                "case {count} components: expected the limit clause to name {COMPONENT_LIMIT}, \
                 got {response}"
            );
        }
    }
}

/// A valid document is still scanned: the refusal path above is reached for
/// defective input only, not for every request.
#[tokio::test]
async fn a_valid_document_is_not_refused() {
    let (_engine, router) = emulator();
    let (status, response) = post_scan(
        &router,
        &json!({
            "sbom": bom(vec![component(
                "ref-clean",
                "left-pad",
                "1.0.0",
                "pkg:npm/left-pad@1.0.0",
            )]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a minimal valid CycloneDX 1.5 document is scanned, got {status}: {response}"
    );
    assert!(
        response["message"].is_null(),
        "a scanned document carries no refusal message, got {response}"
    );
}
