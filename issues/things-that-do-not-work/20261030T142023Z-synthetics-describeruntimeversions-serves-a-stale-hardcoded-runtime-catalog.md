Issue-Source: trace-sweep-divergence
Issue-Trace: synthetics-canary-create-validation-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: synthetics-canary-create-validation-errors synthetics-catalog-describe-runtime-versions
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics|DescribeRuntimeVersions|2xx->2xx|$.NextToken
Issue-Key: synthetics|DescribeRuntimeVersions|2xx->2xx|$.RuntimeVersions[*]
Issue-Commit: 9678b42cc4101899ff7d3545bb952c250af4da46

# synthetics: DescribeRuntimeVersions serves a stale hardcoded runtime catalog and paginates a response AWS returns whole

> _Note: this issue was identified against the code base at commit `9678b42cc4101899ff7d3545bb952c250af4da46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRuntimeVersions

## Description
Two net-new root causes in the synthetics DescribeRuntimeVersions read path. Both are about the same operation but are independent: one is the catalog data the operation serves, the other is the pagination envelope it wraps that catalog in. Both must be closed before this issue is submitted.

Cause 1 of 2: the runtime-version catalog the emulator serves is a stale, mis-ordered, wrong-length hardcoded list (fixture synthetics-catalog-describe-runtime-versions, step task#1; also visible at synthetics-canary-create-validation-errors#task#1).
AWS: DescribeRuntimeVersions (Smithy: POST /runtime-versions, code 200, output DescribeRuntimeVersionsResponse with members RuntimeVersions: RuntimeVersionList and NextToken: Token) returns the service's current catalog of supported canary runtimes. Each element is a RuntimeVersion (Smithy members: Language, VersionName, Description, ReleaseDate, DeprecationDate - DeprecationDate is set only "if this runtime version is deprecated"). The model does not enumerate the version names or fix the ordering, so for the contents this is recorded behavior only: the recording returns an 80-element list (indices 0..79 all diverge) ordered newest-release-first, led by the currently supported families - VersionName "syn-nodejs-5.3" (Language "nodejs", Description "Security fixes. Dependencies: Node JS 22.x"), then "syn-nodejs-playwright-9.0", "syn-nodejs-puppeteer-18.0", "syn-python-selenium-13.0", then older entries such as "syn-nodejs-5.2" - each carrying a ReleaseDate and DeprecationDate left unset for non-deprecated versions.
Emulator: answers 200 but with a different and much older list in a different order. At index 0 it serves VersionName "syn-nodejs-playwright-3.0" (Description "Dependencies: Node JS 22.x, Playwright 1.x", ReleaseDate 1754006400) where the recording has "syn-nodejs-5.3"; index 1 is "syn-nodejs-playwright-2.0" where the recording has "syn-nodejs-playwright-9.0"; index 2 is "syn-nodejs-puppeteer-11.0" where the recording has "syn-nodejs-puppeteer-18.0"; index 3 is "syn-nodejs-puppeteer-10.0" where the recording has "syn-python-selenium-13.0"; index 4 is "syn-python-selenium-5.1" where the recording has "syn-nodejs-5.2". Every one of the 80 compared indices mismatches, so the emulator's catalog shares neither contents nor ordering with the real one, and the newest supported runtime families (the nodejs-5.x, playwright-9.x, puppeteer-18.x and python-selenium-13.x lines) are absent entirely. The Description strings it does serve are also not the service's text (e.g. "Dependencies: Node JS 22.x, Playwright 1.x" versus the recording's "Security fixes. Dependencies: Node JS 22.x, Playwright 1.63.0, Chromium 153.0.8010.36, Firefox 155.0").
Why: DescribeRuntimeVersions is the discovery call clients and the console use to pick a RuntimeVersion before CreateCanary/UpdateCanary. A client that enumerates it and selects the newest supported runtime gets a years-old version; a client that validates a user-supplied runtime name against this list rejects every currently supported runtime (e.g. syn-nodejs-puppeteer-18.0) as unknown, and a client that filters on DeprecationDate to warn about end-of-life runtimes sees a catalog in which the deprecation state of the real runtimes is unrepresentable. Ordering matters too: callers that take element 0 as "latest" get a stale pick rather than an error they could detect.
Fix: the DescribeRuntimeVersions handler's runtime-version catalog in the synthetics service state/seed data - the static RuntimeVersion table it serves must be refreshed to the catalog the recording shows, kept ordered newest-release-first, and must leave DeprecationDate unset for non-deprecated versions while setting it for deprecated ones. This is data, not control flow; no other operation needs to change for this cause.
Traces: synthetics-catalog-describe-runtime-versions#task#1 (all 80 indices), synthetics-canary-create-validation-errors#task#1 ($.RuntimeVersions[0]).

Cause 2 of 2: a spurious NextToken is returned for a catalog AWS serves in one page (fixture synthetics-canary-create-validation-errors, step task#1).
AWS: DescribeRuntimeVersions is modeled paginated (Smithy paginated: inputToken NextToken, outputToken NextToken, pageSize MaxResults; input MaxResults is MaxSize100 @range min 1 max 100, input NextToken is Token @length min 4 max 252). NextToken is an optional output member and, per the pagination contract, is present only when there is a further page to fetch. In the recording the request draws the catalog without a caller-supplied page size and the response carries RuntimeVersions with NO NextToken member at all, i.e. the whole catalog is one page and iteration terminates on that response.
Emulator: answers 200 but includes NextToken with an emulator-authored cursor value of the form "rv#<VersionName>" (observed "rv#syn-python-selenium-2.0" - the "rv#" prefix is fixed emulator text and the suffix is a version name taken from its own catalog position, so no part of this value is a requirement) where the recording has no NextToken, meaning it silently truncated the catalog into a page and advertised a continuation AWS does not advertise.
Why: a paginating client (any SDK paginator, or hand-rolled while-NextToken loop) treats a present NextToken as "more results exist" and issues another DescribeRuntimeVersions with that token. Against AWS the loop ends after one call; against the emulator it makes extra calls, and a client that instead reads only the first page sees a truncated catalog and concludes runtimes are unsupported that are. Because the token is also an opaque emulator-internal cursor, any client that persists or compares tokens across calls is relying on a shape AWS never produced here.
Why both are one issue: they are the same read path, so a fixer touching the catalog is already in the handler that builds this envelope; they are nonetheless two distinct defects (wrong data vs wrong pagination envelope) and closing one does not close the other.
Fix: the DescribeRuntimeVersions handler's pagination/serialization step - emit NextToken only when the catalog is actually truncated (caller-supplied MaxResults, or a page size smaller than the catalog), and omit the member entirely when the response carries the final page.
Traces: synthetics-canary-create-validation-errors#task#1 ($.NextToken).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200).

Cause 1 - fixture synthetics-catalog-describe-runtime-versions, step task#1, DescribeRuntimeVersions, verdict mismatch, expected status 200 / actual status 200. All 80 list indices diverge ($.RuntimeVersions[0] .. $.RuntimeVersions[79]):
  $.RuntimeVersions[0]: expected {Language 'nodejs', VersionName 'syn-nodejs-5.3', Description 'Security fixes. Dependencies: Node JS 22.x', ReleaseDate 1790553600, DeprecationDate unset}, got {Language 'nodejs', VersionName 'syn-nodejs-playwright-3.0', Description 'Dependencies: Node JS 22.x, Playwright 1.x', ReleaseDate 1754006400}
  $.RuntimeVersions[1]: expected VersionName 'syn-nodejs-playwright-9.0' (Description 'Security fixes. Dependencies: Node JS 22.x, Playwright 1.63.0, Chromium 153.0.8010.36, Firefox 155.0'), got 'syn-nodejs-playwright-2.0'
  $.RuntimeVersions[2]: expected VersionName 'syn-nodejs-puppeteer-18.0', got 'syn-nodejs-puppeteer-11.0'
  $.RuntimeVersions[3]: expected VersionName 'syn-python-selenium-13.0' (Language 'python'), got 'syn-nodejs-puppeteer-10.0' (Language 'nodejs')
  $.RuntimeVersions[4]: expected VersionName 'syn-nodejs-5.2', got 'syn-python-selenium-5.1'
Same cause at fixture synthetics-canary-create-validation-errors, step task#1: $.RuntimeVersions[0] expected VersionName 'syn-nodejs-5.2', got 'syn-nodejs-playwright-3.0'.

Cause 2 - fixture synthetics-canary-create-validation-errors, step task#1, DescribeRuntimeVersions, verdict mismatch, expected status 200 / actual status 200:
  $.NextToken: type_mismatch - expected no NextToken member (None), got 'rv#syn-python-selenium-2.0'.

Model grounding: Smithy DescribeRuntimeVersions -> POST /runtime-versions (200), output {RuntimeVersions: RuntimeVersionList, NextToken: Token @length 4..252}, paginated {inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults}, MaxResults: MaxSize100 @range 1..100; RuntimeVersion members Language/VersionName/Description/ReleaseDate/DeprecationDate, DeprecationDate documented as set only when the version is deprecated.

Not covered by any open issue: the open synthetics issues concern error-message wording (20261029T225859Z-synthetics-client-error-messages-are-emulator-authored-instead.md), tags on the wire (20261029T225859Z-synthetics-tags-never-surface-on-the-wire-creategroup.md), association/state conflicts (20261029T184613Z-synthetics-4-net-new-root-causes-duplicate-associateresource.md), dry-run lifecycle (20261030T021959Z-synthetics-a-dry-run-has-no-lifecycle-of.md) and unimplemented stubs; none names DescribeRuntimeVersions or carries these replay keys.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 2 (and cause 1's second witness): replay fixture synthetics-canary-create-validation-errors, step task#1 - call DescribeRuntimeVersions with no MaxResults/NextToken. Observe the 200 response carries a NextToken ('rv#<version-name>') where the recording carries none, and that $.RuntimeVersions[0] is not the recorded newest runtime.
4. Cause 1: replay fixture synthetics-catalog-describe-runtime-versions, step task#1 - call DescribeRuntimeVersions and compare the full RuntimeVersions list against the recording. Observe all 80 indices diverge: the served catalog is ordered differently and omits the current nodejs-5.x / playwright-9.x / puppeteer-18.x / python-selenium-13.x runtime families, serving older versions and non-service Description text instead.
5. Both causes are closed only when DescribeRuntimeVersions serves the recorded catalog, newest-release-first with DeprecationDate set only for deprecated versions, AND omits NextToken when the response is the final page.
