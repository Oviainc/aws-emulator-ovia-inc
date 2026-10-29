Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b5beeec43dab1903d141b50657328b7d4cca24f8

# The draft-4 export carries no root schema, contradicting state.rs's documented rule that ExportSchema promotes the AWSEvent component to the export's root

_Filed: 2026-10-29T08:52:27Z_
_Operation: ExportSchema | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b5beeec43dab1903d141b50657328b7d4cca24f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` states, as the documentation of the constant that names the event envelope component, that `ExportSchema` ROOTS its JSON Schema draft-4 export at that component. `ExportSchema` does nothing of the kind: `to_json_schema_draft4` never reads the constant (a grep over the whole crate shows `EVENT_ENVELOPE_COMPONENT` is referenced only by `state.rs` itself, by `published_document_bytes.rs` and by a unit test in `get_discovered_schema.rs` — never by `export_schema.rs`), and the root object it builds carries exactly three members: `$schema`, the source document's `info.title`, and `definitions`.

The two statements cannot both be true, and the one in `state.rs` is the false one. Its `Shared with `ExportSchema`` claim is also literally untrue: `export_schema.rs` does not import or mention the constant at all, while the sibling constant three lines above it (`OPENAPI_COMPONENT_REF`) makes the same `Shared with `ExportSchema`` claim and IS used there (`export_schema.rs:79`, `:358`).

The behavioural consequence of the code's choice is that the exported `Content` is a draft-4 document whose ROOT carries no schema keyword whatsoever — no `type`, no `properties`, no `required`, no `$ref` — so every component sits in `definitions` and nothing refers to any of them from the root. A draft-04 document with no keywords at its root constrains nothing, i.e. the export validates every instance, which is not a translation of the OpenAPI document the operation read. `state.rs:1851-1853` describes the document that WOULD say something (rooted at the event envelope, whose `detail` references the event-specific component, with the remaining components reachable as `definitions`).

This is one crate holding two contradictory rules for one output, and it must be reconciled: either `export_schema.rs` is missing the promotion `state.rs` says it performs, or the `state.rs` doc comment asserts a rule — and a sharing relationship — that does not exist and must be deleted. As filed, the defect is the contradiction plus the rootless export it produces.

## Evidence

Side A — services/service-com-amazonaws-schemas-schemas/src/state.rs:1851-1854 says the promotion happens:
    /// The component an AWS event schema's OpenAPI document roots at: the event envelope,
    /// whose `detail` references the event-specific component. Shared with
    /// `ExportSchema`, which promotes this component to the root of its draft-4 export.
    pub const EVENT_ENVELOPE_COMPONENT: &str = "AWSEvent";

Contrast the sibling constant whose identical `Shared with ExportSchema` claim IS true — state.rs:1846-1849:
    /// Where an AWS event schema's OpenAPI document keeps its reusable components, as a
    /// `$ref` names one. Shared with `ExportSchema`, which rewrites these refs when it
    /// converts a document to JSON Schema draft 4.
    pub const OPENAPI_COMPONENT_REF: &str = "#/components/schemas/";
(used at export_schema.rs:79 `use crate::state::{JsonNode, JSON_SCHEMA_DRAFT4_URI, OPENAPI_COMPONENT_REF};` and export_schema.rs:358).

Side B — services/service-com-amazonaws-schemas-schemas/src/ops/export_schema.rs:271-274 says the opposite, in the doc of the converter:
    /// * the document's `components.schemas` become the draft-4 `definitions`, every one of
    ///   them — an OpenAPI document's components are co-equal reusable schemas, with no one
    ///   of them marked as the document's root, so promoting one would be this emulator
    ///   choosing a root the source document does not name;
and export_schema.rs:280-281:
    /// * the root carries draft-04's `$schema` and the source document's `info.title`, so
    ///   the translation says which event it describes.

The code, export_schema.rs:302-319 — the root gets `$schema`, `title`, `definitions` and nothing else; no component is promoted and nothing refers to one from the root:
        let definitions: Vec<(String, JsonNode)> = in_name_order(
            components
                .iter()
                .map(|(name, schema)| (name.clone(), to_draft4(schema)))
                .collect(),
        );
        let mut root: Vec<(String, JsonNode)> =
            vec![("$schema".to_string(), JsonNode::text(JSON_SCHEMA_DRAFT4_URI))];
        if let Some(title) = document
            .get("info")
            .and_then(|info| info.get("title"))
            .and_then(::serde_json::Value::as_str)
        {
            root.push(("title".to_string(), JsonNode::text(title)));
        }
        root.push(("definitions".to_string(), JsonNode::Object(definitions)));
        Some(JsonNode::Object(root).to_json())

The constant is unreferenced by the handler it claims to be shared with — grep over services/service-com-amazonaws-schemas-schemas for `EVENT_ENVELOPE_COMPONENT` matches only state.rs:1854/:2528/:2561/:4718, tests/all/published_document_bytes.rs:23/:298 and src/ops/get_discovered_schema.rs:868 (a unit test). src/ops/export_schema.rs is not among them.

Test coverage: tests/all/export_reads_the_stored_schema.rs:288-294 pins the current, rootless shape —
        let components = member_names(&source["components"]["schemas"], case, "components.schemas");
        let definitions = member_names(&converted["definitions"], case, "definitions");
        assert_eq!(
            definitions, components,
            "case {case:?}: expected a definition per component {components:?}, got \
             {definitions:?}"
        );
and :282-287 asserts only that the root names draft-04 (`converted["$schema"]`). No assertion anywhere pins that the root carries a schema, and no test references state.rs:1851-1853's promotion rule, so nothing in the suite reconciles the two statements.

## Steps to Reproduce

1. Start the emulator.
2. Export one of the seeded service-owned event schemas as draft 4: GET /v1/registries/name/aws.events/schemas/name/aws.acm@ACMCertificateAvailable/export?type=JSONSchemaDraft4 -> 200.
3. Parse the returned `Content`. Observe its root object holds exactly `$schema`, `title` and `definitions` — no `type`, no `properties`, no `required`, no `$ref` — so the document's root schema constrains nothing, and the `AWSEvent` component sits in `definitions` with nothing referring to it from the root.
4. Compare with crate::state's own statement about this operation, state.rs:1851-1853: "The component an AWS event schema's OpenAPI document roots at: the event envelope ... Shared with `ExportSchema`, which promotes this component to the root of its draft-4 export." The export performs no such promotion.
5. Confirm the constant is not shared with the handler at all: grep -rn EVENT_ENVELOPE_COMPONENT services/service-com-amazonaws-schemas-schemas/src/ops/export_schema.rs returns nothing, while the neighbouring OPENAPI_COMPONENT_REF making the same claim is imported at export_schema.rs:79.
6. The same divergence shows for a caller-created OpenAPI 3 schema: POST /v1/registries/name/mine, POST /v1/registries/name/mine/schemas/name/order.placed with an OpenAPI 3 document carrying an `AWSEvent` component, then GET .../order.placed/export?type=JSONSchemaDraft4 -> a document whose root carries no keywords.
