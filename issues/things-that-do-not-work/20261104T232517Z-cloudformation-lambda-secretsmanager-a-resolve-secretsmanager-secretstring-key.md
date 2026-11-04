Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back
Issue-Key: cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back|run#11|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# cloudformation__lambda__secretsmanager: a {{resolve:secretsmanager:...:SecretString:<key>}} dynamic reference naming a missing JSON key still reaches CREATE_COMPLETE instead of failing the resource and rolling the stack back (1 cause)

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation Secrets Manager dynamic reference whose JSON key does not exist in the secret's SecretString is silently resolved instead of failing the resource, so the stack completes.

AWS: a template property written as `{{resolve:secretsmanager:<secret-id>:SecretString:<json-key>}}` is resolved by CloudFormation while creating the resource that carries it. When `<json-key>` is not a key of the secret's JSON document, CloudFormation cannot resolve the reference: the resource that carries it goes to CREATE_FAILED (Smithy: cloudformation ResourceStatus enum includes CREATE_FAILED) with the ResourceStatusReason reported by DescribeStackEvents, the stack then goes ROLLBACK_IN_PROGRESS and settles in ROLLBACK_COMPLETE (Smithy: cloudformation StackStatus enum includes ROLLBACK_IN_PROGRESS / ROLLBACK_COMPLETE), and the resource that failed is not left behind — no Lambda function named `<stack-name>-<logical-id>-...` survives. The reason text is recorded behavior only: its fixed text is `Could not find a value associated with JSONKey in SecretString`, with no variable part (the offending key is not interpolated into it); the logical id in the event (`Fn` here) is the request's logical resource id and the only request-derived value. A control template that names a key the secret does have (`api_key`) must instead deploy to CREATE_COMPLETE and the function must observe that key's value in its environment.

Emulator: the create of the bad-key stack settles in `CREATE_COMPLETE`. No resource event reaches CREATE_FAILED, no rollback is performed, and the unresolvable reference is accepted — the check fails its first assertion (`a reference to a missing JSON key must roll the stack back; it ended 'CREATE_COMPLETE'`), so the follow-on assertions (exactly one (`Fn`, `CREATE_FAILED`, `Could not find a value associated with JSONKey in SecretString`) failure event, and no leaked Lambda function) are never reached either.

Why: a client relying on AWS's behavior uses the deploy as the gate on its configuration. A template that points at a key that does not exist in the secret is a deployment bug AWS refuses; here it deploys green, and the function is created with an environment variable that holds nothing (or the literal unresolved `{{resolve:...}}` text). Every negative deployment test of this class passes vacuously, and operators get a CREATE_COMPLETE stack whose Lambda fails at runtime instead of a ROLLBACK_COMPLETE they can act on.

Fix: the CloudFormation stack-create path's dynamic-reference resolver for `secretsmanager` references (the `{{resolve:secretsmanager:<id>:SecretString:<key>[:<stage>[:<version>]]}}` form) must report an unresolvable JSON key as a failure of the resource being created rather than substituting an empty/unresolved value: stamp the resource CREATE_FAILED with that reason, drive the stack through ROLLBACK_IN_PROGRESS to ROLLBACK_COMPLETE, and delete the partially created resources (the function) during rollback.

Related already-open issues, neither of which carries this check's replay key: `20261024T190718Z-secretsmanager-cloudformation-managed-secrets-aws-secretsmanager-secret-deploys-but.md` (same resolver reached through secretsmanager-only checks) and `20261103T171935Z-cloudformation-resource-failures-never-surface-stacks-and-resources.md` (resource failures never surfacing at all).

Traces: cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back (mismatch).

## Evidence
Expected (AWS): create_stack of the template whose Lambda `Environment.Variables.API_KEY` is `{{resolve:secretsmanager:<secret-arn>:SecretString:no_such_key}}` settles at StackStatus ROLLBACK_COMPLETE; DescribeStackEvents reports exactly one failure, logical id `Fn`, ResourceStatus CREATE_FAILED, ResourceStatusReason `Could not find a value associated with JSONKey in SecretString`; ListFunctions shows no function whose name begins with `<stack-name>-`.

Actual (emulator): the same create_stack settles at StackStatus `CREATE_COMPLETE`. Assertion from the replay: `AssertionError: a reference to a missing JSON key must roll the stack back; it ended 'CREATE_COMPLETE'` (check_cloudformation__lambda__secretsmanager_cfn_lambda_env_secretsmanager_missing_json_key_rejected_and_rolled_back.py line 110).

## Steps to Reproduce
1. secretsmanager CreateSecret with SecretString = {"username": "app", "api_key": "<value>"}; keep its ARN.
2. cloudformation CreateStack (Capabilities=[CAPABILITY_IAM]) with a template holding an AWS::IAM::Role trusting lambda.amazonaws.com and an AWS::Lambda::Function whose Environment.Variables.API_KEY is `{{resolve:secretsmanager:<secret-arn>:SecretString:no_such_key}}`.
3. Poll DescribeStacks until StackStatus is terminal — AWS: ROLLBACK_COMPLETE; emulator: CREATE_COMPLETE.
4. DescribeStackEvents on the stack and look for (Fn, CREATE_FAILED, 'Could not find a value associated with JSONKey in SecretString') — absent in the emulator.
5. lambda ListFunctions and look for names beginning with the stack name — AWS leaves none after rollback.
6. Re-run the same template with key `api_key`: it must reach CREATE_COMPLETE and the function must report that key's value.
