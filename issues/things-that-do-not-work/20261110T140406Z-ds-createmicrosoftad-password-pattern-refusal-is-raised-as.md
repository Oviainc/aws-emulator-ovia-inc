Issue-Source: trace-sweep-divergence
Issue-Trace: directory-service-create-microsoft-ad-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: directory-service-create-microsoft-ad-validation-errors ds-madl-traj-create-microsoft-ad-request-mistakes
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds|CreateMicrosoftAD|4xx->4xx|$.__type
Issue-Related: 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md
Issue-Commit: d082b564c28c5020cfd395c3fa9d8077cc4f120e

# ds: CreateMicrosoftAD password-pattern refusal is raised as InvalidParameterException with no message instead of AWS's ValidationException body

> _Note: this issue was identified against the code base at commit `d082b564c28c5020cfd395c3fa9d8077cc4f120e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD

## Description
One net-new wire divergence in the Directory Service (ds) control plane, found by replaying the recorded corpus against the emulator.

Cause 1 of 1: CreateMicrosoftAD refuses a Password that violates the modeled pattern, but it classifies the refusal as a modeled service error and emits no constraint-violation message, where AWS answers the framework-level ValidationException whose body names the member and the pattern.

AWS: Smithy (services/service-com-amazonaws-directoryservice-directoryservice-20150416): CreateMicrosoftAD.Password targets Password, is @required and @sensitive and carries @pattern "^(?=^.{8,64}$)((?=.*\\d)(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[^A-Za-z0-9\\s])(?=.*[a-z])|(?=.*[^A-Za-z0-9\\s])(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[A-Z])(?=.*[^A-Za-z0-9\\s]))^". The operation's modeled error list is ClientException, DirectoryLimitExceededException, InvalidParameterException, ServiceException, UnsupportedOperationException (all client-kind 400 except ServiceException) — it does NOT model ValidationException, so the constraint refusal is the protocol-level one the front door raises before the handler runs (recorded behavior for this framework-level class). Recorded response: HTTP 400, $.__type "ValidationException", and the error body carries a lowercase $.message member. Message template — fixed text: "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; variable parts: the member path taken from the request (here 'password') and the regular expression taken from the model's @pattern for that member. Because Password is @sensitive the offending value is NOT echoed: the clause is "Value at 'password'", never "Value '<secret>' at 'password'". No recording-specific value (directory name, password text, request id) is part of the requirement.

Emulator: answers HTTP 400 — the status now matches — but with $.__type "InvalidParameterException" (a modeled ds service error) and with NO $.message member at all (replay: "$.__type: value_mismatch (expected 'ValidationException', got 'InvalidParameterException')" and "$.message: missing (expected \"1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: (?=^.{8,64}$)...^.*\", got None)"). So the constraint violation is being reported by the operation's own handler as a service-level invalid-parameter error rather than by the shared request-constraint validator, and the validator's message body is never built.

Why: a client relying on AWS's behavior breaks two ways. (1) It branches on the error class: AWS's ValidationException is the SDK's protocol/constraint error and is handled by generic input-validation code, while InvalidParameterException is a modeled Directory Service exception the SDK surfaces as a service fault — retry/classification and typed catch blocks therefore take the wrong path, and SDKs with modeled-error unions may fail to map the response at all. (2) It reads the diagnostic: AWS always states which member failed and what pattern it had to satisfy, so a caller can tell the user that the admin password is too weak; the emulator sends an error with no message member, so the caller has nothing to report and cannot distinguish a bad password from any other invalid parameter.

Fix: CreateMicrosoftAD's request validation — the @pattern check on Password must be performed by the shared request-constraint validator that runs before the handler and that emits the "N validation error(s) detected: ..." ValidationException body, instead of being re-implemented in the handler as an InvalidParameterException. The @sensitive suppression of the echoed value belongs in that same shared constraint-violation serializer.

Traces: directory-service-create-microsoft-ad-validation-errors#task#1 (representative), ds-madl-traj-create-microsoft-ad-request-mistakes#task#2 (same cluster).

Related open issues that do NOT cover this step: 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md is keyed on the earlier shape of this failure (ds|CreateMicrosoftAD|4xx->2xx|...: the password was accepted and answered HTTP 200) plus the 4xx->4xx|$.message subnet-aggregation case; it carries no replay key for the 400->400 $.__type divergence reported here, and the replay marks this cluster unowned. 20261103T100504Z-ds-directory-service-certificate-and-directory-lookups-unvalidated-shortname.md and 20261109T072650Z-ds-validationexception-message-omits-regex-anchors-* cover other ds causes (shortname validation, missing ^...$ anchors in the pattern text) and not the error class or the omitted message member for the password constraint.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 101 steps, 65 passed, 36 mismatched, 20 divergence clusters, score 64.4%). This is the only cluster the replay reports with no owning issue and no blocking stub.

Cluster: service "ds", operation CreateMicrosoftAD, divergence_key "service=ds|CreateMicrosoftAD|status=400->400|keys=$.__type,$.message", verdict mismatch, expected_status 400, actual_status 400, affected_count 2.
Representative step: fixture directory-service-create-microsoft-ad-validation-errors, step task#1.
Other member of the cluster: ds-madl-traj-create-microsoft-ad-request-mistakes#task#2.

Expected (recorded AWS response):
  status 400
  $.__type  = "ValidationException"
  $.message = "1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: (?=^.{8,64}$)((?=.*\\d)(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[^A-Za-z0-9\\s])(?=.*[a-z])|(?=.*[^A-Za-z0-9\\s])(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[A-Z])(?=.*[^A-Za-z0-9\\s]))^.*"

Actual (emulator):
  status 400
  $.__type  = "InvalidParameterException"
  $.message = absent (None)

Message anatomy: fixed text "1 validation error detected: Value at '" ... "' failed to satisfy constraint: Member must satisfy regular expression pattern: " ... ; variable parts are the member path ('password'), taken from the request, and the pattern string, taken from the model. The offending password value is deliberately absent because the member is @sensitive. Nothing in this message is a per-run value.

Model evidence (smithy_query operation=CreateMicrosoftAD on services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): Password -> {required: true, sensitive: true, pattern: "^(?=^.{8,64}$)((?=.*\\d)(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[^A-Za-z0-9\\s])(?=.*[a-z])|(?=.*[^A-Za-z0-9\\s])(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[A-Z])(?=.*[^A-Za-z0-9\\s]))^"}; errors = ClientException, DirectoryLimitExceededException, InvalidParameterException, ServiceException, UnsupportedOperationException — ValidationException is not among them, confirming it is the front-door constraint error rather than a modeled operation error.

## Steps to Reproduce
1. Clone this repository and boot the emulator built from the checkout.
2. Replay wire trace set simparity-wire-aws-v1.
3. Fixture directory-service-create-microsoft-ad-validation-errors, step task#1: POST a DirectoryService_20150416.CreateMicrosoftAD request whose Password violates the modeled @pattern (for example a short all-lowercase string) with an otherwise well-formed Name and VpcSettings.
4. Observe the response: HTTP 400 with body {"__type": "InvalidParameterException"} and no "message" member.
5. Expected: HTTP 400 with "__type": "ValidationException" and "message": "1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: <the Password @pattern from the model>" — the member path taken from the request and the pattern from the model, with the offending value suppressed because Password is @sensitive.
6. The same divergence reproduces at fixture ds-madl-traj-create-microsoft-ad-request-mistakes, step task#2.
