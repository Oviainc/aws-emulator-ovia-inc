Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__s3.bucket-prefix-deploy-publishes-nothing-outside-the-prefix amplify__s3.bucket-prefix-deploy-serves-the-seeded-index-byte-for-byte amplify__s3.e2e-amplify-deploy-frontend-from-s3-lifecycle amplify__s3.file-map-deployment-uploads-each-s3-file-by-its-own-url-and-serves-them amplify__s3.redeploy-from-an-updated-s3-index-replaces-the-served-page amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files amplify__s3.seeded-index-references-asset-paths-the-deployment-answers-404-for amplify__s3.two-branches-deployed-from-different-bucket-prefixes-each-serve-their-own-index
Issue-Key: amplify__s3.bucket-prefix-deploy-publishes-nothing-outside-the-prefix|_http#12|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify__s3.bucket-prefix-deploy-serves-the-seeded-index-byte-for-byte|_http#15|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify__s3.e2e-amplify-deploy-frontend-from-s3-lifecycle|phase#9|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: amplify__s3.file-map-deployment-uploads-each-s3-file-by-its-own-url-and-serves-them|_get#21|PublishedUrlRejected|target, headers = ctx.endpoint_route(url)
Issue-Key: amplify__s3.redeploy-from-an-updated-s3-index-replaces-the-served-page|_get#14|PublishedUrlRejected|target, headers = ctx.endpoint_route(url)
Issue-Key: amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files|_http#13|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify__s3.seeded-index-references-asset-paths-the-deployment-answers-404-for|_http#13|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: amplify__s3.two-branches-deployed-from-different-bucket-prefixes-each-serve-their-own-index|_get#13|PublishedUrlRejected|target, headers = ctx.endpoint_route(url)
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# amplify__s3: every S3-sourced deployment check stops at the published branch URL — App.defaultDomain announces an unreachable <appId>.amplifyapp.com host

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApp

## Description
One net-new root cause, grounded in all eight failing amplify__s3 checks.

Cause 1 of 1: the app projection publishes `App.defaultDomain` as a real-AWS `<appId>.amplifyapp.com` hostname, so the branch hosting URL a client builds from the API's own output names a host the emulator does not serve. Every S3-sourced deployment scenario is blocked at that point and none of the hosting behavior it tests is ever exercised.

  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models GetApp -> GET /apps/{appId}, 200 with `app` @required; `App$defaultDomain` is documented "The default domain for the Amplify app." On real AWS it is the host Amplify's hosting edge answers on, so `https://<branch displayName>.<defaultDomain>/` is a live address: a GET returns the bytes the deployment published, 404 for a path the deployment does not contain, the rewritten asset for a rewrite rule, and a per-branch page for each branch deployed from its own bucket prefix. The requirement is that the announced host be one the client that received it can reach; the specific host and port are environment-specific and are never the requirement.

  Emulator: services/service-com-amazonaws-amplify-amplify/src/ops/create_app.rs sets `default_domain: format!("{app_id}.amplifyapp.com")`. The replay harness refuses the published address: `PublishedUrlRejected: endpoint_url was given https://<branch>.<appId>.amplifyapp.com/ ... its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. The checks construct the URL exactly as a client would, from GetApp.app.defaultDomain and GetBranch.branch.displayName, so the defect is the address the emulator announced.

  Why: a client that deploys a frontend from S3 (StartDeployment with a sourceUrl / file map) then fetches the branch URL to confirm what was published. Against the emulator that fetch leaves the emulator entirely, so prefix scoping (nothing outside the prefix is published), byte-for-byte serving of the seeded index, per-file URL uploads, redeploy replacing the served page, rewrite-rule mapping, the 404 for unreferenced asset paths, and two branches serving their own index are all unobservable — the client cannot verify any deployment it made.

  Fix: mint the app record's `defaultDomain` from the emulator's own published hosting address instead of a synthesized AWS zone (services/service-com-amazonaws-amplify-amplify/src/ops/create_app.rs, `default_domain` assignment, plus the shared app projection in src/state.rs), and route the emulator's hosting edge for that announced host so a GET of `https://<displayName>.<defaultDomain>/<path>` is answered by the emulator that published it.

  Note on existing issues: no open amplify or amplify__s3 issue carries this cause — 20261102T085926Z-amplify-s3-a-create-complete-stack-s-aws.md covers an unapplied AWS::S3::BucketPolicy, and the other open amplify issues cover control-plane projections, job lifecycle, CFN resource types and scaffold stubs. The same root cause is filed separately for the `amplify` service heading, because an issue carries one service token.

  Traces: all eight failing amplify__s3 checks, listed in trace_ids.

## Evidence
All eight amplify__s3 failures carry the identical refusal of the address the emulator published:
  [mismatch] amplify__s3.bucket-prefix-deploy-publishes-nothing-outside-the-prefix — `PublishedUrlRejected: endpoint_url was given https://<branch>.<appId>.amplifyapp.com/ ... its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`
  [mismatch] amplify__s3.bucket-prefix-deploy-serves-the-seeded-index-byte-for-byte — same
  [mismatch] amplify__s3.e2e-amplify-deploy-frontend-from-s3-lifecycle — `AssertionError: [phase 4/7 solve] endpoint_url was given https://main.d07hfbvttxs8ev.amplifyapp.com/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:38565/<resource path>, ...`
  [mismatch] amplify__s3.file-map-deployment-uploads-each-s3-file-by-its-own-url-and-serves-them — same
  [mismatch] amplify__s3.redeploy-from-an-updated-s3-index-replaces-the-served-page — same
  [mismatch] amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files — same
  [mismatch] amplify__s3.seeded-index-references-asset-paths-the-deployment-answers-404-for — same
  [mismatch] amplify__s3.two-branches-deployed-from-different-bucket-prefixes-each-serve-their-own-index — same
(The host labels and ports are per-run values and are not part of the requirement.)

The URL is built from the API's own output: `domain = get_app(appId)['app']['defaultDomain']; display = get_branch(appId, branch)['branch']['displayName']; url = f"https://{display}.{domain}/"`.
Source: services/service-com-amazonaws-amplify-amplify/src/ops/create_app.rs — `default_domain: format!("{app_id}.amplifyapp.com")`.
Expected: `defaultDomain` names a host the emulator serves, and a GET of the branch URL returns the deployed object's bytes (200), 404 for an unpublished path, and the per-branch page for each branch.

## Steps to Reproduce
1. s3.create_bucket(<unique>); put index.html (and assets) under a prefix in that bucket.
2. amplify.create_app(name=<unique>) -> appId; amplify.create_branch(appId=appId, branchName='main').
3. amplify.start_deployment(appId=appId, branchName='main', sourceUrl='s3://<bucket>/<prefix>', sourceUrlType='BUCKET_PREFIX'); poll amplify.get_job until the job status is SUCCEED.
4. domain = amplify.get_app(appId=appId)['app']['defaultDomain'] -> observed `<appId>.amplifyapp.com`, a host on AWS's production zone that the emulator does not serve.
5. display = amplify.get_branch(appId, 'main')['branch']['displayName']; GET https://{display}.{domain}/index.html .
   Observed: the published address is not one of the emulator's own hosts, so the fetch never reaches the emulator.
   Expected: the GET is answered by the emulator with the deployed index.html byte-for-byte (200); a path the deployment did not publish answers 404; a rewrite rule maps the referenced asset path onto the seeded file; a second branch deployed from a different prefix serves its own index.

Replay: run_behavioral_suite and re-run the checks listed in trace_ids.
