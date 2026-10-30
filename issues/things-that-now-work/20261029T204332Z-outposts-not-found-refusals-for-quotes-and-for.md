Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: outposts.delete-quote-removes-it outposts.site-to-outpost-to-quote-e2e-lifecycle outposts.tag-resource-nonexistent-site-rejected
Issue-Shape-Id: com.amazonaws.outposts#OutpostsOlafService
Issue-Key: outposts.delete-quote-removes-it|run#8|AssertionError|assert message == expected, f"unexpected GetQuote message for a deleted quote: {message!r}"
Issue-Key: outposts.site-to-outpost-to-quote-e2e-lifecycle|run#34|AssertionError|assert message == f"Could not find Quote with identifier {quote_id}", (
Issue-Key: outposts.tag-resource-nonexistent-site-rejected|run#9|AssertionError|assert message == "Resource was not found", f"unexpected TagResource message: {message!r}"
Issue-Commit: 8dc4d2e19b7449813b28120a27a1ef82acd340ce

# outposts not-found refusals for quotes and for tagged resources carry the "<member>=<value>" identifier echo instead of AWS's not-found wording

> _Note: this issue was identified against the code base at commit `8dc4d2e19b7449813b28120a27a1ef82acd340ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQuote

## Description
2 net-new root causes. In both, the HTTP status (404) and the error class (NotFoundException) are already correct; the defect is the Message TEXT only. The emulator words every not-found refusal with one shared "identifier echo" helper that renders `<requestMemberName>=<value as supplied>`; AWS Outposts words each not-found refusal as a sentence whose text differs per resource kind. In each cause the quoted expected text is AWS's FIXED text and only the named slots are variable (taken from the request); the recorded ids below are per-run fillers for those slots, never part of the requirement.

Related OPEN issues that do NOT cover these as filed: 20261029T191039Z-outposts-not-found-refusals-echo-the-identifier-instead is the same family from the WIRE corpus, but it enumerates GetOrder/CancelOrder/GetQuote/DeleteQuote/GetCapacityTask/CancelCapacityTask/CreateOrder/DeleteOutpost/StartConnection and carries no behavioral replay key for the checks below, and it does not mention TagResource or ListTagsForResource at all; 20261024T134249Z-outposts-tagresource-validation is about TagResource resolving a ResourceArn by resource id alone (ignoring partition/Region/account), which is a different cause and does not change the refusal text.

Cause 1 of 2: GetQuote (and DeleteQuote) for a QuoteIdentifier that does not exist — including one that existed and was deleted.
AWS: 404 NotFoundException (Smithy: GetQuote and DeleteQuote both list NotFoundException errorKind client httpError 404; QuoteIdentifier is a required httpLabel, pattern ^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:quote/)?oq-[a-f0-9]{17}$) with Message `Could not find Quote with identifier <quoteIdentifier>` — fixed text "Could not find Quote with identifier " plus the identifier exactly as supplied in the request. Recorded behavior supplies the wording; the model documents no message text. The dead QuoteId is unusable, not merely unreadable: a SECOND DeleteQuote on the same identifier fails with the same code and the same Message rather than no-opping.
Emulator: 404 NotFoundException (status and code correct) with Message `quoteIdentifier=oq-f90adf12335872861` — the bare member-name=value echo, after a successful DeleteQuote of that quote.
Why: a client that matches on or surfaces the service's message text (log scraping, user-facing error display, SDK error-message assertions) sees a string AWS never emits, so handling keyed on AWS's wording never fires; the echo form is also indistinguishable from the emulator's validation-failure text for the same member. Both failing checks stopped at the GetQuote message, so the second-DeleteQuote leg of outposts.delete-quote-removes-it and the DeleteOutpost/DeleteSite not-found legs that follow in outposts.site-to-outpost-to-quote-e2e-lifecycle are unverified and must be re-replayed with the fix (that e2e check separately requires GetOutpost's post-delete Message to be `outpostId=<outpostId>` and GetSite's to be `siteId=<siteId>`, so the echo form is correct for THOSE two and must not be rewritten along with the quote paths).
Fix: the not-found arms of the GetQuote and DeleteQuote handlers (services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/get_quote.rs and src/ops/delete_quote.rs) and the shared identifier-echo message helper they call — the helper must stop being the message on these not-found paths.
Traces: outposts.delete-quote-removes-it, outposts.site-to-outpost-to-quote-e2e-lifecycle.

Cause 2 of 2: TagResource (and ListTagsForResource) for a well-formed ResourceArn naming a resource that does not exist.
AWS: 404 NotFoundException (Smithy: TagResource and ListTagsForResource each list NotFoundException errorKind client httpError 404 and ValidationException 400; ResourceArn is a required httpLabel, pattern ^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$) with the wholly FIXED Message `Resource was not found` — no variable parts at all: AWS does NOT echo the ARN here. The tagging APIs perform their own existence check rather than writing or reading a tag record for any syntactically valid ARN, and the identical Tags payload against a real Site ARN succeeds.
Emulator: 404 NotFoundException (status and code correct) with Message `resourceArn=arn:aws:outposts:us-west-2:000000000000:site/os-00000000000000000` — the identifier echo, which both uses the wrong text and leaks the supplied ARN into a message AWS keeps constant.
Why: callers that recognize AWS's constant "Resource was not found" string (or assert on it) never match; and a message that varies with the input where AWS's is fixed defeats any equality-based handling. The ListTagsForResource leg and the positive control (TagResource then ListTagsForResource against the real Site ARN) were not reached because the check stops at this assertion, so both are unverified and should be re-replayed with the fix.
Fix: the not-found arms of the TagResource and ListTagsForResource handlers (src/ops/tag_resource.rs and src/ops/list_tags_for_resource.rs) and the shared identifier-echo message helper — these two paths must emit AWS's constant sentence, not the echo.
Traces: outposts.tag-resource-nonexistent-site-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout (outposts 18/25 passed). All three failures are status- and code-correct (404 / NotFoundException) and Message-divergent:

- [mismatch] outposts.delete-quote-removes-it: CreateQuote(CountryCode=US, RequestedCapacities=[{EC2, c5.24xlarge, 4.0}]) -> DeleteQuote -> GetQuote. AssertionError: "unexpected GetQuote message for a deleted quote: 'quoteIdentifier=oq-f90adf12335872861'"; expected Message == "Could not find Quote with identifier oq-f90adf12335872861" (fixed text + the supplied identifier).

- [mismatch] outposts.site-to-outpost-to-quote-e2e-lifecycle: same divergence inside the end-to-end teardown. AssertionError: "unexpected GetQuote message after DeleteQuote: 'quoteIdentifier=oq-e31cd66f1710b4661'"; expected "Could not find Quote with identifier oq-e31cd66f1710b4661". Everything before it passed: CreateSite, CreateOutpost (SiteId/Name/SupportedHardwareType=RACK echoed, OutpostArn ending in outpost/<outpostId>), GetOutpost, CreateQuote (QuoteStatus CREATED, RequestedCapacities[0].Unit c5.24xlarge), GetSite, GetQuote and ListOutposts linkage, and DeleteQuote itself.

- [mismatch] outposts.tag-resource-nonexistent-site-rejected: CreateSite, then tag_resource(ResourceArn=<real site ARN with the id replaced by os-00000000000000000>, Tags={'sim2real-key':'v'}). AssertionError: "unexpected TagResource message: 'resourceArn=arn:aws:outposts:us-west-2:000000000000:site/os-00000000000000000'"; expected Message == "Resource was not found". The code returned was NotFoundException, so only the text diverges; the ARN's partition/Region/account came from the emulator's own freshly minted SiteArn.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 suite (service outposts).
2. Cause 1 — CreateQuote(CountryCode="US", RequestedCapacities=[{QuoteCapacityType:"EC2", Unit:"c5.24xlarge", Quantity:4.0}]), DELETE /quotes/{quoteId}, then GET /quotes/{quoteId}. Observe 404 with body {"Message":"quoteIdentifier=<quoteId>"} where AWS answers {"Message":"Could not find Quote with identifier <quoteId>"}. Then DELETE /quotes/{quoteId} a second time: it must also answer 404 NotFoundException with that same Message (currently unverified).
3. Cause 1, second binding — run the full site->outpost->quote lifecycle and its teardown; after DeleteQuote the GetQuote refusal shows the same echo. Note that in the SAME teardown, GetOutpost after DeleteOutpost must keep Message "outpostId=<outpostId>" and GetSite after DeleteSite must keep "siteId=<siteId>": do not convert those two to a sentence.
4. Cause 2 — CreateSite, take its SiteArn, replace the site id with os-00000000000000000 (well-formed, never created), then POST /tags/{thatArn} with Tags={"k":"v"}. Observe 404 with the resourceArn echo where AWS answers {"Message":"Resource was not found"}. Repeat with GET /tags/{thatArn} (ListTagsForResource), which must answer the identical code and Message, and confirm the positive control still works: TagResource then ListTagsForResource against the REAL SiteArn returns the tag.
