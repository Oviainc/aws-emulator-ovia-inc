Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-serverless.snapshot-of-workgroupless-namespace-rejected redshift-serverless.usage-limit-on-absent-workgroup-arn-rejected
Issue-Shape-Id: com.amazonaws.redshiftserverless#RedshiftServerless
Issue-Key: redshift-serverless.snapshot-of-workgroupless-namespace-rejected|run#9|AssertionError|assert message == (
Issue-Key: redshift-serverless.usage-limit-on-absent-workgroup-arn-rejected|run#14|AssertionError|assert message == f"The Resource ARN is not valid as the workgroup ID {absent_id}.", message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# redshift-serverless refusal messages diverge from AWS wording: CreateSnapshot on a workgroupless namespace appends the namespace name, and CreateUsageLimit on an unresolvable workgroup ARN uses an invented sentence (2 root causes)

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshot

## Description
Two independent root causes in service-com-amazonaws-redshiftserverless-redshiftserverless. Both refusals pick the RIGHT error shape and status, and both are rejected for the right reason — only the message text diverges from what Redshift Serverless words for that error class. A client (or a check) that matches on the message therefore reads an unknown failure. No open issue covers either; the closest redshift-serverless entries already filed are the UpdateUsageLimit scaffold stub and the redshift-data entries, neither of which touches these messages.

=== Cause 1 of 2: CreateSnapshot of a namespace that has no workgroup — message carries a namespace name AWS does not put in it ===
AWS: a snapshot is taken through the compute attached to a namespace, so CreateSnapshot against an AVAILABLE namespace with no workgroup is refused with ResourceNotFoundException (Smithy: CreateSnapshot errors include ResourceNotFoundException, httpError 404; the message member is a plain String, so the wording itself is recorded behavior only). The recorded message is entirely FIXED text with NO variable part: "You can't create a snapshot because there isn't a workgroup created for the namespace." — the sentence ends at "namespace." and names no resource.
Emulator: refuses correctly with ResourceNotFoundException, but builds the message by interpolating the requested namespaceName before the period, producing "You can't create a snapshot because there isn't a workgroup created for the namespace <namespaceName>." (in the failing replay, the trailing token was that run's generated namespace name — a per-run value that fills the emulator's template, not part of any requirement).
Why: the message is a requirement of the error class. A client that compares the refusal message against AWS's documented sentence, or that parses it to decide the namespace simply lacks compute versus does not exist, never matches, and the trailing name makes the string unstable across runs where AWS's is constant.
Fix: the workgroupless-namespace refusal branch in the CreateSnapshot handler — the ResourceNotFoundException message must be the fixed sentence with no interpolation (the resource_name member is the right place for the namespace name, and it is already set).
Traces: redshift-serverless.snapshot-of-workgroupless-namespace-rejected. Everything else the check asserts passes: code is ResourceNotFoundException, nothing is stored by the rejected call, and the positive control (same CreateSnapshot after an 8-RPU workgroup exists) succeeds.

=== Cause 2 of 2: CreateUsageLimit against a well-formed workgroup ARN that resolves to nothing — wrong message sentence ===
AWS: CreateUsageLimit with a resourceArn whose partition/region/account/shape are all correct but whose workgroup id names no workgroup is refused with ValidationException (Smithy: CreateUsageLimit errors include ValidationException, httpError 400 — note the model also declares ResourceNotFoundException, but the service answers ValidationException here; message wording is recorded behavior only). The message is the fixed text "The Resource ARN is not valid as the workgroup ID " followed by the ARN's trailing workgroup-id resource segment, taken from the request, then "." — i.e. "The Resource ARN is not valid as the workgroup ID <workgroup-id>." Fixed part: everything except the id; variable part: the id, read off the submitted resourceArn.
Emulator: refuses with the right code and status, but emits a different sentence entirely — "resourceArn <full-arn> does not name a namespace or workgroup in this account and Region" — which quotes the whole ARN instead of its id segment, uses invented wording, and has no trailing period.
Why: a client distinguishing an ARN that does not resolve from an ARN that is syntactically malformed does so from this message, since both answer ValidationException/400; it cannot, and any message assertion against AWS's text fails.
Fix: the resourceArn-resolution refusal branch in the CreateUsageLimit handler — emit AWS's sentence, filling the variable slot with the resource segment of the submitted ARN rather than the whole ARN.
Traces: redshift-serverless.usage-limit-on-absent-workgroup-arn-rejected. The rest of that check passes: code is ValidationException, no limit is attached to the real workgroup, and the same request against the resolving ARN succeeds and lists back.

## Evidence
Cause 1 — redshift-serverless.snapshot-of-workgroupless-namespace-rejected [mismatch]
  Expected (AWS, fixed text, no variable part):
    "You can't create a snapshot because there isn't a workgroup created for the namespace."
  Actual (emulator), with the per-run namespace name appended:
    "You can't create a snapshot because there isn't a workgroup created for the namespace sim2real-behav-rsns-8ec62092c8a6."
  Error code and status agree: ResourceNotFoundException / HTTP 404. Assertion site: check_redshift_serverless_snapshot_of_workgroupless_namespace_rejected.py line 38 (`assert message == (...)`).
  Source of the divergence: services/service-com-amazonaws-redshiftserverless-redshiftserverless/src/ops/create_snapshot.rs lines 234-245 — the message is built with format!("... for the namespace {}.", input.namespace_name).

Cause 2 — redshift-serverless.usage-limit-on-absent-workgroup-arn-rejected [mismatch]
  Expected (AWS): "The Resource ARN is not valid as the workgroup ID <workgroup-id>."  (fixed text + the trailing resource segment of the submitted resourceArn)
  Actual (emulator): "resourceArn arn:aws:redshift-serverless:<region>:<account>:workgroup/<workgroup-id> does not name a namespace or workgroup in this account and Region"
  Error code and status agree: ValidationException / HTTP 400. Assertion site: check_redshift_serverless_usage_limit_on_absent_workgroup_arn_rejected.py line 52.
  Source of the divergence: services/service-com-amazonaws-redshiftserverless-redshiftserverless/src/ops/create_usage_limit.rs lines 191-202 — the message is built with format!("resourceArn {} does not name a namespace or workgroup in this account and Region", input.resource_arn).

## Steps to Reproduce
Cause 1:
1. CreateNamespace(namespaceName=<ns>, dbName="dev") and poll GetNamespace until status == AVAILABLE. Attach NO workgroup.
2. CreateSnapshot(namespaceName=<ns>, snapshotName=<snap>, retentionPeriod=1).
3. Read the error: code is ResourceNotFoundException (correct), but the message ends with " <ns>." instead of " namespace." — AWS's sentence contains no namespace name.

Cause 2:
1. CreateNamespace(<ns>) then CreateWorkgroup(workgroupName=<wg>, namespaceName=<ns>, baseCapacity=8) and take workgroupArn from the response.
2. Build an ARN identical to it except the trailing workgroup id, replaced by a fresh UUID (same partition, region, account and shape — only existence differs).
3. CreateUsageLimit(resourceArn=<that ARN>, usageType="serverless-compute", amount=60, period="daily").
4. Read the error: code is ValidationException (correct), but the message is the emulator's "resourceArn ... does not name a namespace or workgroup in this account and Region" rather than AWS's "The Resource ARN is not valid as the workgroup ID <uuid>."

Replay both with: redshift-serverless.snapshot-of-workgroupless-namespace-rejected, redshift-serverless.usage-limit-on-absent-workgroup-arn-rejected
