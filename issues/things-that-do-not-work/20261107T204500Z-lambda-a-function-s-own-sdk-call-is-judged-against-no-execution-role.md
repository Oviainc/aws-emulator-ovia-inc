Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow|_assert_missing_write_policy_denies#24|AssertionError|assert status in {500, 502}, (

# lambda: a function's own AWS SDK call is judged against no execution role, so a handler whose role carries no write policy writes anyway (1 cause)

## Operation
Invoke

## Description
Cause 1 of 1: a call a running function makes with the SDK its runtime ships is signed with the
emulator's own callback credentials and judged against nothing, so it succeeds whatever the
function's execution role allows.

AWS: a function runs AS its execution role. The runtime hands the handler credentials for that
role, and every call the handler makes is authorized against the policies attached to it — a
handler calling `dynamodb:PutItem` with a role that carries no write policy is refused
`AccessDeniedException`, the SDK raises it, and an unhandled raise makes the invocation a function
error (`X-Amz-Function-Error: Unhandled`), which an API Gateway proxy integration in front of it
surfaces as HTTP 502.

Emulator: the guest's SDK call is built, signed with `ctx.callback()`'s credentials and dispatched
in-process (`services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/aws_api.rs`,
`sent`), with no reference to the function's `Role`. The call therefore succeeds, the handler
returns normally, and a client sees a 200 where AWS answers 5xx.

Why: the policies a template attaches to a function's role become invisible — a least-privilege
mistake cannot be observed at all, and a check that asserts a denial sees a success. The crate
already states the one reading of a role's policies for its other gates
(`state::role_actions::withheld_action`, used by `state::execution::log_delivery` for the three
CloudWatch Logs calls and by `state::vpc_attachment_authorization`), so what is missing is that the
same reading is not applied to the calls the function itself makes.

Fix: judge each call the in-process SDK seam makes against the function's execution role before it
is dispatched — the IAM action being `<service prefix>:<operation>` and the resource the one that
call names — and report a refusal to the guest as the owning service's own `AccessDenied` answer, so
the SDK raises it exactly as it raises any other refusal. Note the blast radius: a function whose
role IAM holds no record for must stay permissive, as the existing gate already is, or every suite
that creates a function with a role it never declared would start failing.

## Evidence
AssertionError: a handler whose PutItem is denied cannot answer 200 (assert status in {500, 502}).
The denied-write leg of
`apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow`: the SAM
template's function is given no write policy, its handler puts an item in the table, and the
deployed stage answers with the handler's own 200 instead of the integration failure a denied write
would cause.

## Steps to Reproduce
1. create_stack from a SAM template whose `AWS::Serverless::Function` declares no `Policies` and
   whose handler calls `dynamodb:PutItem` on a table the same stack declares.
2. Invoke the function (directly, or through the deployed stage's invoke URL).
3. Observe HTTP 200 with the handler's own result, and the item present in the table, instead of the
   `AccessDeniedException` the role's policies owe and the function error it causes.
4. Or replay apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow.
