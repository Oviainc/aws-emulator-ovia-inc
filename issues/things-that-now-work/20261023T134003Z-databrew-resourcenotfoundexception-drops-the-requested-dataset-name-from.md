Issue-Source: trace-sweep-divergence
Issue-Trace: databrew-dataset-lifecycle
Issue-Step: task#8
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: databrew-dataset-lifecycle
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|DescribeDataset|4xx->4xx|$.Message
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# databrew ResourceNotFoundException drops the requested dataset name from its message

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDataset

## Description
Cause 1 of 1: DescribeDataset on a dataset that does not exist returns the right status and error shape but a generic, request-independent message.

AWS: Smithy (services/service-com-amazonaws-databrew-awsgluedatabrew/smithy-model/com-amazonaws-databrew-awsgluedatabrew.json): DescribeDataset GET /datasets/{Name}, Name is an @httpLabel required member; modeled error ResourceNotFoundException, errorKind client, httpError 404, with a single `Message` member. The model documents the EXCEPTION as "One or more resources can't be found." but does not state the message emitted for a particular lookup. The recording shows the message is built from the request: it is a 404 ResourceNotFoundException whose `Message` names the resource type and the exact `Name` that was looked up (recorded example: "Dataset sim2real-probe-databrew-ds-missing-db7f31a4 wasn't found."). That request-derived rule -- resource kind plus the requested dataset name -- is the requirement, not the recorded text.

Emulator: replays 404 with the shape-level documentation string instead, `$.Message` = "One or more resources can't be found." -- a constant that contains no part of the request. HTTP status (404) and the error shape are correct; only the message is request-independent.

Why: a client that inspects ResourceNotFoundException.Message to report or log WHICH dataset was missing gets nothing actionable, and tooling that scrapes the resource name out of the message (common in DataBrew orchestration scripts and in test assertions over not-found paths) sees a constant and cannot tell one missing dataset from another, nor a missing dataset from any other missing DataBrew resource (recipe, job, project).

Fix: the DescribeDataset handler's not-found path in the databrew service crate -- where the dataset lookup miss is turned into ResourceNotFoundException. It should format the message from the requested `Name` ("Dataset <Name> wasn't found.") rather than returning the shape's doc string as a literal. If the generic constant is shared by all databrew not-found errors, give the dataset lookup its own resource-named message instead of reusing it.

Traces: databrew-dataset-lifecycle#task#8 (DescribeDataset of an absent dataset, expected 404 / ResourceNotFoundException with the dataset-named message, got 404 with the constant message).

## Evidence
Fixture databrew-dataset-lifecycle, step task#8, operation DescribeDataset.
Expected status 400? no -- expected 404, actual 404 (status matches).
Body divergence at $.Message:
  expected: "Dataset sim2real-probe-databrew-ds-missing-db7f31a4 wasn't found."  (example; the requirement is that the message names the resource kind and the requested Name)
  actual:   "One or more resources can't be found."  (constant, independent of the request)
Replay divergence_key: service=databrew|DescribeDataset|status=404->404|keys=$.Message
Model support: ResourceNotFoundException (client, httpError 404, member Message); DescribeDataset.Name is @httpLabel + @required, so the name is always available to the handler.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1); fixture databrew-dataset-lifecycle, step task#8.
2. Equivalent by hand: GET /datasets/<a-dataset-name-that-was-never-created> against the databrew endpoint (aws databrew describe-dataset --name <absent-name>).
3. Observe HTTP 404 with ResourceNotFoundException and inspect $.Message: the emulator returns the constant "One or more resources can't be found." instead of a message naming the requested dataset, e.g. "Dataset <absent-name> wasn't found."
