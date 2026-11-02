Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401 amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401|_http_until#12|Timeout|time.sleep(3.0)
Issue-Key: amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses|run#12|JSONDecodeError|assert json.loads(stored) == [
Issue-Commit: fec48c0e4a4ef614c94c9aeb73915e609cba5726

# amplify hosting edge: branch basic auth is never enforced, GetBranch echoes the submitted credentials verbatim, and customHeaders is neither re-serialized to JSON nor applied to responses

> _Note: this issue was identified against the code base at commit `fec48c0e4a4ef614c94c9aeb73915e609cba5726`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateBranch

## Description
Four net-new root causes in the amplify data plane (the hosting edge this crate serves for a branch's default domain) and in the representation of the two app/branch members that drive it. Each must be closed before this issue is submitted.

Cause 1 of 4: the hosting edge does not enforce branch basic auth — a branch with enableBasicAuth=true serves its site to anonymous and wrong-password requests.
  AWS: Smithy models CreateBranch/UpdateBranch.enableBasicAuth (boolean) and basicAuthCredentials (BasicAuthCredentials, @length 0..2000, @sensitive), documented "the basic authorization credentials for the branch. You must base64-encode the authorization credentials and provide them in the format user:password". Recorded behavior only (the edge is plain HTTPS, not a modeled operation): after UpdateBranch(enableBasicAuth=True, basicAuthCredentials=base64('user:password')), a GET of the branch's default-domain URL with no Authorization header — and one with the wrong password — answers HTTP 401 carrying a `WWW-Authenticate: Basic ...` challenge, while a GET with the right credentials answers 200 and the deployed bytes; UpdateBranch(enableBasicAuth=False) re-opens the page with 200.
  Emulator: the edge never looks at the branch's enableBasicAuth/basicAuthCredentials. serve_hosted_site resolves the path against the app's customRules and answers 200 with the file, 405 for a non-GET/HEAD, or 404 — there is no Authorization parsing, no 401 and no challenge header anywhere on that path. The check consequently exhausts its budget polling for the 401 and is reported as "Check timed out after 60.0s"; the phases before it (CreateApp, CreateBranch, manual deploy, the 200 from the branch URL) are exercised identically by amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses, which gets past them.
  Why: basic auth is the only access control on an Amplify branch's site. A client that protects a preview branch this way is told the branch is protected while the emulator serves it to everyone, so a password-protected deployment is in fact public and any negative test of the protection passes vacuously.
  Fix: the hosting edge in this crate's service seam — before serving a file, require and verify the Authorization: Basic header against the branch's stored credentials when enableBasicAuth is set, answering 401 with a `WWW-Authenticate: Basic` challenge otherwise.
  Traces: amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401; services/service-com-amazonaws-amplify-amplify/src/service.rs:241-303 (serve_hosted_site) and :375-403 (override_routing_error, where the edge is dispatched).

Cause 2 of 4: GetBranch reports the basicAuthCredentials value exactly as submitted, so the plaintext password is readable back out of the API.
  AWS: Smithy marks Branch$basicAuthCredentials @sensitive. Recorded behavior only: after UpdateBranch with a base64 user:password value, GetBranch reports a non-empty basicAuthCredentials that is NOT the submitted value, and whose base64 decoding does not contain the submitted password.
  Emulator: stores the request's string unchanged and the Branch projection reports it, so GetBranch returns the submitted value verbatim and base64-decoding it yields user:<plaintext password>.
  Why: anyone with amplify:GetBranch can read the branch's password, which AWS does not disclose; a client that compares the reported value with what it submitted (to detect configuration drift) also gets the opposite answer from AWS's.
  Fix: the branch write handlers' ingestion of basicAuthCredentials plus the Branch projection GetBranch serves — store/report the value AWS reports rather than echoing the request. Not yet observed at replay (the check times out on Cause 1 first), so this cause is grounded in the check's assertions plus services/service-com-amazonaws-amplify-amplify/src/ops/create_branch.rs:279.

Cause 3 of 4: customHeaders given as YAML is stored and reported verbatim, where AWS re-serializes the document and GetApp reports the equivalent JSON.
  AWS: Smithy models App$customHeaders / UpdateApp.customHeaders as CustomHeaders (string, @length 0..25000, pattern ^(?s)) — the model says nothing about the document's format. Recorded behavior: UpdateApp(customHeaders=<the YAML document "customHeaders:\n  - pattern: '**/*'\n    headers:\n      - key: 'X-Sim2real-Probe'\n        value: 'on'\n">) is followed by GetApp reporting customHeaders as the equivalent JSON array — json.loads of it yields [{"pattern": "**/*", "headers": [{"key": "X-Sim2real-Probe", "value": "on"}]}] — i.e. the API re-serializes the YAML into the JSON form of the rules it parsed, dropping the outer `customHeaders:` key.
  Emulator: GetApp returns the submitted YAML text unchanged, so json.loads of it raises json.decoder.JSONDecodeError "Expecting value: line 1 column 1 (char 0)".
  Why: a client reads back customHeaders to inspect or edit the effective rules and parses it as JSON (as AWS's own console flow does); against the emulator every such read fails to parse, and a client cannot distinguish "not configured" from "configured in another format".
  Fix: the UpdateApp/CreateApp handler's ingestion of customHeaders (parse the YAML document into the rule list) and the App projection GetApp serves (emit that list as JSON).
  Traces: amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses.

Cause 4 of 4: the hosting edge applies none of the app's customHeaders to the responses it serves.
  AWS: recorded behavior only: with the customHeaders rule above configured (pattern '**/*'), a GET of a matching path on the branch's default domain comes back carrying `x-sim2real-probe: on` in addition to the file's own content-type (text/css for a .css object); before the rule is configured the header is absent.
  Emulator: serve_hosted_site sets only content-type and content-length on a served file — the owning app's customHeaders are never consulted, so no configured header ever appears and the check's wait for it cannot succeed.
  Why: customHeaders is how an Amplify-hosted site sets CSP, HSTS, cache-control and CORS headers. A client validating its security headers end-to-end sees them silently missing, and anything that depends on a configured header at the edge behaves as if the configuration were absent.
  Fix: the hosting edge in this crate's service seam — resolve the owning app's customHeaders rules against the request path (alongside the existing customRules resolution) and add the matched rules' key/value pairs to the response headers.
  Traces: amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses (the header wait follows Cause 3's assertion in the same check); services/service-com-amazonaws-amplify-amplify/src/service.rs:260-278.

## Evidence
Cause 1 (amplify.basic-auth-branch-rejects-anonymous-and-wrong-credentials-with-401): the check deploys a page, confirms the branch URL serves 200, then UpdateBranch(enableBasicAuth=True, basicAuthCredentials=base64('probe:<password>')) and polls the URL for a 401. Expected: 401 with a `www-authenticate` header starting 'Basic'; 200 + the exact page bytes for Authorization: Basic base64('probe:<password>'); 401 for a wrong password; 200 again after enableBasicAuth=False. Actual: the suite reports "Error: Check timed out after 60.0s" — no 401 ever appears, and the edge code path that answers the request (src/service.rs serve_hosted_site) contains no Authorization handling and no 401 branch at all (its only non-200 answers are 405 for a non-GET/HEAD and 404 for an unpublished path).

Cause 2 (same check, later assertions): GetBranch must report enableBasicAuth True and a basicAuthCredentials that is neither absent, nor '', nor the submitted value, and whose base64 decoding does not contain the submitted password. The emulator stores input.basic_auth_credentials unchanged (create_branch.rs:279) and reports it through the Branch projection.

Cause 3 (amplify.custom-headers-yaml-is-stored-as-json-and-served-on-responses): expected json.loads(GetApp()['app']['customHeaders']) == [{'pattern': '**/*', 'headers': [{'key': 'X-Sim2real-Probe', 'value': 'on'}]}]. Actual: json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0) — the member holds the submitted YAML text.

Cause 4 (same check): expected a GET of <branch-url>styles/main.css to carry header x-sim2real-probe: on (and content-type text/css). Actual: the edge emits only content-type and content-length; no configured custom header is ever served. (The header name/value above are the check's probe values filling the template; the requirement is that a matching rule's key/value is served.)

## Steps to Reproduce
Cause 1 and 2:
1. app = amplify.create_app(name=<unique>)['app']; amplify.create_branch(appId=app['appId'], branchName='main').
2. Manual-deploy a page: dep = amplify.create_deployment(appId, 'main'); PUT a zip containing index.html to dep['zipUploadUrl']; amplify.start_deployment(appId, 'main', jobId=dep['jobId']); poll get_job to SUCCEED.
3. GET https://main.<app defaultDomain>/ -> 200 with the page (holds today).
4. amplify.update_branch(appId, 'main', enableBasicAuth=True, basicAuthCredentials=base64('probe:<password>')).
5. GET the same URL with no Authorization -> expect 401 with `WWW-Authenticate: Basic ...`; observed 200 forever (the check times out).
6. GET it with Authorization: Basic base64('probe:<password>') -> expect 200 and the page; with a wrong password -> expect 401.
7. amplify.get_branch(appId, 'main')['branch'] -> expect enableBasicAuth True and a basicAuthCredentials that is not the submitted string and does not decode to the plaintext password.
8. amplify.update_branch(appId, 'main', enableBasicAuth=False) -> the URL must serve 200 and the page again.

Cause 3 and 4:
1. Create an app and branch 'main' and manual-deploy {'index.html': ..., 'styles/main.css': ...}; confirm the branch URL serves 200 and that x-sim2real-probe is absent.
2. amplify.update_app(appId=appId, customHeaders="customHeaders:\n  - pattern: '**/*'\n    headers:\n      - key: 'X-Sim2real-Probe'\n        value: 'on'\n").
3. json.loads(amplify.get_app(appId=appId)['app']['customHeaders']) -> expect [{'pattern': '**/*', 'headers': [{'key': 'X-Sim2real-Probe', 'value': 'on'}]}]; observed a JSONDecodeError because the YAML is echoed.
4. GET <branch-url>styles/main.css -> expect header x-sim2real-probe: on and content-type text/css; observed no x-sim2real-probe header.
