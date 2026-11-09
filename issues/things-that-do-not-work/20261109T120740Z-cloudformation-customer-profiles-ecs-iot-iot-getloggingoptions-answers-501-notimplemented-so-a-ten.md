Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__customer-profiles__ecs__iot__kafka__kinesis__neptune__rds__s3tables.cdk-streaming-iot-estate-serves-and-destroys
Issue-Key: cloudformation__customer-profiles__ecs__iot__kafka__kinesis__neptune__rds__s3tables|GetLoggingOptions|NotImplemented
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# iot GetLoggingOptions answers 501 NotImplemented, so a ten-stack CDK estate cannot record or restore the account's v1 logging options and the whole deploy check aborts

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLoggingOptions

## Description
Cause 1 of 1: iot GetLoggingOptions is an unimplemented scaffold stub and refuses with a non-AWS error class.
AWS: Smithy (service-com-amazonaws-iot-awsiotservice): GetLoggingOptions — GET /loggingOptions, empty input, output members roleArn : AwsArn and logLevel : LogLevel (enum); modeled errors InternalException (500), InvalidRequestException (400), ServiceUnavailableException (503). NotImplemented is not in that set. For an account that has configured v1 logging AWS answers 200 with roleArn and logLevel; for an account that never set v1 logging options the recording shows the refusal AWS uses — error code InvalidRequestException with the fixed message "No Logging Options found" (no variable part), which the caller treats as "unset".
Emulator: the operation answers 501 with code NotImplemented and the message "operation GetLoggingOptions is scaffolded but not implemented in this emulator" — an error class the model does not declare for this operation and a message of the emulator's own authorship. The caller cannot distinguish "unset" from "broken", and because the estate check snapshots the account's IoT logging settings before deploying anything, the refusal aborts the run before the CDK deploy, leaving the other assertions of that check (profiles, seeds, shadow, bus, container instance, stream, tables, topic, graph and the four negative classes) unexercised.
Why: any client that reads account-singleton IoT logging state — to back it up, to assert it, or to decide whether to call SetLoggingOptions — gets a 501 instead of either the configuration or the modeled InvalidRequestException "No Logging Options found". Code that catches only the modeled errors propagates the 501 as a hard failure, as this check does.
Fix: implement the iot GetLoggingOptions handler in the iot service crate over the account-level v1 logging state written by SetLoggingOptions: answer 200 {roleArn, logLevel} when set, and refuse 400 InvalidRequestException "No Logging Options found" when the account has never set it. No open issue in the tracker covers iot GetLoggingOptions (the related fixed issue 20261106T201842Z-iot-getv2loggingoptions-stub.md covers only the v2 read), and the report did not mark this failure as already filed.
Traces: cloudformation__customer-profiles__ecs__iot__kafka__kinesis__neptune__rds__s3tables.cdk-streaming-iot-estate-serves-and-destroys.

## Evidence
Expected: iot GetLoggingOptions -> 200 {"roleArn": "<arn>", "logLevel": "<LogLevel>"} when v1 logging is configured, otherwise 400 InvalidRequestException with message "No Logging Options found" (fixed text, no request- or state-derived parts). Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetLoggingOptions operation: operation GetLoggingOptions is scaffolded but not implemented in this emulator — HTTP 501, error code NotImplemented, emulator-authored message.

## Steps to Reproduce
1. Boot the emulator. 2. Call iot GetLoggingOptions on a fresh account (no SetLoggingOptions beforehand). 3. Observe 501 NotImplemented instead of 400 InvalidRequestException "No Logging Options found". 4. Replay cloudformation__customer-profiles__ecs__iot__kafka__kinesis__neptune__rds__s3tables.cdk-streaming-iot-estate-serves-and-destroys — it aborts in its pre-deploy snapshot of IoT settings.
