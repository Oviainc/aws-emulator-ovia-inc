Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files
Issue-Key: amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files|_http_until#13|Timeout|time.sleep(3.0)
Issue-Commit: f2dfb6e6c8bef3e3a2af1e117b6fc51f81eb645a

# Amplify hosting edge never applies an app's customRules 200-rewrites: the rewritten source path keeps answering 404

> _Note: this issue was identified against the code base at commit `f2dfb6e6c8bef3e3a2af1e117b6fc51f81eb645a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApp

## Description
Cause 1 of 1: an Amplify app's `customRules` have no effect on what the hosting edge (`https://<branch displayName>.<app defaultDomain>/`) serves. A 200-rewrite rule installed with UpdateApp never starts mapping its `source` pattern onto its `target` path, so the referenced path stays a 404 forever and the check's convergence poll never completes (the harness kills it at 60s; the check's own HTTP convergence budget is 180s and its job budget is 240s, both longer, so the timeout means a poll never turned green rather than that the budget was merely tight).

AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models `UpdateApp.customRules` as `CustomRules`, a list of `CustomRule{source (required, @length 1..2048), target (required, @length 1..2048), status (@length 3..7), condition}`; the `status` member is documented with `200` meaning "Represents a 200 rewrite rule". UpdateApp returns 200 with the updated `app`, and GetApp returns the same `app`, so `customRules` round-trips. The model describes only the control plane; the hosting-edge effect is recorded behavior only: on real AWS, after `customRules` are set, a raw GET to a path matching `source` is served, with status 200, from the object at `target`, with the `<*>` placeholder in `source` substituted into `target` (so `source=/assets/<*>` + `target=/styles/<*>` makes `GET /assets/main.css` return the bytes deployed at `styles/main.css`), and a literal rule (`source=/build/app.bundle.js`, `target=/scripts/app.js`) returns the bytes deployed at `scripts/app.js`. Before any rule exists, the same GET is a 404 — that part the emulator already gets right.

Emulator: the check reaches the harness ceiling with the rewritten path never answering 200. The pre-rule assertion (`assets/main.css` -> 404) is the state the emulator stays in after UpdateApp installs the two rules, so the rewrite is never consulted when the edge resolves a request path. Hypothesis (neither the model nor the replay output localises it further, because the only detail the suite reports for this check is `Check timed out after 60.0s`): either UpdateApp accepts and stores `customRules` but the hosting-edge request path never reads them, or it does not persist them at all. The sibling amplify__s3 checks pass (4/5 for the group), so the CreateApp/CreateBranch/StartDeployment-from-S3 path and plain edge serving do converge; the rewrite is the step this check adds.

Why: a client that uses Amplify hosting rewrites — the standard way an SPA maps public asset paths and a single-page-app catch-all onto what the build actually deployed — gets 404 for every rewritten path. Nothing surfaces the failure at the API: UpdateApp succeeds and GetApp echoes the rules back, so the app looks correctly configured while the edge serves none of it. Code that polls the edge for the rewrite to take effect (the documented behavior, since the edge is not promptly consistent) hangs until its own deadline instead of converging.

Fix: the Amplify hosting-edge data-plane handler that resolves a request path for `<branch displayName>.<app defaultDomain>` must consult the owning app's `customRules` before it answers 404 — match each rule's `source` against the request path (including `<*>` placeholder matching), and for `status` "200" serve the deployed object named by the substituted `target` with status 200 and that object's own Content-Type. It must also be verified that `UpdateApp` (services/service-com-amazonaws-amplify-amplify/src/ops/update_app.rs) persists `customRules` into the app state that the edge reads, not only into the record GetApp echoes.

Traces: amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files (group hosting-rules). No open issue covers this: the only open amplify issues are the TagResource and UntagResource scaffold stubs (20261023T144746Z-amplify-tagresource-stub.md, 20261023T144746Z-amplify-untagresource-stub.md) and an amplifybackend error-contract issue, none of which touch customRules or the hosting edge.

## Evidence
Failing check: [mismatch] amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files
Suite detail: `Error: Check timed out after 60.0s`
Group pass rate: amplify__s3 4/5 (80%), this is the only failure.

Expected (real AWS): after `update_app(appId=..., customRules=[{source:'/assets/<*>', target:'/styles/<*>', status:'200'}, {source:'/build/app.bundle.js', target:'/scripts/app.js', status:'200'}])`, `GetApp` echoes those rules AND a raw `GET https://<displayName>.<defaultDomain>/assets/main.css` converges to `200` with a body byte-equal to the file deployed at `styles/main.css`, and `GET /build/app.bundle.js` answers `200` with the bytes deployed at `scripts/app.js`.

Actual (emulator): the rewritten paths keep answering the pre-rule `404`; `_http_until(url + 'assets/main.css', status == 200, 'the /assets/<*> rewrite takes effect')` never satisfies its predicate and the check is killed at the 60s harness ceiling (the check's own HTTP deadline is 180s, its job deadline 240s). The control assertion the emulator does satisfy is the pre-rule one: `assets/main.css` answers 404 before any rule is installed.

## Steps to Reproduce
1. Create a source bucket (versioned, SSE-S3, all public access blocked, enforceSSL Deny statement) and `put_object` a zip whose entries are `index.html`, `styles/main.css`, `scripts/app.js` at key `site.zip`.
2. Append an Allow statement for the `amplify.amazonaws.com` service principal (`s3:GetObject`, `s3:ListBucket`) to the bucket policy.
3. `amplify.create_app(name=...)`, then `amplify.create_branch(appId=..., branchName='main')`.
4. `amplify.start_deployment(appId=..., branchName='main', sourceUrl='s3://<bucket>/site.zip', sourceUrlType='ZIP')` and poll `get_job` to `SUCCEED`.
5. Build the edge URL from the API: `https://{get_branch(...).branch.displayName}.{get_app(...).app.defaultDomain}/`; poll it until it answers 200.
6. `GET <url>assets/main.css` — expect 404 (emulator agrees).
7. `amplify.update_app(appId=..., customRules=[{'source':'/assets/<*>','target':'/styles/<*>','status':'200'},{'source':'/build/app.bundle.js','target':'/scripts/app.js','status':'200'}])`; confirm `get_app(...).app.customRules` equals the list set.
8. Poll `GET <url>assets/main.css` for a 200. On real AWS it converges within seconds and the body equals the seeded `styles/main.css`; on the emulator it stays 404 indefinitely.
9. `GET <url>build/app.bundle.js` — expect 200 with the bytes of `scripts/app.js`.

Replay: run_behavioral_suite and re-run check amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files.
