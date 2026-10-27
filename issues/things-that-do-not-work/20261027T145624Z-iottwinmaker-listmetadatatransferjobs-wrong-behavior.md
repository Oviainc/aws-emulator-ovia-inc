Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# ListMetadataTransferJobs refuses maxResults=0, although its member documents no minimum and the sibling listing with the identical documentation serves it

_Filed: 2026-10-27T14:56:24Z_
_Operation: ListMetadataTransferJobs | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate resolves its page size through one shared rule, `crate::state::page_size`, parameterised by what THAT operation's own `maxResults` member documents. The crate states the rule explicitly: the minimum is "taken from that member's own documentation", and a member that documents no minimum serves an explicit zero as a page of no rows instead of refusing it.

`ListMetadataTransferJobs` breaks that rule. Its `maxResults` documentation is "The maximum number of results to return at one time." — it publishes neither a default nor a minimum — and its modelled `@range` is `{min: 0, max: 200}`, so the framework validator admits a zero. The handler nevertheless builds the rule with `PageSizeRule::minimum_one(...)`, so `maxResults: 0` is answered with HTTP 400 `ValidationException` ("maxResults must be at least 1; omit it for the default of 25") instead of a 200 carrying an empty `metadataTransferJobSummaries`.

`ListScenes` is the control: its `maxResults` targets the same `MaxResults` shape with the same `@range {min: 0, max: 200}`, and its documentation is equally silent about a minimum ("Specifies the maximum number of results to display."), and that handler uses `PageSizeRule::minimum_zero(...)` and serves the zero. So the two identically-documented members get opposite answers to the same request.

The handler comment also misattributes its own choice: it claims the zero is refused "the way every listing of this service whose `maxResults` documents a minimum of 1 refuses it" — but this member documents no minimum at all. The per-operation table in `tests/all/page_size_rule.rs` carries the same mistake: its own doc states the classification rule and then classifies this operation against it.

## Evidence

MODEL (the member's own documentation, `smithy_query(query="doc",shape="ListMetadataTransferJobsRequest",member="maxResults")`):
  "<p>The maximum number of results to return at one time.</p>"
— no default, no "Valid Range: Minimum value of 1", unlike e.g. `ListEntitiesRequest$maxResults`, whose doc reads "The maximum number of results to return at one time. The default is 25.\nValid Range: Minimum value of 1. Maximum value of 250."

MODEL (the constraint, `smithy_query(query="operation",operation="ListMetadataTransferJobs")`):
  "maxResults": {"target": "MaxResults", "range": {"min": 0, "max": 200}, "type": "integer"}
Identical to ListScenes': "maxResults": {"target": "MaxResults", "range": {"min": 0, "max": 200}, "type": "integer"}

CRATE RULE (src/state.rs:174-180):
  "The rule is the same for every paginated operation of this service; only these two numbers differ between them, so they are parameters rather than part of the rule: ... and a member documenting a minimum of 1 refuses the zero a member documenting a minimum of 0 serves as a page of no rows."
and src/ops/list_sync_jobs.rs:304-306: "so both the default and what a zero means are per-operation, each taken from that member's own documentation."

THE VIOLATING SIDE (src/ops/list_metadata_transfer_jobs.rs:291-303):
  "// ... an explicit zero is refused the way every listing of this service whose
   // `maxResults` documents a minimum of 1 refuses it, ...
   let page_size = crate::state::page_size(
       input.max_results,
       crate::state::PageSizeRule::minimum_one(crate::state::DEFAULT_PAGE_SIZE),
   )"

THE SIBLING WITH THE SAME DOCUMENTATION (src/ops/list_scenes.rs:234-249):
  "// ... THIS member's documentation publishes no default and no minimum, so a zero is served
   // as what it says — a page of no rows — rather than refused ...
   let page_size = crate::state::page_size(
       input.max_results,
       crate::state::PageSizeRule::minimum_zero(crate::state::DEFAULT_PAGE_SIZE),
   )"

THE REFUSAL TEXT (src/state.rs:220-232): `format!("maxResults must be at least {}; omit it for the default of {}", self.minimum, self.default)`

TEST COVERAGE — the test pins the current behaviour but contradicts its OWN stated reason (tests/all/page_size_rule.rs:109-119):
  "/// The smallest `maxResults` this operation's member documents: zero where
   /// the member publishes a minimum of 0 or none at all (so a zero is a page of
   /// no rows), one where it publishes \"Minimum value of 1\" — read off the member
   /// docs with `smithy_query(query=\"doc\",shape=\"<operation>Request\",member=\"maxResults\")`.
   fn documented_minimum(self) -> u32 {
       match self {
           Self::ListScenes | Self::ListSyncJobs | Self::ListComponents => 0,
           _ => 1,
       }
   }"
`ListMetadataTransferJobs` falls into the `_ => 1` arm although its member "publishes ... none at all", which the doc above says must map to zero; the assertion that then fires is tests/all/page_size_rule.rs:442-454.

## Steps to Reproduce

1. CreateWorkspace `w1`.
2. CreateMetadataTransferJob with sources `[{"type":"s3","s3Configuration":{"location":"arn:aws:s3:::metadata/entities.json"}}]` and destination `{"type":"iottwinmaker","iotTwinMakerConfiguration":{"workspace":"<w1 arn>"}}`.
3. POST /metadata-transfer-jobs-list with body `{"sourceType":"s3","destinationType":"iottwinmaker","maxResults":0}`.
4. Observe HTTP 400 ValidationException "maxResults must be at least 1; omit it for the default of 25", although the member documents no minimum and the modelled @range admits 0.
5. Compare with the identically-documented sibling: POST /workspaces/w1/scenes-list with `{"maxResults":0}` returns HTTP 200 with an empty `sceneSummaries` list.
