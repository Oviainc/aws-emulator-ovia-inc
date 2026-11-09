Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: budgets__cloudformation.deploy-budgets-budget-cost-lifecycle budgets__cloudformation.deploy-budgets-budget-invalid-time-period-rejected-rollback budgets__cloudformation.deploy-budgets-action-apply-iam-policy-lifecycle
Issue-Key: budgets__cloudformation.deploy-budgets-budget-cost-lifecycle|run#9|AssertionError|assert [b["BudgetName"] for b in listed] == [budget_ref], (
Issue-Key: budgets|DescribeBudget|NotFoundException
Issue-Key: budgets__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# budgets__cloudformation: 3 net-new causes — AWS::Budgets::Budget provisions no budget and its Ref is a synthesized physical id, and AWS::Budgets::BudgetsAction is not a registered resource type

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
CloudFormation cannot deploy Budgets resources: a stack declaring AWS::Budgets::Budget reports success without the budget ever existing in the Budgets service, the Ref of that resource is not the budget name, and AWS::Budgets::BudgetsAction is refused at template-validation time. Three distinct causes, all reachable from a plain JSON template with no parameters.

Cause 1 of 3: an AWS::Budgets::Budget resource is never provisioned into the Budgets service.
AWS: CloudFormation's AWS::Budgets::Budget handler calls Budgets CreateBudget with the declared Budget (BudgetName, BudgetLimit, TimeUnit, BudgetType, optional TimePeriod), so once the stack is CREATE_COMPLETE the budget is a normal Budgets resource: DescribeBudgets (Smithy: DescribeBudgets, paginated on Budgets/NextToken) lists exactly one entry whose BudgetName is the declared name, and DescribeBudget(AccountId, BudgetName) (Smithy: DescribeBudget -> Budget, errors AccessDeniedException/InvalidParameterException/NotFoundException 404) answers 200 with that budget — BudgetType COST, TimeUnit MONTHLY, BudgetLimit exactly as declared, and TimePeriod.Start/End the declared epoch seconds. Recorded behavior confirms both reads succeed for the templated name.
Emulator: the stack reaches CREATE_COMPLETE, but no budget is created. DescribeBudgets(AccountId=<account>) contains no entry with the declared BudgetName (the check's filtered list is []), and DescribeBudget for the declared name is refused 404 NotFoundException with message 'Unable to get budget: <BudgetName> - the budget doesn't exist.' (fixed text 'Unable to get budget: ' / ' - the budget doesn't exist.', variable part the BudgetName from the request — here the name the template declared). The resource behaves as a no-op/generic stub: it takes a status but writes no Budgets state.
Why: every IaC-provisioned budget is invisible to the Budgets API. A client that deploys a budget by template and then reads, updates, attaches notifications/subscribers to, or deletes it through Budgets sees NotFoundException for a resource CloudFormation told it was created; the stack delete likewise removes nothing, so the check's post-delete absence assertion is untestable.
Fix: the CloudFormation resource provisioner for the AWS::Budgets::Budget type — it must invoke the Budgets CreateBudget/UpdateBudget/DeleteBudget state transitions on create/update/delete instead of falling through to the generic no-state resource path.
Traces: budgets__cloudformation.deploy-budgets-budget-cost-lifecycle (DescribeBudgets empty for the declared name), budgets__cloudformation.deploy-budgets-budget-invalid-time-period-rejected-rollback (the positive-control stack is CREATE_COMPLETE, then DescribeBudget on its name raises NotFoundException).

Cause 2 of 3: Ref on an AWS::Budgets::Budget resource returns a synthesized physical id instead of the BudgetName.
AWS: for AWS::Budgets::Budget, Ref (and the resource's PhysicalResourceId) is the budget's name — the value you can pass straight to Budgets DescribeBudget(BudgetName=...) and the value that appears as the DescribeBudgets entry's BudgetName (Smithy: BudgetName, length 1-100, pattern '^(?![^:\\]*/action/|(?i).*<script>.*</script>.*)[^:\\]+$'). Recorded behavior: the stack Output built from {"Ref": "Budget"} equals the declared BudgetName.
Emulator: the Output resolves to a CloudFormation-style generated name of the form <stack-name>-<logical-id>-<random suffix> (shape only; the recorded run showed one such value), which is neither the declared BudgetName nor any name Budgets will resolve. So even if Cause 1 is fixed, Ref/GetAtt consumers and any resource referencing {"Ref": "Budget"} (for example a BudgetsAction's BudgetName) address a budget that does not exist.
Why: templates that wire a budget's name into other resources or into Outputs propagate a bogus identifier; downstream Budgets calls using it are refused 404.
Fix: the AWS::Budgets::Budget resource handler's physical-id/Ref resolution in the CloudFormation provisioner — the primary identifier must be the declared Budget.BudgetName, not an auto-generated name.
Traces: budgets__cloudformation.deploy-budgets-budget-cost-lifecycle (Ref 'sim2real-cfnbudget-<stack>-Budget-<suffix>' vs the declared budget name).

Cause 3 of 3: AWS::Budgets::BudgetsAction is not a registered CloudFormation resource type.
AWS: AWS::Budgets::BudgetsAction is a supported type; CreateStack accepts the template and the handler calls Budgets CreateBudgetAction (Smithy: CreateBudgetAction requires AccountId, BudgetName, NotificationType, ActionType, ActionThreshold, Definition, ExecutionRoleArn, ApprovalModel, Subscribers; returns ActionId, a 36-char uuid). The resource's Ref is '<ActionId>|<BudgetName>', GetAtt ActionId is that ActionId, and the action is then visible through DescribeBudgetActionsForBudget (Smithy: output Actions, required) and DescribeBudgetAction.
Emulator: CreateStack is refused 400 ValidationError with message 'Template format error: Unrecognized resource types: [AWS::Budgets::BudgetsAction]' (fixed text 'Template format error: Unrecognized resource types: [...]', variable part the list of type names from the template). Nothing in the template is provisioned, so no budget action can be created by IaC at all, and the action lifecycle (create, the ActionId/Ref representation, the action-dropping update that leaves the budget, and the per-action NotFoundException afterwards) is unreachable.
Why: any template declaring a budget action — the normal way APPLY_IAM_POLICY/SCP/SSM budget actions are deployed — fails at validation, and the whole stack (roles, managed policy, topic, budget) is refused with it.
Fix: the CloudFormation template resource-type registry plus a new AWS::Budgets::BudgetsAction resource handler wired to the Budgets CreateBudgetAction/UpdateBudgetAction/DeleteBudgetAction state transitions, with Ref = '<ActionId>|<BudgetName>' and the ActionId attribute.
Traces: budgets__cloudformation.deploy-budgets-action-apply-iam-policy-lifecycle (CreateStack ValidationError).

Note: this is not covered by the existing open CloudFormation unrecognized-type issues (for example 20261109T113214Z-cloudformation-mediatailor-..., 20261109T113512Z-cloudformation-network-firewall-...), which name other type namespaces; no open issue names AWS::Budgets::*.

## Evidence
Suite: budgets__cloudformation 0/3 (0%).

Cause 1 — deploy-budgets-budget-cost-lifecycle (mismatch):
  stack CREATE_COMPLETE, then
  AssertionError: DescribeBudgets entries named '<budget-name>': []; Ref '<stack>-Budget-<suffix>'
  Expected: exactly one DescribeBudgets entry whose BudgetName == the declared name.
Cause 1 — deploy-budgets-budget-invalid-time-period-rejected-rollback (error):
  control stack CREATE_COMPLETE, then
  botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DescribeBudget operation: Unable to get budget: <budget-name> - the budget doesn't exist.
  Expected: 200 with Budget.BudgetName == the declared name and TimePeriod.Start/End == the declared epoch seconds.

Cause 2 — deploy-budgets-budget-cost-lifecycle:
  Output BudgetRef = {"Ref": "Budget"} resolved to a generated '<stack-name>-<logical-id>-<suffix>' physical name.
  Expected: BudgetRef == the declared Budget.BudgetName.

Cause 3 — deploy-budgets-action-apply-iam-policy-lifecycle (error):
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Budgets::BudgetsAction]
  Expected: CreateStack accepted; stack CREATE_COMPLETE; DescribeBudgetActionsForBudget returns one action whose ActionId == GetAtt BudgetsAction.ActionId; Ref == '<ActionId>|<BudgetName>'; DescribeBudgetAction returns Status STANDBY, ApprovalModel MANUAL, ActionType APPLY_IAM_POLICY, NotificationType ACTUAL, ActionThreshold {ActionThresholdValue: 80.0, ActionThresholdType: PERCENTAGE}, the stack's ExecutionRole ARN, Definition.IamActionDefinition {PolicyArn, Roles:[TargetRole name]} and Subscribers [{SubscriptionType: SNS, Address: topic arn}].

## Steps to Reproduce
Cause 1 and 2:
1. cloudformation CreateStack with TemplateBody {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Budget":{"Type":"AWS::Budgets::Budget","Properties":{"Budget":{"BudgetName":"<name>","BudgetLimit":{"Amount":"50.0","Unit":"USD"},"TimeUnit":"MONTHLY","BudgetType":"COST"}}}},"Outputs":{"BudgetRef":{"Value":{"Ref":"Budget"}}}}
2. Poll DescribeStacks until CREATE_COMPLETE; read the BudgetRef Output -> it is a generated '<stack>-Budget-<suffix>' name, not '<name>' (Cause 2).
3. budgets DescribeBudgets(AccountId=<account>) -> no entry with BudgetName '<name>'; budgets DescribeBudget(AccountId=<account>, BudgetName='<name>') -> 404 NotFoundException 'Unable to get budget: <name> - the budget doesn't exist.' (Cause 1).
4. Same template with Budget.TimePeriod {"Start":"1704067200","End":"1861920000"} reproduces step 3 identically.

Cause 3:
1. cloudformation CreateStack with a template whose Resources include an AWS::Budgets::BudgetsAction (BudgetName {"Ref":"Budget"}, NotificationType ACTUAL, ActionType APPLY_IAM_POLICY, ActionThreshold {Type: PERCENTAGE, Value: 80.0}, Definition.IamActionDefinition {PolicyArn, Roles}, ExecutionRoleArn, ApprovalModel MANUAL, Subscribers [{Type: SNS, Address}]) with Capabilities=["CAPABILITY_IAM"].
2. The call is refused 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::Budgets::BudgetsAction]'.
