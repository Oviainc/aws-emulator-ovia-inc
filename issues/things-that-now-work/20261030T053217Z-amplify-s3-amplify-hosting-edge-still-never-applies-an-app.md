Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files
Issue-Key: amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files|_http_until#21|Timeout|time.sleep(3.0)
Issue-Commit: 11f4c8cad091e31e006092cf5180555ad0976c5e

# Amplify hosting edge still never applies an app's customRules 200-rewrites: the rewritten path keeps answering 404 and the convergence poll never completes

> _Note: this issue was identified against the code base at commit `11f4c8cad091e31e006092cf5180555ad0976c5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApp

## Description
One net-new root cause grounded in a failing behavioral check whose id carries the compound service token `amplify__s3` (the check exercises amplify's hosting edge over content deployed from an s3 source; both constituent services are stood up). The divergence is Amplify's.

NOTE ON OVERLAP: open issue 20261029T122518Z-amplify-s3-amplify-hosting-edge-never-applies-an-app-s.md describes this same check id and the same symptom, but the replay report did NOT mark this failure as filed or OWNED by it (the report marks every already-filed failure, and it marked the three ListBackendEnvironments/ListWebhooks/UntagResource stub blockers in this same run). The check is therefore re-filed here with the current observation so it is not lost; if triage confirms the two are the same cause, merge this into that issue.

Cause 1 of 1: an app's `customRules` have no effect on what the hosting edge (`https://<branch displayName>.<app defaultDomain>/`) serves, so a path covered by a 200-rewrite rule keeps answering 404 forever.

  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models UpdateApp with `customRules` as `CustomRules`, a list of `CustomRule{source @required @length 1..2048, target @required @length 1..2048, status @length 3..7, condition}`, and documents `status` `200` as "Represents a 200 rewrite rule". The control-plane round-trip (UpdateApp 200 -> GetApp echoes the rules) is modeled; the edge effect is recorded behavior only: on real AWS, once the rules are installed, a raw GET of a path matching `source` is answered 200 with the bytes of the object deployed at `target`, with the `<*>` placeholder substituted from source into target (`source=/assets/<*>`, `target=/styles/<*>` makes `GET /assets/main.css` serve what was deployed at `styles/main.css`), and a literal rule (`source=/build/app.bundle.js`, `target=/scripts/app.js`) serves `scripts/app.js`. Before any rule exists the same GET is 404 — behavior the emulator already matches.

  Emulator: the check is killed at the harness ceiling with the rewritten path never turning 200. Its own budgets are longer than the ceiling (HTTP convergence 180s, job 240s), so the ceiling means a poll never went green, not that the budget was merely tight. The pre-rule control (`assets/main.css` -> 404) is the state the emulator stays in after UpdateApp installs both rules. Hypothesis (the only detail the suite reports for this check is `Check timed out after 60.0s`, so the hang is not localised further): either UpdateApp persists `customRules` only into the record GetApp echoes and the edge request path never reads them, or the edge has no rewrite-resolution step at all. The sibling amplify__s3 checks pass (4/5 for the group), so CreateApp/CreateBranch/StartDeployment-from-S3 and plain edge serving do converge; the rewrite is the step this check adds.

  Why: Amplify hosting rewrites are the standard way an app maps its public asset paths (and an SPA catch-all) onto what the build actually deployed. Nothing surfaces the failure at the API — UpdateApp succeeds and GetApp echoes the rules back — so the app looks correctly configured while the edge serves 404 for every rewritten path, and a client that polls the edge for the rewrite to take effect (correct, since the edge is not promptly consistent) hangs until its own deadline instead of converging.

  Fix: the Amplify hosting-edge data-plane handler that resolves a request path for `<branch displayName>.<app defaultDomain>` must consult the owning app's `customRules` before answering 404 — match each rule's `source` against the request path including `<*>` placeholder matching, and for `status` "200" serve the deployed object named by the substituted `target` with status 200 and that object's Content-Type. It must also be verified that services/service-com-amazonaws-amplify-amplify/src/ops/update_app.rs persists `customRules` into the state the edge reads, not only into the record GetApp returns.

  Traces: amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files (group hosting-rules).

## Evidence
Failing check: [mismatch] amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files
Suite detail: `Error: Check timed out after 60.0s` (the only detail reported).
Group pass rate: amplify__s3 4/5 (80%) — this is the group's only failure.

Expected (real AWS): after update_app(appId=..., customRules=[{source:'/assets/<*>', target:'/styles/<*>', status:'200'}, {source:'/build/app.bundle.js', target:'/scripts/app.js', status:'200'}]), GetApp echoes those rules AND a raw `GET https://<displayName>.<defaultDomain>/assets/main.css` converges to 200 with a body byte-equal to the file deployed at `styles/main.css`, and `GET /build/app.bundle.js` answers 200 with the bytes deployed at `scripts/app.js`.

Actual (emulator): the rewritten paths keep answering the pre-rule 404; the check's `_http_until(url + 'assets/main.css', status == 200, 'the /assets/<*> rewrite takes effect')` never satisfies its predicate and the check is killed at the 60s harness ceiling, below its own 180s HTTP / 240s job budgets. The control assertion the emulator does satisfy is the pre-rule one: `assets/main.css` answers 404 before any rule is installed. Host, generated app id, branch display name and domain are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Create a source bucket (versioned, SSE-S3, all public access blocked, enforceSSL Deny statement) and s3.put_object a zip whose entries are `index.html`, `styles/main.css`, `scripts/app.js` at key `site.zip`.
2. Append an Allow statement for the `amplify.amazonaws.com` service principal (s3:GetObject, s3:ListBucket) to the bucket policy.
3. amplify.create_app(name=<unique>) -> appId; amplify.create_branch(appId=appId, branchName='main').
4. amplify.start_deployment(appId=appId, branchName='main', sourceUrl='s3://<bucket>/site.zip', sourceUrlType='ZIP'); poll get_job to SUCCEED.
5. Build the edge URL from the API: https://{get_branch(...).branch.displayName}.{get_app(...).app.defaultDomain}/ and poll it until it answers 200.
6. GET <url>assets/main.css -> expect 404 (emulator agrees).
7. amplify.update_app(appId=appId, customRules=[{'source':'/assets/<*>','target':'/styles/<*>','status':'200'},{'source':'/build/app.bundle.js','target':'/scripts/app.js','status':'200'}]); confirm get_app(...).app.customRules equals the list set.
8. Poll GET <url>assets/main.css for a 200. On real AWS it converges within seconds with the body of the seeded styles/main.css; on the emulator it stays 404 until the deadline.
9. GET <url>build/app.bundle.js -> expect 200 with the bytes of scripts/app.js.

Replay: run_behavioral_suite and re-run check amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files.
