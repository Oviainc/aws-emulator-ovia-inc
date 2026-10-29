Issue-Source: trace-sweep-divergence
Issue-Trace: inspector2-filter-lifecycle-and-validation
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: inspector2-filter-lifecycle-and-validation
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2|UpdateFilter|4xx->4xx|$.message
Issue-Key: inspector2|UpdateFilter|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: inspector2|UpdateFilter|4xx->4xx|status
Issue-Key: inspector2|ListFilters|2xx->2xx|$.filters[*].tags
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# inspector2 filter surface: UpdateFilter mis-classifies a malformed filterArn as not-found, and ListFilters omits the required empty tags map

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFilter

## Description
TWO causes on the inspector2 filter surface: a malformed filterArn is routed to the store lookup instead of being refused by request validation, and the Filter projection drops `tags` when the filter has none.

Cause 1 of 2: UpdateFilter answers 404 ResourceNotFoundException for a filterArn that is not an ARN at all, where AWS answers 400 ValidationException.
AWS: Smithy: UpdateFilter.filterArn (required, FilterArn, @length min 1 max 128); modeled errors include ValidationException (httpError 400) and ResourceNotFoundException (httpError 404) — the model states no @pattern on FilterArn, so this is recorded behavior only for the exact wording. The recording shows AWS validating the ARN's SHAPE before any lookup: HTTP 400, error type ValidationException, message "Invalid Filter Arn <filterArn>". Fixed text: "Invalid Filter Arn "; variable part: the filterArn exactly as the request sent it. Only a syntactically well-formed filter ARN that names nothing reaches the 404 ResourceNotFoundException path.
Emulator: skips the shape check and goes straight to the store, answering HTTP 404 with x-amzn-errortype ResourceNotFoundException and message "The filter <filterArn> does not exist."
Why: the two outcomes are different error CLASSES with different retry/handling semantics — a client that distinguishes "you sent me garbage" (ValidationException, 400, never retried, surfaced as a caller bug) from "that filter is gone" (ResourceNotFoundException, 404, often treated as benign/idempotent on an update-or-create path) takes the wrong branch, silently treating a malformed-input bug as a missing resource. SDK error-type dispatch (catching ValidationException) also fails to fire.
Fix: UpdateFilter's handler/request validation — refuse a filterArn that is not a well-formed inspector2 filter ARN with 400 ValidationException and the "Invalid Filter Arn <filterArn>" message BEFORE the store lookup; leave the existing 404 ResourceNotFoundException for a well-formed ARN naming no filter.
Traces: inspector2-filter-lifecycle-and-validation#task#8.

Cause 2 of 2: ListFilters omits `tags` from a Filter that carries no tags, instead of serializing an empty map.
AWS: Smithy: ListFilters output.filters (required, FilterList of Filter); Filter.tags targets TagMap (a map, not required on the shape). The recording shows AWS emitting the member as a present, EMPTY object ({}) for a filter created without tags — i.e. the Filter projection always carries `tags`, empty when the filter has none.
Emulator: answers 200 with the filter present but `tags` absent from the object entirely ($.filters[0].tags is None), so the member is missing rather than empty.
Why: a client that reads the tag set unconditionally (e.g. iterating filter.tags, or diffing tags between ListFilters calls) sees a null/absent member where AWS gives an empty map — in strongly typed SDKs an absent map and an empty map are distinguishable, and tag-reconciliation code that treats absent as "unknown, leave alone" rather than "no tags" will refuse to remove the last tag or will crash dereferencing nothing. It also makes the listing's shape depend on whether tags happen to be set.
Fix: the Filter serializer/projection shared by ListFilters (and therefore the shape the filter listing emits) — always emit `tags`, as an empty map when the stored filter holds no tags.
Traces: inspector2-filter-lifecycle-and-validation#task#2 (cluster also covers inspector2-filter-lifecycle-and-validation#task#4).

## Evidence
Cause 1 — fixture inspector2-filter-lifecycle-and-validation step task#8, UpdateFilter: expected status 400, actual 404. $.message expected "Invalid Filter Arn not-an-arn-at-all", actual "The filter not-an-arn-at-all does not exist."; $headers.x-amzn-errortype expected the ValidationException shape (recorded as <shaped-not-asserted>), actual "ResourceNotFoundException". The value "not-an-arn-at-all" is the request's filterArn filling the message template, not part of the required text.

Cause 2 — fixture inspector2-filter-lifecycle-and-validation step task#2, ListFilters: expected status 200, actual 200; $.filters[0].tags: missing — expected {} (present, empty object), got None (member absent). Same divergence recurs at step task#4 of the same fixture (2 affected steps in the cluster).

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: POST /filters/update (UpdateFilter) with filterArn set to a string that is not an ARN (e.g. "not-an-arn-at-all") — fixture inspector2-filter-lifecycle-and-validation, step task#8. Emulator returns 404/ResourceNotFoundException; AWS returns 400/ValidationException with "Invalid Filter Arn <filterArn>".
3. Cause 2: POST /filters/create (CreateFilter) with no tags, then POST /filters/list (ListFilters) — fixture inspector2-filter-lifecycle-and-validation, steps task#2 and task#4. Inspect $.filters[0]: the emulator omits `tags`; AWS serializes "tags": {}.
