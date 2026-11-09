Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mgn.account-initialization-gate-rejected mgn.create-connector-invalid-ssm-instance-id-pattern-rejected
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn.account-initialization-gate-rejected|run#8|AssertionError|assert code == "UninitializedAccountException", (
Issue-Key: mgn.create-connector-invalid-ssm-instance-id-pattern-rejected|run#15|AssertionError|assert good_code == "UninitializedAccountException", (
Issue-Commit: c2fc8676d63317349fbaa2ea39a789b7f43b5131

# mgn CreateWave and CreateConnector answer 501 NotImplemented instead of the modeled UninitializedAccountException account gate (2 causes)

> _Note: this issue was identified against the code base at commit `c2fc8676d63317349fbaa2ea39a789b7f43b5131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWave

## Description
Two net-new root causes in the mgn (ApplicationMigrationService) crate, each in its own operation handler and each to be closed separately. Both are operations that AWS gates behind the one-time account-level InitializeService call: on an account that is not initialized they must be refused with the modeled UninitializedAccountException, but the emulator answers HTTP 501 NotImplemented from a scaffolded stub. NOTE: the open issue 20261023T003710Z-mgn-tag-api-and-createlaunchconfigurationtemplate-answer-501-notimplemented.md covers the Tag* family, CreateLaunchConfigurationTemplate and the InitializeService/gate state in UpdateApplication+ListApplications; it does NOT cover CreateWave or CreateConnector, which are separate handler files with their own replay keys. Interestingly CreateApplication in the same suite DOES already answer UninitializedAccountException/'Account not initialized' correctly, which is the baseline these two operations must match.

Cause 1 of 2: CreateWave answers 501 NotImplemented instead of UninitializedAccountException on an uninitialized account.
AWS: Smithy (com.amazonaws.mgn#CreateWave) binds POST /CreateWave, success 201, and models exactly three errors: ConflictException (httpError 409), ServiceQuotaExceededException (402) and UninitializedAccountException (httpError 400). The account-initialization precondition is therefore part of CreateWave's contract. Recorded behavior for the exact wire message: on an account whose execution role cannot create the service-linked role (so InitializeService can never complete), CreateWave is refused with HTTP 400, error code UninitializedAccountException and message 'Account not initialized'. That message is entirely fixed text for this error class — it carries no request- or state-derived values — so it must be reproduced verbatim for every gated operation.
Emulator: HTTP 501, error code NotImplemented. Replay detail: "AssertionError: CreateWave on an uninitialized account must raise UninitializedAccountException, got 'NotImplemented'". The handler is a scaffolded stub that still carries the SCAFFOLDED marker and returns crate::scaffolded::Scaffolded::not_implemented("CreateWave").
Why: 501 NotImplemented is not a modeled error, so the SDK cannot map it onto UninitializedAccountException. A client whose bootstrap path is 'create wave -> on UninitializedAccountException call InitializeService and retry' never takes that branch; it sees a transport-level 'operation not implemented' failure instead of the service's own precondition, and cannot tell an uninitialized account from an outage. It also hides the ordering AWS guarantees: the account gate is evaluated ahead of any per-resource check.
Fix: the CreateWave handler and the mgn account-initialization state it must consult — refuse with UninitializedAccountException (400, 'Account not initialized') while the account is uninitialized, otherwise mint a wave-[0-9a-zA-Z]{17} id and answer 201 with the modeled Wave output; drop the SCAFFOLDED marker.
Traces: mgn.account-initialization-gate-rejected (the CreateApplication assertion ahead of it passed; the DescribeSourceServers and DeleteLaunchConfigurationTemplate assertions after it were never reached because the check aborted here, so those two operations' gate behavior is still unobserved and must be re-replayed with the fix).

Cause 2 of 2: CreateConnector validates the ssmInstanceID pattern but then answers 501 NotImplemented instead of falling through to the account gate.
AWS: Smithy (com.amazonaws.mgn#CreateConnector) binds POST /CreateConnector, success 201, with ssmInstanceID (SsmInstanceID, required, @length 19..20, @pattern (^i-[0-9a-zA-Z]{17}$)|(^mi-[0-9a-zA-Z]{17}$)), and models UninitializedAccountException (httpError 400) and ValidationException (400). Recorded behavior: a malformed ssmInstanceID is refused at field level ahead of the account gate (HTTP 400, code BadRequestException, message 'Invalid request body' — fixed text for that class), while a structurally VALID but nonexistent id (e.g. mi- followed by 17 characters) clears field validation and is refused one step later by the account gate: HTTP 400, code UninitializedAccountException, message 'Account not initialized'. The two inputs must surface two DIFFERENT codes. ListConnectors models UninitializedAccountException as well and must honor the same gate.
Emulator: the malformed-value path is already correct (BadRequestException / 'Invalid request body', refused before the handler by schema validation), but the well-formed path reaches the scaffolded handler and answers error code NotImplemented. Replay detail: "AssertionError: a well-formed but nonexistent ssmInstanceID must clear field validation and fail one step later on the account-initialization gate, got 'NotImplemented' (connectorID=None)". The handler still carries the SCAFFOLDED marker and returns crate::scaffolded::Scaffolded::not_implemented("CreateConnector").
Why: a client cannot distinguish 'the id I passed is malformed' from 'this account has not been initialized' — the second case collapses onto an unmodeled 501. Pattern-boundary tests pass for the wrong reason, and connector creation is unreachable with no modeled explanation. The ListConnectors assertions in the same check were never reached, so the read side of the connector gate is unverified.
Fix: the CreateConnector handler (and the same gate check in the ListConnectors handler) over the mgn account-initialization and connector state: after the modeled field constraints, refuse with UninitializedAccountException (400, 'Account not initialized') while the account is uninitialized, otherwise mint a connector-[0-9a-zA-Z]{17} id and answer 201 with the modeled Connector output; drop the SCAFFOLDED marker.
Traces: mgn.create-connector-invalid-ssm-instance-id-pattern-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service mgn: 4/20 passed (16 failed). Both failures below are net-new: neither is marked ALREADY FILED nor OWNED in the replay report.

[mismatch] mgn.account-initialization-gate-rejected
  AssertionError: CreateWave on an uninitialized account must raise UninitializedAccountException, got 'NotImplemented'
  Expected (model + recording): HTTP 400, error code UninitializedAccountException, message 'Account not initialized' (fixed text, no per-run values).
  Actual (emulator):            HTTP 501, error code NotImplemented.
  Baseline in the SAME check that passed: ListNetworkMigrationDefinitions returned a normal items list (account/credentials healthy), and CreateApplication was correctly refused with UninitializedAccountException / 'Account not initialized' — so the gate exists in the emulator for CreateApplication and is simply absent from CreateWave.
  Not reached (aborted here): DescribeSourceServers and DeleteLaunchConfigurationTemplate gate assertions.
  Stub: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/create_wave.rs:171 — Err(crate::scaffolded::Scaffolded::not_implemented("CreateWave")), SCAFFOLDED marker at line 4.

[mismatch] mgn.create-connector-invalid-ssm-instance-id-pattern-rejected
  AssertionError: a well-formed but nonexistent ssmInstanceID must clear field validation and fail one step later on the account-initialization gate, got 'NotImplemented' (connectorID=None)
  Request A (malformed): CreateConnector(name=..., ssmInstanceID='not-a-valid-ssm-instance-id') -> emulator already answers BadRequestException / 'Invalid request body' (assertions passed).
  Request B (well-formed, nonexistent): CreateConnector(name=..., ssmInstanceID='mi-00000000000000000', tags={...})
  Expected (model + recording): HTTP 400, error code UninitializedAccountException, message 'Account not initialized'; must differ from the malformed-input code.
  Actual (emulator):            error code NotImplemented (HTTP 501), connectorID None.
  Not reached (aborted here): the ListConnectors gate assertions (UninitializedAccountException / 'Account not initialized').
  Stub: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/create_connector.rs:165 — Err(crate::scaffolded::Scaffolded::not_implemented("CreateConnector")), SCAFFOLDED marker at line 4.

Model support (services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model/com-amazonaws-mgn-applicationmigrationservice.json):
  CreateWave: POST /CreateWave, success 201; errors ConflictException(409), ServiceQuotaExceededException(402), UninitializedAccountException(400).
  CreateConnector: POST /CreateConnector, success 201; ssmInstanceID required, @length 19..20, @pattern (^i-[0-9a-zA-Z]{17}$)|(^mi-[0-9a-zA-Z]{17}$); errors UninitializedAccountException(400), ValidationException(400).
  ListConnectors: POST /ListConnectors, success 200; errors UninitializedAccountException(400), ValidationException(400).
  UninitializedAccountException: com.amazonaws.mgn#UninitializedAccountException, client error, httpError 400, members message/code.

Related open issue (does NOT cover these two operations): 20261023T003710Z-mgn-tag-api-and-createlaunchconfigurationtemplate-answer-501-notimplemented.md (Tag* family, CreateLaunchConfigurationTemplate, InitializeService/gate state for UpdateApplication+ListApplications).

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (branch mainline) and boot the emulator built from that checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1, or just the two checks below.
3. mgn.account-initialization-gate-rejected: on an account that has not completed InitializeService, call ListNetworkMigrationDefinitions (succeeds, positive control), then CreateApplication (correctly refused UninitializedAccountException / 'Account not initialized'), then CreateWave(name=<any valid wave name>). The emulator answers HTTP 501 NotImplemented; AWS answers HTTP 400 UninitializedAccountException with message 'Account not initialized'. After the fix the check continues into DescribeSourceServers and DeleteLaunchConfigurationTemplate(launchConfigurationTemplateID='lct-00000000000000000'), which must BOTH also answer UninitializedAccountException / 'Account not initialized' (the account gate ahead of any not-found check).
4. mgn.create-connector-invalid-ssm-instance-id-pattern-rejected: call CreateConnector with ssmInstanceID='not-a-valid-ssm-instance-id' (already refused BadRequestException / 'Invalid request body'), then call CreateConnector with a well-formed but nonexistent ssmInstanceID such as 'mi-' + 17 characters. The emulator answers NotImplemented; AWS answers HTTP 400 UninitializedAccountException / 'Account not initialized', i.e. a DIFFERENT code from the malformed input. After the fix the check continues into ListConnectors, which must also answer UninitializedAccountException / 'Account not initialized'.
5. Re-replay mgn.account-initialization-gate-rejected and mgn.create-connector-invalid-ssm-instance-id-pattern-rejected after the fix; BOTH must pass (and both run further assertions than they reach today) before closing.
