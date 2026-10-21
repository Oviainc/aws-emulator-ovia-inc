Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the accessanalyzer emulator

_Filed: 2026-10-21T21:18:23Z_
_Operation: service-com-amazonaws-accessanalyzer-accessanalyzer | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/` is an untouched scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns HTTP 501 `NotImplemented`, holding no state. Implement a FIRST stateful slice around the primary resource, the **analyzer**, plus its archive rules (restJson1, sigv4 name `access-analyzer`).

Happy path to implement, in order:

1. **CreateAnalyzer** — `PUT /analyzer`. Input: `analyzerName` (required, len 1..255, pattern `^[A-Za-z_][A-Za-z0-9_.-]*$`), `type` (required: ACCOUNT | ACCOUNT_INTERNAL_ACCESS | ACCOUNT_UNUSED_ACCESS | ORGANIZATION | ORGANIZATION_INTERNAL_ACCESS | ORGANIZATION_UNUSED_ACCESS), optional `tags`, `configuration`, `archiveRules`, `clientToken`. Output: `arn` — must be a generated, STORED ARN of the form `arn:aws:access-analyzer:<region>:<account>:analyzer/<analyzerName>`. Re-creating an existing name returns `ConflictException` (409).
2. **GetAnalyzer** — `GET /analyzer/{analyzerName}`. Output: required `analyzer` (AnalyzerSummary) rebuilt from stored state: `arn`, `name`, `type`, `createdAt`, `status` (`ACTIVE`), plus `tags` and `configuration` exactly as supplied at create. Unknown name → `ResourceNotFoundException` (404).
3. **ListAnalyzers** — `GET /analyzer`. Input: `type`, `maxResults`, `nextToken` (all query params). Output: required `analyzers` (list<AnalyzerSummary>) listing every analyzer actually stored, filtered by `type` when given, truncated to `maxResults` with an opaque `nextToken` that resumes.
4. **CreateArchiveRule** — `PUT /analyzer/{analyzerName}/archive-rule`. Input: `ruleName` (required, pattern `^[A-Za-z][A-Za-z0-9_.-]*$`), `filter` (required map<string,Criterion>), `clientToken`. Empty output body. Store the rule under the analyzer; unknown analyzer → 404; duplicate rule name → `ConflictException` (409).
5. **ListArchiveRules** / **GetArchiveRule** — `GET /analyzer/{analyzerName}/archive-rule` and `.../archive-rule/{ruleName}`. Return ArchiveRuleSummary values built from state: `ruleName`, `filter`, `createdAt`, `updatedAt` (all required). Unknown analyzer or rule → 404.
6. **DeleteAnalyzer** — `DELETE /analyzer/{analyzerName}`. Removes the analyzer and its archive rules; afterwards GetAnalyzer returns 404 and ListAnalyzers no longer includes it. Unknown name → `ResourceNotFoundException` (404).

NEVER STUBS: each of these must compute its response from the shared engine state reached via `crate::state::service_state(&*self.engine)`. No canned ARNs, no echoed input, no random id that is not stored — a create followed by a get/list must return exactly what was created, and a delete must be observable by the next call.

## State Model

In `src/state.rs`, on `ComAmazonawsAccessanalyzerAccessanalyzerState`:

- `analyzers: HashMap<String /* analyzerName */, Analyzer>`
- `struct Analyzer { name: String, arn: String, r#type: String, status: String /* ACTIVE */, status_reason: Option<StatusReason>, created_at: SystemTime, last_resource_analyzed: Option<String>, last_resource_analyzed_at: Option<SystemTime>, tags: HashMap<String,String>, configuration: Option<AnalyzerConfiguration>, archive_rules: HashMap<String /* ruleName */, ArchiveRule> }`
- `struct ArchiveRule { rule_name: String, filter: HashMap<String, Criterion>, created_at: SystemTime, updated_at: SystemTime }`
- optional for idempotency: `client_tokens: HashMap<String /* clientToken */, String /* resulting arn */>` so a repeated `clientToken` on CreateAnalyzer returns the first result instead of a conflict.

All other operations (findings, access previews, policy generation/validation, checks, tagging) stay 501 for now and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate (39 operations, protocol restJson1, sigv4 `access-analyzer`) documents the analyzer lifecycle used above: CreateAnalyzer `PUT /analyzer` → output `arn`, GetAnalyzer `GET /analyzer/{analyzerName}` → required `analyzer` (AnalyzerSummary), ListAnalyzers `GET /analyzer` → required `analyzers` + `nextToken` with `maxResults`/`type` query params, CreateArchiveRule/GetArchiveRule/ListArchiveRules under `/analyzer/{analyzerName}/archive-rule`, DeleteAnalyzer `DELETE /analyzer/{analyzerName}`, each declaring ResourceNotFoundException (404) and (for creates) ConflictException (409). Field lists for AnalyzerSummary (arn, name, type, createdAt, status required; tags, configuration, statusReason, lastResourceAnalyzed* optional) and ArchiveRuleSummary (ruleName, filter, createdAt, updatedAt all required) come from the Smithy model via smithy_query. Every file in src/ops/ is the generated stub returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (501), per the crate README's description of the scaffold marker.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `accessanalyzer` client at it.
2. `aws accessanalyzer create-analyzer --analyzer-name demo --type ACCOUNT --tags env=test` → expect 200 with an `arn` ending in `analyzer/demo` (currently HTTP 501 NotImplemented).
3. `aws accessanalyzer get-analyzer --analyzer-name demo` → expect the stored AnalyzerSummary: name `demo`, type `ACCOUNT`, status ACTIVE, the same arn, tags `env=test`.
4. `aws accessanalyzer list-analyzers` → expect `analyzers` to contain exactly the `demo` summary; repeat with `--type ORGANIZATION` and expect an empty list.
5. `aws accessanalyzer create-archive-rule --analyzer-name demo --rule-name rule1 --filter '{"resourceType":{"eq":["AWS::S3::Bucket"]}}'` → expect 200, then `get-archive-rule --analyzer-name demo --rule-name rule1` returns that filter with createdAt/updatedAt, and `list-archive-rules --analyzer-name demo` lists it.
6. `aws accessanalyzer create-analyzer --analyzer-name demo --type ACCOUNT` again → expect ConflictException (409).
7. `aws accessanalyzer delete-analyzer --analyzer-name demo` → 200, then `get-analyzer --analyzer-name demo` returns ResourceNotFoundException (404) and `list-analyzers` returns an empty list.
