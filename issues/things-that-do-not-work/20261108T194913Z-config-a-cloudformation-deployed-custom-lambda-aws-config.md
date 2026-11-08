Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.deploy-config-rule-cloudformation-evaluates-resource
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.deploy-config-rule-cloudformation-evaluates-resource|_poll_compliance#16|TimeoutError|raise TimeoutError(f"compliance for account {account_id} reaching {expected!r}: the run budget ended first; not graded")
Issue-Commit: cb2802bcfee81b1f998edc25751cc74821df892a

# config: a CloudFormation-deployed CUSTOM_LAMBDA AWS::Config::ConfigRule is never evaluated — StartConfigRulesEvaluation has no effect and GetComplianceDetailsByConfigRule stays empty

> _Note: this issue was identified against the code base at commit `cb2802bcfee81b1f998edc25751cc74821df892a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComplianceDetailsByConfigRule

## Description
One net-new root cause in the AWS Config emulator, surfaced by the only `### config` failing check in this replay that is not marked ALREADY FILED or OWNED.

Cause 1 of 1: a custom-Lambda (Source.Owner=CUSTOM_LAMBDA) config rule provisioned by a CloudFormation stack is created and readable, but Config never dispatches it to its evaluator Lambda, so no evaluation result ever exists for it.

AWS: Smithy (com.amazonaws.configservice#StarlingDoveService) models PutConfigRule with Source.Owner=CUSTOM_LAMBDA + Source.SourceIdentifier (the evaluator Lambda ARN) and SourceDetails[{EventSource=aws.config, MessageType=ScheduledNotification, MaximumExecutionFrequency=TwentyFour_Hours}]; StartConfigRulesEvaluation (input ConfigRuleNames, empty output, errors InvalidParameterValueException / LimitExceededException / NoSuchConfigRuleException / ResourceInUseException); PutEvaluations (Evaluations list, ResultToken required, errors InvalidParameterValueException / InvalidResultTokenException / NoSuchConfigRuleException); GetComplianceDetailsByConfigRule (input ConfigRuleName required, output EvaluationResults). On AWS, StartConfigRulesEvaluation on such a rule causes Config to invoke the evaluator Lambda with an event carrying accountId, configRuleName and a valid resultToken; the Lambda's PutEvaluations verdict then becomes a row in GetComplianceDetailsByConfigRule for that rule, qualified by EvaluationResultIdentifier.EvaluationResultQualifier.ResourceId (here the account id, ComplianceResourceType AWS::::Account), with the ComplianceType and the Annotation the Lambda supplied. Recorded behavior confirms the detail row appears within the poll window.

Emulator: the stack provisions and DescribeConfigRules(ConfigRuleNames=[<rule>]) resolves exactly one rule with Source.Owner=CUSTOM_LAMBDA (those assertions pass), and StartConfigRulesEvaluation is accepted 2xx — but the evaluator Lambda is never invoked, no PutEvaluations verdict is ever recorded, and GetComplianceDetailsByConfigRule(ConfigRuleName=<rule>) answers 200 with EvaluationResults containing no entry whose EvaluationResultQualifier.ResourceId is the caller's account for the entire poll window (the poll ends with no NON_COMPLIANT, and in this replay the run budget expired first — the harness reports `TimeoutError: compliance for account <accountId> reaching 'NON_COMPLIANT': the run budget ended first`). There is no refusal and no error code: the evaluation pipeline simply has no effect.

Why: a client that deploys its Config rule baseline through CloudFormation (the normal path) is told the stack and the rule are fine, then can never learn the rule's verdict. Nothing distinguishes "not evaluated yet" from "will never be evaluated", so compliance dashboards read the account as clean, gating logic that waits for NON_COMPLIANT hangs forever, and the Annotation the evaluator wrote is unobservable.

Fix: the Config rule-evaluation engine — the dispatcher that, on rule creation and on StartConfigRulesEvaluation, invokes a CUSTOM_LAMBDA rule's Source.SourceIdentifier Lambda with a usable ResultToken, plus the PutEvaluations handler/state that must land the returned verdict (ComplianceType, Annotation, OrderingTimestamp, resource qualifier) in the rule's evaluation-result store so GetComplianceDetailsByConfigRule projects it. The CloudFormation provisioner for AWS::Config::ConfigRule must register the rule into that same engine, not only into the rule store. Open issue 20261107T193246Z-config-4-net-new-causes-custom-lambda-and.md describes the same dispatch gap for a directly-created rule but does NOT carry this check's replay key (its Issue-Trace-Ids list config.aggregate-config-rule-compliance-summary-matches-details, not this check), so if its fix covers the dispatch path this check must still be replayed to confirm it.

Traces: config.deploy-config-rule-cloudformation-evaluates-resource.

## Evidence
From the behavioral replay (123/177 passed). This is the only `### config` failure not marked ALREADY FILED / OWNED.

config.deploy-config-rule-cloudformation-evaluates-resource [error]:
  check_config_deploy_config_rule_cloudformation_evaluates_resource.py line 126 -> _poll_compliance line 320
  TimeoutError: compliance for account 000000000000 reaching 'NON_COMPLIANT': the run budget ended first; not graded

  Expected (AWS): after CreateStack provisions AWS::IAM::Role + AWS::Lambda::Function (python3.12 handler calling config.put_evaluations with ComplianceResourceType 'AWS::::Account', ComplianceResourceId=event['accountId'], ComplianceType 'NON_COMPLIANT', Annotation 'sim2real-cfn-deploy-check', ResultToken=event['resultToken']) + AWS::Lambda::Permission(Principal config.amazonaws.com) + AWS::Config::ConfigRule(Source.Owner CUSTOM_LAMBDA, SourceIdentifier = the function ARN, ScheduledNotification/TwentyFour_Hours), a StartConfigRulesEvaluation(ConfigRuleNames=[<rule>]) causes the Lambda to be invoked and GetComplianceDetailsByConfigRule(ConfigRuleName=<rule>) to return an EvaluationResults entry whose EvaluationResultIdentifier.EvaluationResultQualifier.ResourceId == the caller's account id, ComplianceType == 'NON_COMPLIANT' and Annotation == the value the Lambda passed.

  Actual (emulator): DescribeConfigRules already shows the rule with Source.Owner=CUSTOM_LAMBDA and StartConfigRulesEvaluation returns success, but GetComplianceDetailsByConfigRule never yields any result row for the account for the whole polling window — no ComplianceType is ever observed (the poll's `last` stays None), so the Annotation assertion is never reached. The Lambda is never invoked and no PutEvaluations verdict is recorded.

  Notes on per-run values: the account id (000000000000 here), the stack/function/role/rule names (sim2real-*-<id>) and the poll duration are per-run values, not part of the requirement; the requirement is that an evaluation result for the evaluated resource, with the evaluator's ComplianceType and Annotation, becomes readable after the rule is evaluated. The replay's poll ended on the run's remaining time rather than its own 180 s cap, so the window length is environment-dependent — the divergence is that no evaluation was ever dispatched or recorded, matching the already-filed CUSTOM_LAMBDA dispatch gap in 20261107T193246Z-config-4-net-new-causes-custom-lambda-and.md for a different check.

## Steps to Reproduce
1. Ensure a configuration recorder (and delivery channel) exist for the account/region.
2. CreateStack with a template declaring: AWS::IAM::Role trusting lambda.amazonaws.com with an inline policy allowing config:PutEvaluations; AWS::Lambda::Function (python3.12, inline ZipFile code that calls config.put_evaluations with Evaluations=[{ComplianceResourceType:'AWS::::Account', ComplianceResourceId: event['accountId'], ComplianceType:'NON_COMPLIANT', Annotation:'sim2real-cfn-deploy-check', OrderingTimestamp: now}] and ResultToken=event['resultToken']); AWS::Lambda::Permission(Action lambda:InvokeFunction, Principal config.amazonaws.com); AWS::Config::ConfigRule with Source{Owner:CUSTOM_LAMBDA, SourceIdentifier: Fn::GetAtt EvalFunction.Arn, SourceDetails:[{EventSource:aws.config, MessageType:ScheduledNotification, MaximumExecutionFrequency:TwentyFour_Hours}]}. Wait for CREATE_COMPLETE.
3. DescribeConfigRules(ConfigRuleNames=[<rule>]) — one rule, Source.Owner == CUSTOM_LAMBDA (this already works).
4. StartConfigRulesEvaluation(ConfigRuleNames=[<rule>]) — accepted.
5. Poll GetComplianceDetailsByConfigRule(ConfigRuleName=<rule>): on AWS an EvaluationResults entry for ResourceId == the account id appears with ComplianceType 'NON_COMPLIANT' and Annotation 'sim2real-cfn-deploy-check'; on the emulator EvaluationResults never contains any entry for that resource, and the evaluator Lambda records no invocation.
