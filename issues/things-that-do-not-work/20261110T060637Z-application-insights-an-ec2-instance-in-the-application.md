Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-insights.custom-component-ec2-only-and-exclusive
Issue-Shape-Id: com.amazonaws.applicationinsights#EC2WindowsBarleyService
Issue-Key: application-insights.custom-component-ec2-only-and-exclusive|_component_names#23|Timeout|for c in ai.list_components(ResourceGroupName=group_name)["ApplicationComponentList"]
Issue-Commit: cad593586d705c3641fe11f59f14efd9baecb330

# application-insights: an EC2 instance in the application's resource group is never onboarded as a component, so the whole custom-component flow (CreateComponent) is unreachable

> _Note: this issue was identified against the code base at commit `cad593586d705c3641fe11f59f14efd9baecb330`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListComponents

## Description
One net-new root cause, grounded in the one surviving failing check for this service. The open issue `20261109T180657Z-application-insights-an-application-never-discovers-its-resource.md` describes the same discovery gap from the S3-bucket side and lists this check in its own trace ids, but the suite no longer binds this check to it (it is NOT reported as OWNED by that issue in this run's report), so the check is unowned and is filed here. That issue does not cover the EC2/custom-component surface below.

Cause 1 of 1: CreateApplication onboards no components, so an EC2 instance that the application's Resource Groups group resolves to is never reported by ListComponents, and every CreateComponent behavior behind it is unreachable.
Failing check: application-insights.custom-component-ec2-only-and-exclusive

AWS: ListComponents (Smithy: com.amazonaws.applicationinsights#ListComponents, output ApplicationComponentList, HTTP 200, paginated by NextToken/MaxResults with MaxResults range 1..40) is documented as listing "the auto-grouped, standalone, and custom components of the application". Creating an application over a resource group makes Application Insights onboard every supported resource the group resolves to: each resource that is not part of a custom component becomes a standalone ApplicationComponent whose ComponentName is that resource's ARN, with Monitor=false. For a TAG_FILTERS_1_0 group that resolves to a tagged S3 bucket and a tagged EC2 instance, ListComponents therefore reports exactly two components, named `arn:aws:s3:::<bucket>` and `arn:aws:ec2:<region>:<account>:instance/<instance-id>` — the ARNs are values taken from the request/state, the rule (one standalone component per resolved resource, named by its ARN) is the requirement. Only once that instance component exists are the CreateComponent behaviors this check then asserts reachable: CreateComponent (Smithy: input ResourceGroupName, ComponentName @pattern ^[\d\w\-_\.+]*$ @length 1..128, ResourceList, all required; errors ValidationException 400, ResourceInUseException 400, ResourceNotFoundException 404) refuses a non-EC2 ARN with HTTP 400 ValidationException and the message "Only EC2 Instance can be grouped." (fixed text, no variable part — recorded behavior; the model documents no message); it refuses a resource already grouped by another custom component with HTTP 400 ValidationException and the message "Component <resource-arn> is grouped in another component." where `<resource-arn>` is the ARN taken from the request's ResourceList and the rest is fixed text (recorded behavior only); a successful CreateComponent over an EC2 instance absorbs that instance's attached EBS volume into DescribeComponent's ResourceList and removes the instance's standalone default component from ListComponents; DeleteComponent releases the instance so it reverts to its own standalone component and can be grouped again.

Emulator: CreateApplication stores an empty component map and nothing ever populates it; ListComponents only pages that stored map. So ListComponents answers HTTP 200 with an empty ApplicationComponentList for the life of the application, and the instance ARN never appears. The check polls ListComponents for both the bucket ARN and the instance ARN with a 240 s budget, the predicate is never satisfied, and the harness kills the check: "Check timed out after 180.0s". Because the poll never returns, none of the CreateComponent assertions above are ever issued, so the emulator's EC2-only refusal, its exclusivity refusal, its volume absorption and its delete-then-regroup behavior are all unverified and, on the emulator's current state model (a component map no operation fills), have nothing to operate on.

Why: a client that creates an application and waits for its components before grouping an EC2 instance — the documented onboarding sequence, and the only way to reach CreateComponent/DescribeComponent/DeleteComponent for a discovered resource — does not get a detectable wrong answer, it hangs forever on a 200 with an empty list. Every custom-component behavior of the service is then unreachable, and a client that instead skips the wait and calls CreateComponent directly gets neither the EC2-only nor the already-grouped refusal AWS would give it.

Fix: the component-discovery/onboarding step the application's state owes — CreateApplication's handler plus the shared application state that computes an application's component set from its Resource Groups group (the same group-resolution seam CreateApplication already uses to admit the group), surfaced through the ListComponents and DescribeComponent readers — and, behind it, CreateComponent's validation (EC2-only, exclusive ownership) and the state transitions CreateComponent/DeleteComponent owe to the standalone component set.

Traces: application-insights.custom-component-ec2-only-and-exclusive.

## Evidence
Suite report (0/4 passed; this check mismatched and NOT marked ALREADY FILED or OWNED):

  ### application-insights
  #### [mismatch] application-insights.custom-component-ec2-only-and-exclusive
  Error: Check timed out after 180.0s

Expected vs actual:
  Expected: after s3.create_bucket + put_bucket_tagging, ec2.run_instances with the same tag, resource-groups.create_group with a TAG_FILTERS_1_0 query on that tag, and application-insights.create_application(ResourceGroupName=<group>, AutoConfigEnabled=False), ListComponents(ResourceGroupName=<group>) eventually returns an ApplicationComponentList whose ComponentName set is exactly {"arn:aws:s3:::<bucket>", "arn:aws:ec2:<region>:<account>:instance/<instance-id>"}, each with Monitor=false.
  Actual: ListComponents answers HTTP 200 with an empty ApplicationComponentList on every call, indefinitely. The check's _poll_until(..., timeout_s=240, interval_s=10, description="the bucket and the instance to be auto-discovered as components") never satisfies its predicate and the check is killed by the 180 s harness cap.

Unreachable-behind-the-hang (asserted by the same check, never exercised because the poll never returns):
  CreateComponent(ResourceGroupName=<group>, ComponentName=<custom>, ResourceList=["arn:aws:s3:::<bucket>"]) must fail HTTP 400 ValidationException with Message exactly "Only EC2 Instance can be grouped." and must create nothing.
  CreateComponent over an EC2 instance ARN must succeed; DescribeComponent(ComponentName=<custom>).ResourceList must then contain both the instance ARN and the ARN of the instance's attached EBS volume; ListComponents must no longer report the instance's standalone component.
  A second CreateComponent naming the same instance ARN must fail HTTP 400 ValidationException with Message "Component <instance-arn> is grouped in another component." (fixed text plus the ARN from the request), and DescribeComponent for that refused name must fail ResourceNotFoundException (HTTP 404).
  After DeleteComponent(ComponentName=<custom>), the instance must reappear as its own standalone component and be groupable again.

Note: the failure surfaces as a client hang on HTTP 200 responses, not as an error response, so there is no diverging status/code/message to quote for the discovery cause itself; the statuses, codes and messages quoted above are the ones AWS gives for the CreateComponent refusals the hang prevents the check from reaching.

## Steps to Reproduce
Against the emulator, with s3, ec2, resource-groups and application-insights mounted:

1. s3.create_bucket(Bucket=<bucket>, CreateBucketConfiguration={"LocationConstraint": <region>}); s3.put_bucket_tagging(Bucket=<bucket>, Tagging={"TagSet": [{"Key": <tag>, "Value": "1"}]}).
2. ec2.run_instances(ImageId=<al2023-ami>, InstanceType=<t3.micro-class>, MinCount=1, MaxCount=1, SubnetId=<subnet>, TagSpecifications=[{"ResourceType": "instance", "Tags": [{"Key": <tag>, "Value": "1"}]}, {"ResourceType": "volume", "Tags": [{"Key": <tag>, "Value": "1"}]}]).
3. resource-groups.create_group(Name=<group>, ResourceQuery={"Type": "TAG_FILTERS_1_0", "Query": json.dumps({"ResourceTypeFilters": ["AWS::AllSupported"], "TagFilters": [{"Key": <tag>, "Values": ["1"]}]})}).
4. application-insights.create_application(ResourceGroupName=<group>, AutoConfigEnabled=False).
5. Poll application-insights.list_components(ResourceGroupName=<group>). Expected: ApplicationComponentList eventually has ComponentName set == {"arn:aws:s3:::<bucket>", "arn:aws:ec2:<region>:<account>:instance/<instance-id>"}. Observed: empty ApplicationComponentList on every call (HTTP 200), so any poll never completes and the check dies at the 180 s cap.
6. Once step 5 returns, the rest of the check must hold: create_component with ResourceList=["arn:aws:s3:::<bucket>"] -> 400 ValidationException "Only EC2 Instance can be grouped."; create_component with ResourceList=[<instance-arn>] -> success, describe_component ResourceList contains <instance-arn> and the attached volume's ARN, list_components no longer shows <instance-arn>; a second create_component naming <instance-arn> -> 400 ValidationException "Component <instance-arn> is grouped in another component." and describe_component for that name -> 404 ResourceNotFoundException; delete_component then restores <instance-arn> as a standalone component and a third create_component over it succeeds.

Replay application-insights.custom-component-ec2-only-and-exclusive to confirm.
