Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-path-comparators-dereference-the-referenced-field stepfunctions.deep-sfn-language-choice-missing-comparator-path-rejected stepfunctions.stop-execution-on-express-unsupported stepfunctions.conc-distributed-map-zero-tolerance-run-fails-and-keeps-completed-items
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-path-comparators-dereference-the-referenced-field|run#4|AssertionError|assert ref_path in cause, (
Issue-Key: stepfunctions.deep-sfn-language-choice-missing-comparator-path-rejected|run#5|AssertionError|assert missing in (bad.get("cause") or ""), (
Issue-Key: stepfunctions.stop-execution-on-express-unsupported|run#18|AssertionError|assert describe_error.get("Code") == "InvalidArn", (
Issue-Key: stepfunctions.conc-distributed-map-zero-tolerance-run-fails-and-keeps-completed-items|_wait_terminal#13|Timeout|time.sleep(interval_s)
Issue-Commit: 69a812023dc7b19082ccb88a67a077b5f3204d48

# stepfunctions: 3 net-new causes — an unresolvable Choice comparator path reports a cause that names no path, DescribeExecution on an EXPRESS execution ARN answers ExecutionDoesNotExist instead of InvalidArn, and a zero-tolerance DISTRIBUTED Map run never terminates

> _Note: this issue was identified against the code base at commit `69a812023dc7b19082ccb88a67a077b5f3204d48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Three net-new root causes behind 4 failing checks.

Cause 1 of 3: when a Choice rule's comparator path cannot be resolved against the execution data, the States.Runtime cause does not name the path that failed to resolve.
  AWS: recorded behavior only (the Amazon States Language is not in the Smithy model; DescribeExecution models error/cause as SensitiveError/SensitiveCause). The recording shows the execution FAILED with error States.Runtime and a cause that CONTAINS the unresolvable path exactly as the definition writes it ('$.limit', '$.n'), so a client can tell which reference broke. Hypothesis for the full template: "An error occurred while executing the state '<state name>' (entered at the event id #<id>). Invalid path '<path>' : No results for path: $['<field>']" — the state name, event id and path are values from the definition and the run; what the recording establishes as the requirement is that the referenced path appears in the cause.
  Emulator: the cause is "An error occurred while executing the state 'Route' (entered at the event id #2). The choice state's condition path references an invalid value." — the trailing sentence is emulator-authored fixed text carrying no request-derived value, and the path ('$.limit') is nowhere in it. Only the state name and event id are real.
  Why: a client debugging a failed execution (or a Catch that classifies on the cause) cannot tell WHICH path was missing; with several comparators in a rule the message is useless, and log/alert parsers keyed on AWS's path-naming cause never match.
  Fix: the Choice state's path-resolution diagnostics in the sfn service crate's interpreter (services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs — the cause string built when a *Path/Variable reference does not resolve).
  Traces: stepfunctions.choice-path-comparators-dereference-the-referenced-field, stepfunctions.deep-sfn-language-choice-missing-comparator-path-rejected. (Open issue 20261029T183117Z-sfn-interpreter-and-data-fidelity-five-causes-still-open.md covers Choice *Path TYPE mismatches failing instead of not matching — a different cause; it does not carry these keys.)

Cause 2 of 3: DescribeExecution on an EXPRESS execution ARN answers ExecutionDoesNotExist instead of refusing the ARN by its resource type.
  AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions): DescribeExecution models both ExecutionDoesNotExist and InvalidArn (both HTTP 400). Recorded behavior: an EXPRESS execution ARN is refused by its RESOURCE TYPE, with code InvalidArn and the message "Invalid Arn: 'Resource type not valid in this context: express'" — entirely fixed text, the word express being the ARN's resource type. The same ARN is refused identically by StopExecution, which the emulator already gets right in this check.
  Emulator: DescribeExecution answers code 'ExecutionDoesNotExist' with "Execution Does Not Exist: '<execution arn>'". It also mints a standard ':execution:' ARN for the EXPRESS run rather than an ':express:' one (see open issue 20261103T052008Z-states-express-executions-get-a-standard-execution-arn-so.md, filed under the 'states' heading and not carrying this check's key), which is why the two operations disagree about the same ARN.
  Why: the two refusals are a client's only way to learn that EXPRESS executions have no execution-scoped read; ExecutionDoesNotExist says instead that the execution is gone, so callers retry or treat a started execution as lost, and code branching on InvalidArn never fires.
  Fix: the EXPRESS resource-type ARN check in DescribeExecution (and every execution-scoped read) in the sfn service crate, together with the ':express:' execution-ARN minting in StartExecution that it keys on.
  Traces: stepfunctions.stop-execution-on-express-unsupported.

Cause 3 of 3: a DISTRIBUTED Map run with ToleratedFailurePercentage never reaches a terminal state — the check is killed at the harness deadline.
  AWS: recorded behavior only. With 20 items of which 1 fails and ToleratedFailurePercentage=0, the first failing child fails the map and the execution ends FAILED with error States.ExceedToleratedFailureThreshold while the 19 completed children keep their rows (no rollback); the same machine at ToleratedFailurePercentage=10 ends SUCCEEDED with the same 19 rows. Both settle well inside the check's own 200 s per-run budget.
  Emulator: 'Check timed out after 180.0s' — neither the zero-tolerance run nor the tolerant control settles, so the refusal and its error name are unobservable.
  Why: a client cannot use Distributed Map's failure-tolerance contract at all: executions hang instead of terminating, and anything waiting on a terminal status blocks forever.
  Fix: the Map-state/map-run driver in the sfn service crate — drive child executions to completion, settle the parent, and surface States.ExceedToleratedFailureThreshold when the tolerance is exceeded.
  Traces: stepfunctions.conc-distributed-map-zero-tolerance-run-fails-and-keeps-completed-items. (Open issue 20261106T214711Z-stepfunctions-4-net-new-causes-resultwriter-refused-as.md lists this check id under its Cause 3, but its replay key for it is a _wait_terminal timeout and the run now dies at the harness deadline, so the suite did not report this failure as owned; if that issue is still open these two should be reconciled rather than fixed twice.)

## Evidence
Cause 1: AssertionError: the cause must name the path that failed to resolve ($.limit); got "An error occurred while executing the state 'Route' (entered at the event id #2). The choice state's condition path references an invalid value." and AssertionError: the cause must name the unresolvable path '$.limit', got "An error occurred while executing the state 'C' (entered at the event id #2). The choice state's condition path references an invalid value." (definition: {"C": {"Type": "Choice", "Choices": [{"Variable": "$.n", "NumericLessThanPath": "$.limit", "Next": "Y"}], "Default": "N"}} with input {"n": 1}). State name and event id are per-definition values; the requirement is that the cause contains the referenced path.

Cause 2: AssertionError: expected DescribeExecution to also reject the EXPRESS execution ARN, got {'Message': "Execution Does Not Exist: 'arn:aws:states:us-west-2:000000000000:execution:sim2real-behav-sm-<id>:<uuid>'", 'Code': 'ExecutionDoesNotExist'}. Expected Code 'InvalidArn' with Message "Invalid Arn: 'Resource type not valid in this context: express'" (all fixed text). Note the ARN itself carries ':execution:' where AWS mints ':express:'.

Cause 3: 'Error: Check timed out after 180.0s' for a 20-item DISTRIBUTED Map (1 failing item) run first at ToleratedFailurePercentage=10 (expected SUCCEEDED) and then at 0 (expected FAILED with error States.ExceedToleratedFailureThreshold and the 19 completed rows retained).

## Steps to Reproduce
Cause 1: CreateStateMachine with a Choice state whose rule is {"Variable": "$.n", "NumericLessThanPath": "$.limit", "Next": "Y"} and a Default; StartExecution with {"n": 1} (and again with {"limit": 2}); read DescribeExecution.cause — it must contain '$.limit' (resp. '$.n').
Cause 2: CreateStateMachine type=EXPRESS, StartExecution, then DescribeExecution(executionArn=<that arn>) — expect InvalidArn with "Invalid Arn: 'Resource type not valid in this context: express'"; the emulator answers ExecutionDoesNotExist. StopExecution on the same ARN already answers InvalidArn.
Cause 3: put a 20-element items.json in S3 with one item flagged to fail; CreateStateMachine with a Map state ProcessorConfig.Mode=DISTRIBUTED, an ItemReader over that object and a DynamoDB write per item; run it with ToleratedFailurePercentage=10 (expect SUCCEEDED) and with 0 (expect FAILED States.ExceedToleratedFailureThreshold, 19 rows kept). Poll DescribeExecution — neither run leaves RUNNING.
