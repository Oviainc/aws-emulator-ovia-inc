Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__sqs.dead-letter-queue-is-refused-until-the-role-may-send-to-it
Issue-Key: lambda__logs__sqs.dead-letter-queue-is-refused-until-the-role-may-send-to-it|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# CreateFunction accepts a DeadLetterConfig the execution role may not SendMessage to (1 cause)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Cause 1 of 1 — the DeadLetterConfig target is not checked against the execution role, so a function whose DLQ can never be written is created.
AWS: Smithy (com.amazonaws.lambda) CreateFunction with DeadLetterConfig.TargetArn; models InvalidParameterValueException. The model carries no message text, so for the text this is recorded behavior only: Lambda validates the execution role against the named target when DeadLetterConfig is set and refuses CreateFunction with InvalidParameterValueException and the message `The provided execution role does not have permissions to call SendMessage on SQS` — entirely fixed text, no variable part. No function is created (GetFunction then answers ResourceNotFoundException `Function not found: arn:aws:lambda:<region>:<account>:function:<name>`). Once the role is granted sqs:SendMessage on the queue, the identical CreateFunction succeeds and GetFunctionConfiguration reports the DeadLetterConfig.
Emulator: answers success and creates the function, so the refusal never happens.
Why: this is the one point at which Lambda tells a caller its DLQ wiring is unusable. Accepted, the function is created with a dead-letter target it can never write to, so asynchronous invocation failures are silently lost instead of landing in the queue — and the create that should have failed reports success, so IaC records a healthy resource.
Fix: the DeadLetterConfig target authorization check in CreateFunction's (and UpdateFunctionConfiguration's) validation layer in the lambda crate — evaluate the execution role's sqs:SendMessage on the target before the function is written.
Traces: lambda__logs__sqs.dead-letter-queue-is-refused-until-the-role-may-send-to-it

One cause must be closed before submitting.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

dead-letter-queue-is-refused-until-the-role-may-send-to-it:
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code
  raised by the check's _error() helper around create_function(..., DeadLetterConfig={'TargetArn': <dlq arn>}) on a role carrying only AWSLambdaBasicExecutionRole.
  Expected: code 'InvalidParameterValueException', message 'The provided execution role does not have permissions to call SendMessage on SQS', and GetFunction answering ('ResourceNotFoundException', 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>').

## Steps to Reproduce
Boot the emulator from this checkout.

1. aws sqs create-queue --queue-name dlq
2. Create an IAM role trusted by lambda.amazonaws.com with only AWSLambdaBasicExecutionRole attached (no sqs:SendMessage).
3. aws lambda create-function --function-name f --runtime python3.12 --role <role arn> --handler index.handler --zip-file fileb://fn.zip --dead-letter-config TargetArn=<dlq arn>
-> succeeds. AWS answers InvalidParameterValueException 'The provided execution role does not have permissions to call SendMessage on SQS' and creates nothing.
4. Grant the role sqs:SendMessage on the queue and repeat: the create must succeed and get-function-configuration must report the DeadLetterConfig.

Then re-replay: lambda__logs__sqs.dead-letter-queue-is-refused-until-the-role-may-send-to-it
