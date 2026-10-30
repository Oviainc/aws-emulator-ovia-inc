Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.delete-builtin-browser-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetBrowser|ResourceNotFoundException
Issue-Commit: 11c4079474be1ad931c1a1cc92224a1a5bcf318e

# bedrock-agentcore-control: the service-provided built-in browser aws.browser.v1 is absent, so GetBrowser answers 404 instead of a READY built-in

> _Note: this issue was identified against the code base at commit `11c4079474be1ad931c1a1cc92224a1a5bcf318e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBrowser

## Description
Cause 1 of 1: the AgentCore control-plane state never contains the service-provided (built-in) browser tool `aws.browser.v1`, so every read addressed to it is refused as a missing resource. Failing check: bedrock-agentcore-control.delete-builtin-browser-rejected.

AWS: `aws.browser.v1` is the service-provided browser tool and it exists in an account/region without anybody creating it. The model admits it as a first-class identifier rather than as a customer value: `GetBrowser.browserId` / `GetBrowserResponse.browserId` target `BrowserId` with pattern `^(aws\.browser\.v1|[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10})$`, and `BrowserArn` is `^arn:aws(-[^:]+)?:bedrock-agentcore:[a-z0-9-]+:(aws|[0-9]{12}):browser(-custom)?/(aws\.browser\.v1|[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10})$` — i.e. the bare `browser/` resource type under the `aws` service account is reserved for exactly this built-in, while a customer-created browser is `browser-custom/<name>-<suffix>` under a 12-digit account (Smithy: GetBrowser input/output, shapes BrowserId and BrowserArn; GetBrowser models ResourceNotFoundException only for an identifier that names nothing). Recorded behavior: `GetBrowser(browserId="aws.browser.v1")` succeeds with HTTP 200 and `status` = `READY` both before and after a rejected delete, while `DeleteBrowser(browserId="aws.browser.v1")` is refused — the built-in is readable but not deletable, and its refusal is 404 `ResourceNotFoundException` with message `Browser with ID aws.browser.v1 does not exist` (fixed text `Browser with ID ` + the `browserId` from the request + ` does not exist`).

Emulator: the first call of the check, `GetBrowser(browserId="aws.browser.v1")`, is refused with HTTP 404 `ResourceNotFoundException` / `Browser with ID aws.browser.v1 does not exist`, because the handler only looks the id up in the store of browsers a `CreateBrowser` minted and that store starts empty. The built-in is therefore invisible to every read, and the check cannot even reach its assertions about the rejected delete.

Why: a client that uses the built-in browser tool — the normal way to drive AgentCore's browser without provisioning one (resolve `aws.browser.v1`, check it is `READY`, then start a session against it) — gets a 404 on a resource AWS always has, so it concludes the account has no browser tool and either aborts or tries to create one. Code that distinguishes the built-in from custom browsers (by ARN resource type `browser` vs `browser-custom`, or by refusing to delete the built-in) can never exercise that path here, and the asymmetry AWS exposes (readable, not deletable) is lost: here the built-in is uniformly absent.

Fix: the service's state, not one handler — the browser store must contain the service-provided record from the moment an account's state exists, exactly as `TokenVaults` already seeds its service-managed default vault, so `GetBrowser` (and any browser listing) answers from it and only `DeleteBrowser` keeps refusing it. The seeded record should report `browserId` `aws.browser.v1`, status `READY`, an ARN built with the bare `browser` resource type and the `aws` service account (not `BROWSER_ARN_RESOURCE` = `browser-custom`), and it must not occupy the custom-browser name-uniqueness index or be removable by `DeleteBrowser`.

Note: the open issue 20261029T212239Z-bedrock-agentcore-control-getbrowser-stub.md carries this same check id, but its cause (GetBrowser answering 501 `NotImplemented` as a scaffold stub) no longer reproduces — the handler is implemented and now answers a modeled 404. This is a distinct root cause (missing seeded state) and that issue does not cover it.

Traces: bedrock-agentcore-control.delete-builtin-browser-rejected. Code citations: services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/get_browser.rs:188 (lookup in `store.browsers`), src/state.rs:1919 (`pub browsers: BTreeMap<String, BrowserRecord>` documented as "every custom browser", derived-Default empty), src/state.rs:1452 (`browser_not_found`), src/state.rs:1458-1463 (`BROWSER_ARN_RESOURCE = "browser-custom"`, with the comment that the bare spelling is the service-provided `aws.browser.v1` "which no operation mints"), src/state.rs:1304-1310 (the seeding precedent for a service-managed resource).

## Evidence
Check bedrock-agentcore-control.delete-builtin-browser-rejected [error].

Expected (AWS / recorded): `GetBrowser(browserId="aws.browser.v1")` -> HTTP 200, body with required members `browserId=aws.browser.v1`, `browserArn` of the form `arn:aws:bedrock-agentcore:<region>:aws:browser/aws.browser.v1`, `name`, `networkConfiguration`, `createdAt`, `lastUpdatedAt` and `status=READY`; the same read after a rejected `DeleteBrowser` still reports `READY`; `DeleteBrowser(browserId="aws.browser.v1")` -> HTTP 404 `ResourceNotFoundException`, message `Browser with ID aws.browser.v1 does not exist`.

Actual (emulator): the first `GetBrowser` call already fails —
```
botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetBrowser operation: Browser with ID aws.browser.v1 does not exist
```
so the built-in browser does not exist in the emulator's state at all. Message anatomy: `Browser with ID ` and ` does not exist` are fixed text, `aws.browser.v1` is the `browserId` from the request — correct wording for an id that names nothing, wrong outcome for this id, which AWS always resolves.

Model evidence (smithy_query operation=GetBrowser,DeleteBrowser): `BrowserId` pattern `^(aws\.browser\.v1|[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10})$`; `BrowserArn` pattern allows `:(aws|[0-9]{12}):browser(-custom)?/(aws\.browser\.v1|...)`; GetBrowser http GET /browsers/{browserId}, success 200; DeleteBrowser http DELETE /browsers/{browserId}, success 202, errors include ResourceNotFoundException (404) and ConflictException (409).

## Steps to Reproduce
1. Boot the emulator (bedrock-agentcore-control served).
2. Call GetBrowser with browserId="aws.browser.v1" against a fresh account state, having created no browser.
3. Observe HTTP 404 ResourceNotFoundException "Browser with ID aws.browser.v1 does not exist"; AWS answers HTTP 200 with status READY for this service-provided browser.
4. Call DeleteBrowser with browserId="aws.browser.v1" and then GetBrowser again: AWS refuses the delete with that same 404 message and the built-in is still READY afterwards; here there is nothing to refuse because the built-in was never present.
5. Replay bedrock-agentcore-control.delete-builtin-browser-rejected to confirm.
