Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle
Issue-Key: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# apigateway__dynamodb__iam__lambda: an execute-api IAM implicit-deny message omits AWS's 'because no identity-based policy allows the execute-api:Invoke action' clause

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api, AWS_IAM authorization)

## Description
Cause 1 of 1: when a SigV4-signed request reaches an AWS_IAM-authorized method and the caller's identity holds no policy allowing execute-api:Invoke on that method ARN, the emulator refuses with the right status and error type but truncates AWS's message, dropping the clause that names WHY the call was denied.

AWS: recorded behavior only for the execute-api data plane (it is not an operation in the apigateway Smithy model). AWS's refusal for an implicit deny is HTTP 403 with header x-amzn-errortype: AccessDeniedException and a JSON body whose Message is, as a template:
  "User: <caller-arn> is not authorized to perform: execute-api:Invoke on resource: <method-arn> because no identity-based policy allows the execute-api:Invoke action"
Fixed text, exactly as AWS words it: "User: ", " is not authorized to perform: ", " on resource: ", and the trailing " because no identity-based policy allows the execute-api:Invoke action". Variable parts taken from the request and state: <caller-arn> is the signing principal's ARN (for an assumed role, arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>) and <method-arn> is arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/<http-method><resource-path>. The action name inside the trailing clause is the same action named earlier in the message.

Emulator: the status (403) and x-amzn-errortype (AccessDeniedException) are right, and the "User: ... is not authorized to perform: execute-api:Invoke on resource: ..." prefix is right, but the message ENDS at the resource ARN — the trailing " because no identity-based policy allows the execute-api:Invoke action" clause is absent. The recorded body was {"Message":"User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/GET/data-agreements/{agreementId}"} (the account id, role name, session name, api id and stage in that capture are per-run values that fill the template, not part of it).

Why: the reason clause is how a client distinguishes an implicit deny (no policy grants the action — fix by adding a grant) from an explicit deny (a Deny statement matched — adding a grant will not help) and from a resource-policy denial. Diagnostic tooling and runbooks that match on AWS's wording to classify the denial misclassify every execute-api refusal here, and an operator following the emulator's message is told the call was denied but never told that nothing allowed it.

Fix: the authorization-refusal serializer for the execute-api data plane in the apigateway service — the component that formats the AccessDeniedException Message for an AWS_IAM-authorized method must append the implicit-deny reason clause, naming the same action it names earlier.

Traces: apigateway__dynamodb__iam__lambda.e2e-debug-api-gateway-iam-authorization-lifecycle (phase 3/5 'solve').

## Evidence
Expected (AWS): HTTP 403, x-amzn-errortype: AccessDeniedException, body Message ending with
  ":<api-id>/<stage>/GET<resource-path> because no identity-based policy allows the execute-api:Invoke action"

Actual (emulator): HTTP 403 and x-amzn-errortype: AccessDeniedException are correct, but the assertion on the message tail fails:
  AssertionError: [phase 3/5 solve] the denial read '{"Message":"User: arn:aws:sts::000000000000:assumed-role/sim2real-agr-restricted-4af63853da12/sim2real-agr is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:us-west-2:000000000000:8sh3tqaaaa/beta/GET/data-agreements/{agreementId}"}'
The message stops at the method ARN; the trailing reason clause AWS always appends for an implicit deny is missing. (The account, role, session, api id and stage shown are this run's values, not requirements.)

## Steps to Reproduce
1. Create a REST API with a GET method on /data-agreements/{agreementId} whose authorizationType is AWS_IAM, wired to a Lambda proxy integration, and deploy it to a stage.
2. Create an IAM role the caller can assume whose policy grants execute-api:Invoke on a DIFFERENT method ARN only (e.g. the same api/stage but another HTTP method), so nothing allows execute-api:Invoke on the GET method ARN.
3. AssumeRole into that role and send a SigV4-signed GET to https://<api-id>.execute-api.<region>.<host>/<stage>/data-agreements/<id>.
4. Observe HTTP 403 with x-amzn-errortype: AccessDeniedException, then read the JSON body's Message: it ends at the method ARN instead of continuing '... because no identity-based policy allows the execute-api:Invoke action'.
