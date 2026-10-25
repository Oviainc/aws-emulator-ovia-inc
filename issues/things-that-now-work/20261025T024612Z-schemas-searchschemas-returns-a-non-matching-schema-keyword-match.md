Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-schema-search-and-list
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-schema-search-and-list
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|SearchSchemas|2xx->2xx|$.Schemas[*]
Issue-Commit: 8d2004325ac14849f8dfb95f46b05c10ee176845

# SearchSchemas returns a non-matching schema: keyword match is too broad

> _Note: this issue was identified against the code base at commit `8d2004325ac14849f8dfb95f46b05c10ee176845`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchSchemas

## Description
SearchSchemas on a caller-owned registry selects a schema that the recorded AWS response does not select, so a search that AWS answers with an EMPTY result set comes back from the emulator carrying a schema summary.

Cause 1 of 1: SearchSchemas reports a schema whose keywords do not match.

AWS: recorded behavior only. Smithy: SearchSchemas GET /v1/registries/name/{RegistryName}/schemas/search, Keywords (@httpQuery "keywords", required) documented only as "Specifying this limits the results to only schemas that include the provided keywords"; output SearchSchemasResponse.Schemas is a list of SearchSchemaSummary (RegistryName, SchemaArn, SchemaName, SchemaVersions) and the model states nothing about what a keyword is matched against. The recording is the only statement of the rule: for this search of the fixture's caller-created registry, AWS answers HTTP 200 with NO entry in Schemas (the recorded response has no $.Schemas[0]) even though the registry does hold the caller-created schema, so the keywords of that request match NOTHING that AWS indexes that schema under.

Emulator: HTTP 200 with one entry in Schemas - $.Schemas[0] = {RegistryName: <the searched registry>, SchemaArn: arn:aws:schemas:<region>:<account>:schema/<registry>/<schema>, SchemaName: <the caller-created schema>, SchemaVersions: [{SchemaVersion "1", Type OpenApi3, CreatedDate}]}. The status and the summary's shape are right; the RESULT SET is wrong - it contains a schema AWS excluded.

Hypothesis: services/service-com-amazonaws-schemas-schemas/src/ops/search_schemas.rs indexes every schema under its own NAME (indexed_values pushes schema.name.to_lowercase() unconditionally) and matches each keyword as a case-insensitive SUBSTRING of an indexed value (matches_every_keyword -> value.contains(keyword)), with Keywords split on commas/whitespace by keywords_of. Any keyword that happens to occur anywhere inside the generated schema name therefore selects the schema, where AWS selected nothing. The same substring-over-name rule would also make a keyword that is a fragment of a name (not a whole indexed term) a match.

Why: a client that uses SearchSchemas to decide whether a schema matching a term exists, or that renders the search result to a user, gets a false positive - it sees and may act on a schema the real service did not return (and, when paginating, a page composed of schemas AWS would not have listed). Discovery/search flows built on AWS semantics become non-deterministic between the emulator and AWS.

Fix: the keyword-match selection in the SearchSchemas handler - what a schema is indexed under and how a keyword is compared against it (indexed_values / matches_every_keyword / keywords_of in the SearchSchemas handler), not the serializer: the 200 status, Schemas list and SearchSchemaSummary members already serialize correctly. Note the existing open issue 20261023T142626Z-schemas-empty-result-responses-use-the-wrong-status.md concerns the transport of an EMPTY result and does not cover this over-broad selection.

Traces: fixture schemas-schema-search-and-list step task#6 (operation SearchSchemas, verdict mismatch, expected 200 -> actual 200, detail "$.Schemas[0]: extra (expected None, got {...})").

## Evidence
Replay of trace set simparity-wire-ga-prod-v1, fixture schemas-schema-search-and-list, step task#6, operation SearchSchemas (service schemas), verdict mismatch.

Expected (recorded AWS): HTTP 200, body with Schemas containing NO element at index 0 (empty result set for the requested keywords).

Actual (emulator): HTTP 200, body with $.Schemas[0] present:
  {'RegistryName': 'sim2real-probe-schemas-searchreg-ddb1920f', 'SchemaArn': 'arn:aws:schemas:us-west-2:000000000000:schema/sim2real-probe-schemas-searchreg-ddb1920f/sim2real-probe-schemas-apple-ddb1920f', 'SchemaName': 'sim2real-probe-schemas-apple-ddb1920f', 'SchemaVersions': [{'CreatedDate': '2026-10-25T02:45:04.154612Z', 'SchemaVersion': '1', 'Type': 'OpenApi3'}]}
(the registry/schema names, account, region and timestamp above are this recording's values, not requirements; the requirement is that the result set is empty for keywords AWS matches nothing against.)

divergence_key: service=schemas|SearchSchemas|status=200->200|keys=$.Schemas[0]
Model checked: SearchSchemas input Keywords/Limit/NextToken/RegistryName; output Schemas -> SearchSchemaSummary; errors BadRequestException(400), ForbiddenException(403), UnauthorizedException(401), InternalServerErrorException(500), ServiceUnavailableException(503). The model documents no keyword-matching rule, so the recorded empty result is the authority.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the wire corpus (trace set simparity-wire-ga-prod-v1).
2. Replay fixture schemas-schema-search-and-list up to step task#6: it creates a caller-owned registry and schemas in it (OpenApi3, one version each), then issues GET /v1/registries/name/<registry>/schemas/search?keywords=<the recorded keywords> (SearchSchemas).
3. Compare the response body against the recording: AWS returns HTTP 200 with an empty Schemas list; the emulator returns HTTP 200 with one SearchSchemaSummary for the caller-created schema ($.Schemas[0] extra).
4. Equivalently, by hand: create a registry and a schema in it, then call SearchSchemas on that registry with a keyword that is only a fragment of the schema name and appears nowhere the service indexes the schema under - the emulator selects the schema, AWS selects nothing.
