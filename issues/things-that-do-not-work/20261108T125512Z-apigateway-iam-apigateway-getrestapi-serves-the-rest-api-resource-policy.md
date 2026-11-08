Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__iam.xsvc-apigateway-3-iam-resource-policy-allow-rescues-unauthorized-role
Issue-Key: apigateway__iam.xsvc-apigateway-3-iam-resource-policy-allow-rescues-unauthorized-role|run#11|JSONDecodeError|stored = json.loads(json.loads('"' + ctx.client("apigateway").get_rest_api(restApiId=api["api_id"])["policy"] + '"'))
Issue-Commit: f4bfe841f53882612751e062d2ff632442747d6f

# apigateway: GetRestApi serves the REST API resource policy as a raw JSON document instead of AWS's escaped stringified policy

> _Note: this issue was identified against the code base at commit `f4bfe841f53882612751e062d2ff632442747d6f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRestApi

## Description
One net-new root cause, grounded in 1 failing behavioral check listed under the suite heading `### apigateway__iam`. The failure is marked neither ALREADY FILED nor OWNED in the replay report.

Cause 1 of 1: the `policy` member of the RestApi body is returned as a plain, unescaped JSON object, not as AWS's STRINGIFIED policy document. Failing check: apigateway__iam.xsvc-apigateway-3-iam-resource-policy-allow-rescues-unauthorized-role.

AWS: Smithy com.amazonaws.apigateway#RestApi$policy (model services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json), documented as "A stringified JSON policy document that applies to this RestApi regardless of the caller and Method configuration", and bound to GetRestApi's output (as well as CreateRestApi/UpdateRestApi). The model constrains the encoding no further, so the rest is RECORDED BEHAVIOR: API Gateway stores the resource policy as a STRING whose own inner double quotes are backslash-escaped, so the value carried in the JSON response field, after normal JSON decoding, is still an escaped document (characters `{\"Version\":\"2012-10-17\" ...`). A client therefore unescapes once and then parses — exactly what the recording does: `json.loads(json.loads('"' + get_rest_api(...)['policy'] + '"'))`. Only the ESCAPING is the requirement; the statements, the api id, the stage and the account in the document are values taken from the request and from the emulator's state.

Emulator: after UpdateRestApi with patchOperations [{op: replace, path: /policy, value: <json.dumps(doc)>}], GetRestApi returns `policy` holding the document with RAW, unescaped double quotes. The recorded client's unescape step then fails: wrapping the value in quotes yields `"{"Version": ...`, whose first JSON value is the 1-character string `{`, and decoding raises `json.decoder.JSONDecodeError: Extra data: line 1 column 4 (char 3)`. The handler path is a verbatim pass-through of the stored value: GetRestApi builds its body from `CreateRestApiOutput::from(entry)` and copies `policy: body.policy` (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_rest_api.rs lines 111-138, 206-221), and nothing in that path re-encodes what UpdateRestApi's /policy patch stored.

Why: every client that reads an API's resource policy back — the AWS CLI/SDK round trip, Terraform/CloudFormation drift detection, and any tool that diffs or edits the policy before writing it again — unescapes the field once before parsing, because that is what AWS returns. Against this emulator that unescape either throws (as here) or silently mangles the document, so reading a resource policy back is impossible and no tool can verify what it just wrote. It also hides the next assertion in the same flow (that the stored policy has the `execute-api:/<stage>/<METHOD>/<path>` shorthand expanded into the full `arn:<partition>:execute-api:<region>:<account>:<api-id>/<stage>/<METHOD>/<path>` resource), which was never reached and is NOT claimed here as a second defect.

Fix: the RestApi body serializer for the `policy` member — the shared `CreateRestApiOutput::from(&RestApiEntry)` conversion that GetRestApi reuses, together with whatever UpdateRestApi's `/policy` patch and CreateRestApi's `policy` input write into the stored entry — must render the stored resource policy as AWS's stringified document (inner quotes escaped) on every operation that returns the RestApi shape, so that one unescape followed by one parse yields the policy object.

Traces: apigateway__iam.xsvc-apigateway-3-iam-resource-policy-allow-rescues-unauthorized-role (the JSONDecodeError is raised reading back the policy right after UpdateRestApi + CreateDeployment).

## Evidence
Replay report, service heading `### apigateway__iam`:

  [error] apigateway__iam.xsvc-apigateway-3-iam-resource-policy-allow-rescues-unauthorized-role
    File "__init__.py", line 346, in loads
      return _default_decoder.decode(s)
    File "decoder.py", line 341, in decode
      raise JSONDecodeError("Extra data", s, end)
    json.decoder.JSONDecodeError: Extra data: line 1 column 4 (char 3)

The raising line in the check is:

  stored = json.loads(json.loads('"' + ctx.client("apigateway").get_rest_api(restApiId=api["api_id"])["policy"] + '"'))

Expected (AWS, recorded): `policy` is a stringified policy document — its inner double quotes arrive backslash-escaped, so `json.loads('"' + policy + '"')` yields the document text and the second `json.loads` yields the policy object; the next assertion can then inspect Statement[0].Resource.

Actual (emulator): `policy` arrives as a raw JSON object text beginning `{"`. `json.loads('"' + policy + '"')` parses the 3-character prefix `"{"` as the string `{` and reports extra data at char 3 — the exact column in the failure, which pins the diverging value to the first three characters of the field and not to any per-run value (api id, account, region, stage are all variable parts here and none of them is at issue).

Smithy citation: RestApi$policy — "A stringified JSON policy document that applies to this RestApi regardless of the caller and Method configuration" (GetRestApi output member `policy`, String).

Emulator code path: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_rest_api.rs:111-138 (`impl From<&RestApiEntry> for GetRestApiOutput` copies `policy: body.policy`) and :206-221 (handler returns `GetRestApiOutput::from(entry)` with no re-encoding).

## Steps to Reproduce
1. git_clone, then run_behavioral_suite against the emulator.
2. apigateway: CreateRestApi; GetResources to find the root id; CreateResource pathPart=widgets; PutMethod GET with authorizationType=AWS_IAM; PutMethodResponse 200; PutIntegration type=MOCK; PutIntegrationResponse 200; CreateDeployment stageName=test.
3. UpdateRestApi with patchOperations=[{"op":"replace","path":"/policy","value": <a json.dumps'ed policy document allowing execute-api:Invoke on "execute-api:/test/GET/widgets">}], then CreateDeployment stageName=test.
4. Call GetRestApi and read the `policy` field. AWS returns a stringified document whose inner quotes are escaped, so `json.loads(json.loads('"' + policy + '"'))` yields the policy object; the emulator returns the raw document and that unescape raises JSONDecodeError: Extra data: line 1 column 4 (char 3).
5. Re-replay apigateway__iam.xsvc-apigateway-3-iam-resource-policy-allow-rescues-unauthorized-role.
