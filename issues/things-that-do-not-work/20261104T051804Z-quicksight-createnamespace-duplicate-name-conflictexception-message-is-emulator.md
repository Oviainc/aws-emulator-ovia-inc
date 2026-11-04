Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.duplicate-namespace-creation-rejected
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight.duplicate-namespace-creation-rejected|run#10|AssertionError|assert err["Message"] == (
Issue-Commit: 04fa8901c0470ed069cb581dfd2246db1b5bced9

# quicksight CreateNamespace duplicate-name ConflictException message is emulator-authored ("The namespace <ns> already exists.") instead of AWS's "Cannot create a namespace when status is CREATED. Failed to create namespace <ns> for account <acct>. "

> _Note: this issue was identified against the code base at commit `04fa8901c0470ed069cb581dfd2246db1b5bced9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNamespace

## Description
One net-new root cause in quicksight.

Cause 1 of 1: CreateNamespace for a name the account already holds is refused with the right class but the wrong message text.

AWS: Smithy (services/service-com-amazonaws-quicksight-quicksight-20180401/smithy-model/com-amazonaws-quicksight-quicksight-20180401.json) models CreateNamespace with error ConflictException (HTTP 409, "Updating or deleting a resource can cause an inconsistent state."), and documents that "The namespace must be unique within the Amazon Web Services account." The model does not fix the message string, so the message itself is recorded behavior only: the recording shows real QuickSight answering the second CreateNamespace for an existing namespace with HTTP 409, Code "ConflictException", and Message built from this template — fixed text "Cannot create a namespace when status is CREATED. Failed to create namespace ", then the requested namespace name, then " for account ", then the AwsAccountId from the request, then ". " (the sentence ends with a period followed by a single trailing space). Its only variable parts are the Namespace value from the request and the AwsAccountId from the request; the status word ("CREATED") is the CreationStatus the already-existing namespace currently holds in service state, not a constant. Everything else is fixed text.

Emulator: answers ConflictException/409 (class and code are correct) but with its own sentence, "The namespace <namespace> already exists." — e.g. "The namespace s2rdupb32b1b97c06f already exists." It never mentions the existing namespace's status, never names the account, and omits the "Failed to create namespace ... for account ..." clause and the trailing space. The same handler also takes this path for the built-in default namespace, so that refusal carries the same invented wording.

Why: a client that keys on the refusal text gets the wrong answer. Callers and SDK/CLI users parse this message to distinguish "already exists in CREATED state" from the other states QuickSight reports here (a namespace stuck in CREATING, DELETING or a *_FAILURE status produces the same error class with a different status word), and to recover the account id from the failure when a batch provisioner refuses across several accounts. Golden-file and error-text assertions written against AWS fail against the emulator even though the HTTP status and error code match, and any logic branching on the status word in the message cannot branch at all.

Fix: the refusal in the CreateNamespace handler's duplicate-name branch — the ConflictException message it formats. It must render AWS's template, interpolating the requested Namespace, the request's AwsAccountId, and the existing namespace record's current CreationStatus (the status derived from the stored record, as DescribeNamespace reports it), including the trailing space. The default-namespace branch in the same handler must render the same template with that namespace's status.

Traces: behavioral check quicksight.duplicate-namespace-creation-rejected, which asserts Code == "ConflictException" (passes) and then the message template (fails). Handler: services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/create_namespace.rs lines 215-224.

No open issue covers this: the two existing quicksight message-wording issues (20261031T224711Z-quicksight-data-source-resourcenotfoundexception-message-is-invented-wording.md and 20261103T040231Z-quicksight-data-set-resourcenotfoundexception-message-is-invented-wording.md) are ResourceNotFoundException wording on data sources and data sets, a different operation and a different error class.

## Evidence
Expected (real AWS, recorded): HTTP 409, Error.Code = "ConflictException", Error.Message = "Cannot create a namespace when status is CREATED. Failed to create namespace <Namespace> for account <AwsAccountId>. " — fixed text: "Cannot create a namespace when status is ", ". Failed to create namespace ", " for account ", ". " (trailing space); variable parts: the existing namespace's CreationStatus from state, the Namespace and AwsAccountId from the request.

Actual (emulator): HTTP 409, Error.Code = "ConflictException", Error.Message = "The namespace <Namespace> already exists."

Replay output: AssertionError: {'Message': 'The namespace s2rdupb32b1b97c06f already exists.', 'Code': 'ConflictException'} raised at check_quicksight_duplicate_namespace_creation_rejected.py line 38 (the `assert err["Message"] == ...` after `assert err["Code"] == "ConflictException"` passed). The namespace name and account id in that line are per-run values filling the template, not part of it.

Source: services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/create_namespace.rs:219 — `message: Some(format!("The namespace {} already exists.", input.namespace))`.

## Steps to Reproduce
1. Ensure the QuickSight account subscription exists (DescribeAccountSubscription / CreateAccountSubscription with Edition=ENTERPRISE, AuthenticationMethod=IAM_ONLY).
2. CreateNamespace(AwsAccountId=<acct>, Namespace=<new-name>, IdentityStore="QUICKSIGHT").
3. Poll DescribeNamespace(AwsAccountId=<acct>, Namespace=<new-name>) until CreationStatus == "CREATED".
4. Call CreateNamespace again with the SAME AwsAccountId and Namespace.
5. Inspect the ClientError: Code is "ConflictException" (correct) but Message is "The namespace <new-name> already exists." instead of "Cannot create a namespace when status is CREATED. Failed to create namespace <new-name> for account <acct>. ".
