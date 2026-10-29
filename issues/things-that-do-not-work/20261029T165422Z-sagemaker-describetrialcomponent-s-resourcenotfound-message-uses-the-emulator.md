Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.e2e-experiment-trial-component-association-tracks-run sagemaker.e2e-trial-component-association-lineage-flow
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.e2e-experiment-trial-component-association-tracks-run|run#37|AssertionError|assert tc_message == expected, f"expected message {expected!r}, got {tc_message!r}"
Issue-Key: sagemaker.e2e-trial-component-association-lineage-flow|run#30|AssertionError|assert tc_message == expected, f"expected message {expected!r}, got {tc_message!r}"
Issue-Commit: 06b0d41e00354be103b9e8418d096ed156619087

# SageMaker: DescribeTrialComponent's ResourceNotFound message uses the emulator's generic "Could not find …" wording instead of AWS's "Trial component '<arn>' does not exist."

> _Note: this issue was identified against the code base at commit `06b0d41e00354be103b9e8418d096ed156619087`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTrialComponent

## Description
One root cause. This issue bundles ONE cause and that cause must be closed before submitting.

Cause 1 of 1: DescribeTrialComponent refuses an unknown/deleted trial component with the correct error code but the wrong message text.

AWS: Smithy: DescribeTrialComponent takes TrialComponentName (ExperimentEntityNameOrArn, @length 1..256, pattern ^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:(experiment|experiment-trial|experiment-trial-component|artifact|action|context)\/)?([a-zA-Z0-9](-*[a-zA-Z0-9]){0,119})$) and models exactly one error, ResourceNotFound (HTTP 400, "Resource being access is not found."). For a trial component that does not exist (here: one that was deleted), recorded AWS answers HTTP 400 / code ResourceNotFound with the message template: the fixed text `Trial component '` + `' does not exist.` around the component's full ARN, i.e. `Trial component '<arn>' does not exist.`. The only variable part is the ARN, which is derived from the request's TrialComponentName plus the request's region and the caller's account: arn:aws:sagemaker:<region>:<account>:experiment-trial-component/<TrialComponentName>. This is the same sentence family the sibling entities use and that this emulator already gets right for them — DescribeTrial answers `Trial '<arn>' does not exist.` and DescribeExperiment answers `Experiment '<arn>' does not exist.`, both of which PASS in the same two checks, so only the trial-component reader diverges.

Emulator: HTTP 400 and code ResourceNotFound are correct, but the message is the emulator's own generic not-found sentence: `Could not find trial component "<arn>".` — different fixed text (`Could not find ` … `.`), different quoting of the ARN (double quotes instead of single quotes), and no `does not exist` clause.

Why: a client that reads the refusal's message rather than only its code gets text real AWS never emits. Operators and log-matching/alerting rules keyed on AWS's `does not exist` sentence do not fire, and the emulator is internally inconsistent: the same deleted-experiment teardown yields AWS wording for the trial and the experiment but emulator wording for the trial component, so a caller cannot parse the three uniformly.

Fix: the operation's handler — the not-found closure in DescribeTrialComponent must build the message with the trial-component equivalent of the state slice's own `trial_does_not_exist` sentence helper instead of the generic `could_not_find` helper, so every reader of a missing trial component words it the way AWS does.

Traces: sagemaker.e2e-experiment-trial-component-association-tracks-run, sagemaker.e2e-trial-component-association-lineage-flow. In both, the trial and experiment message assertions pass first and only the trial-component message assertion fails.

## Evidence
Cause 1 — both checks fail on the same assertion (`assert tc_message == expected`):

sagemaker.e2e-experiment-trial-component-association-tracks-run (check_sagemaker_e2e_experiment_trial_component_association_tracks_run.py:117):
  AssertionError: expected message "Trial component 'arn:aws:sagemaker:us-west-2:000000000000:experiment-trial-component/sim2real-sm-tc-4c18d33ee206' does not exist.", got 'Could not find trial component "arn:aws:sagemaker:us-west-2:000000000000:experiment-trial-component/sim2real-sm-tc-4c18d33ee206".'

sagemaker.e2e-trial-component-association-lineage-flow (check_sagemaker_e2e_trial_component_association_lineage_flow.py:93):
  AssertionError: expected message "Trial component 'arn:aws:sagemaker:us-west-2:000000000000:experiment-trial-component/sim2real-sm-tc-5dd79a12fceb' does not exist.", got 'Could not find trial component "arn:aws:sagemaker:us-west-2:000000000000:experiment-trial-component/sim2real-sm-tc-5dd79a12fceb".'

(The component names, region and account above are per-run values that fill the template; the requirement is the template, not these literals.)

Expected vs actual:
  status: 400 vs 400 (matches)
  code:   ResourceNotFound vs ResourceNotFound (matches)
  message fixed text: "Trial component '" … "' does not exist." vs "Could not find trial component \"" … "\"."
  message variable part: the component's ARN, from the request's TrialComponentName + request region + caller account (same value in both expected and actual, so the ARN itself is not the defect)

Code citations grounding the cause:
  services/service-com-amazonaws-sagemaker-sagemaker/src/ops/describe_trial_component.rs:172-199 — the `not_found` closure builds `message` from `crate::state::could_not_find(crate::state::TRIAL_COMPONENT_KIND_LABEL, identifier)` on both the bad-ARN path (line 188) and the unknown-name path (lines 195-199).
  services/service-com-amazonaws-sagemaker-sagemaker/src/ops/describe_trial.rs:157-163 — the sibling reader uses `crate::state::trial_does_not_exist(...)`, which is why DescribeTrial's message matches AWS in the same checks.

Not filed as separate findings (not emulator-caused): nothing in these two checks failed on a runner tooling, TLS/DNS or per-run-value difference; the only divergence is the message text above.

## Steps to Reproduce
1. CreateExperiment(ExperimentName=E).
2. CreateTrial(TrialName=T, ExperimentName=E).
3. CreateTrialComponent(TrialComponentName=C).
4. AssociateTrialComponent(TrialComponentName=C, TrialName=T), then DisassociateTrialComponent(TrialComponentName=C, TrialName=T).
5. DeleteTrialComponent(TrialComponentName=C); DeleteTrial(TrialName=T); DeleteExperiment(ExperimentName=E).
6. Poll DescribeTrialComponent(TrialComponentName=C) until it is refused, and read Error.Code and Error.Message.
   Expect: 400, Code=ResourceNotFound, Message="Trial component 'arn:aws:sagemaker:<region>:<account>:experiment-trial-component/C' does not exist."
   Observe: 400, Code=ResourceNotFound, Message='Could not find trial component "arn:aws:sagemaker:<region>:<account>:experiment-trial-component/C".'
7. For contrast, DescribeTrial(TrialName=T) and DescribeExperiment(ExperimentName=E) on the same deleted resources already return AWS's wording ("Trial '<arn>' does not exist." / "Experiment '<arn>' does not exist."), confirming the divergence is confined to the trial-component reader.
8. A short form that needs no teardown: DescribeTrialComponent(TrialComponentName='never-created-component') on a fresh emulator reproduces the same message divergence.
