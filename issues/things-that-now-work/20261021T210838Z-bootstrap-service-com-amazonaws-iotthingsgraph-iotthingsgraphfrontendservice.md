Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the IoTThingsGraph emulator

_Filed: 2026-10-21T21:08:38Z_
_Operation: service-com-amazonaws-iotthingsgraph-iotthingsgraphfrontendservice | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 35 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL stateful slice around the service's primary resource, the flow template (workflow), so a realistic client script runs end-to-end.

Happy path to implement, in order (awsJson1_1, `POST /` with `X-Amz-Target: IotThingsGraphFrontEndService.<Op>`):

1. **CreateFlowTemplate** — input `definition` (DefinitionDocument{`language` (required, GRAPHQL), `text` (required, GraphQL text, 0..1048576)}, required) and optional `compatibleNamespaceVersion` (long). Must mint a workflow id of the modeled `Urn` form (`^urn:tdm:...:workflow:<Name>$`, len 0..160, derived from the workflow name in the definition text), store the definition, and return `summary` (FlowTemplateSummary{`id`, `arn`, `revisionNumber` = 1, `createdAt` = now}). A second create for the same id must fail `ResourceAlreadyExistsException` (HTTP 409).
2. **GetFlowTemplate** — input `id` (required), optional `revisionNumber`. Must return `description` (FlowTemplateDescription{`summary`, `definition`, `validatedNamespaceVersion`}) reconstructed from the stored record — the `definition.text`/`language` returned must be byte-identical to what CreateFlowTemplate/UpdateFlowTemplate persisted, and `revisionNumber` must match the stored revision. When `revisionNumber` is given, return that revision; unknown id or revision → `ResourceNotFoundException` (HTTP 404).
3. **UpdateFlowTemplate** — input `id` (required), `definition` (required), optional `compatibleNamespaceVersion`. Must append a NEW revision (revisionNumber incremented from the latest stored revision, new `createdAt`) and return the new `summary`; a later GetFlowTemplate with no `revisionNumber` must return the new definition, and with the old `revisionNumber` must still return the old definition. Unknown id → `ResourceNotFoundException` (404).
4. **SearchFlowTemplates** — input optional `filters` (list<FlowTemplateFilter>{`name` required enum, `value` required list<string>}), `nextToken`, `maxResults` (1..250). Must return `summaries` built from the stored workflows (latest revision per workflow by default), honour `maxResults` as a real page size, return an opaque `nextToken` when more remain, and resume from that token. With no filters it must list every created workflow; a filter on DEVICE_MODEL_ID must actually narrow the result set rather than being ignored.
5. **GetFlowTemplateRevisions** — input `id` (required), `nextToken`, `maxResults`. Must return `summaries` for every stored revision of that workflow (so after one create + one update it returns two summaries with revisionNumber 1 and 2), paginated by the same token scheme. Unknown id → `ResourceNotFoundException` (404).
6. **DeleteFlowTemplate** — input `id` (required). Must remove the workflow and all its revisions from state; a subsequent GetFlowTemplate must fail `ResourceNotFoundException` (404) and SearchFlowTemplates must no longer list it. Deleting a workflow referenced by a stored system template/instance → `ResourceInUseException` (HTTP 412).

Every one of these must compute its response from persisted shared state reached via `crate::state::service_state(&*self.engine)` — no canned responses, no echoing the input back, no minting an id without storing the record. Input constraints the model declares (the `Urn` pattern and 0..160 length on `id`, required `language`/`text` on DefinitionDocument, `maxResults` range 1..250) must be rejected with `InvalidRequestException` (HTTP 400) rather than accepted.

## State Model

In `src/state.rs`, on the service state struct:

- `flow_templates: BTreeMap<String /* workflow urn id */, FlowTemplateRecord>`
- `FlowTemplateRecord { id: String, arn: String, revisions: Vec<FlowTemplateRevision>, created_at: SystemTime }`
- `FlowTemplateRevision { revision_number: i64, definition_language: String, definition_text: String, compatible_namespace_version: Option<i64>, validated_namespace_version: Option<i64>, created_at: SystemTime }`
- `next_revision: i64` per record (or derived as `revisions.last().revision_number + 1`)
- `page_tokens: BTreeMap<String /* nextToken */, PageCursor>` where `PageCursor { kind: &'static str, key: Option<String>, offset: usize }`, so SearchFlowTemplates / GetFlowTemplateRevisions tokens are opaque and resumable rather than recomputed guesses.

All other operations (system templates, system instances, namespaces/uploads, entities/things, flow executions, tagging) stay 501 for now and are tracked as individual gap issues.

## Evidence

OPERATIONS.md for this crate documents all 35 operations and their contracts: CreateFlowTemplate (input `definition` DefinitionDocument required, `compatibleNamespaceVersion`; output `summary` FlowTemplateSummary; errors include ResourceAlreadyExistsException 409), GetFlowTemplate (`id` required, `revisionNumber`; output `description` FlowTemplateDescription; ResourceNotFoundException 404), UpdateFlowTemplate (`id` + `definition` required; output `summary`), SearchFlowTemplates and GetFlowTemplateRevisions (both @paginated inputToken=nextToken/outputToken=nextToken/items=summaries, `maxResults` range 1..250), DeleteFlowTemplate (`id` required; ResourceInUseException 412). smithy_query on FlowTemplateSummary/FlowTemplateDescription/DefinitionDocument/FlowTemplateFilter confirms the member names and the required `language`/`text` and the Urn pattern. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` returns all 35 handler files (including create_flow_template.rs, get_flow_template.rs, update_flow_template.rs, search_flow_templates.rs, get_flow_template_revisions.rs, delete_flow_template.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, and the crate has no state fields or tests beyond the generated state_roundtrip.rs example.

## Steps to Reproduce

1. Start the emulator and point an awsJson1_1 client at it with target prefix `IotThingsGraphFrontEndService`.
2. CreateFlowTemplate with definition {language: "GRAPHQL", text: "query MyFlow { ... }"} → expect 200 with summary.id matching ^urn:tdm:.*:workflow:.*$, summary.revisionNumber == 1, summary.arn and summary.createdAt set (actual today: HTTP 501 NotImplemented).
3. GetFlowTemplate with that id → expect 200 and description.definition.text byte-identical to the text uploaded, description.summary.revisionNumber == 1.
4. UpdateFlowTemplate with the same id and a modified definition text → expect 200 with summary.revisionNumber == 2.
5. GetFlowTemplate with the id and no revisionNumber → expect the UPDATED text; GetFlowTemplate with revisionNumber 1 → expect the ORIGINAL text.
6. GetFlowTemplateRevisions with the id → expect 2 summaries (revisionNumber 1 and 2); repeat with maxResults=1 → expect 1 summary plus a nextToken, and passing that nextToken returns the second summary.
7. SearchFlowTemplates with no filters → expect the created workflow listed; create a second workflow and call again with maxResults=1 → expect one summary plus a nextToken that pages to the other.
8. DeleteFlowTemplate with the id → expect 200, then GetFlowTemplate on it → expect ResourceNotFoundException (HTTP 404) and SearchFlowTemplates no longer lists it.
9. CreateFlowTemplate again with the same workflow definition name while it still exists → expect ResourceAlreadyExistsException (HTTP 409); GetFlowTemplate with id "not-a-urn" → expect InvalidRequestException (HTTP 400).
