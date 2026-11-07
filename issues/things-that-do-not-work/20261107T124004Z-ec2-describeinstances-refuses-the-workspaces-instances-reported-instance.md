Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-instances-billing-configuration-monthly-reflected-in-get
Issue-Step: teardown#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-instances-billing-configuration-monthly-reflected-in-get
Issue-Shape-Id: com.amazonaws.workspacesinstances#EUCMIFrontendAPIService
Issue-Key: ec2|DescribeInstances|2xx->4xx|$
Issue-Key: ec2|DescribeInstances|2xx->4xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.instanceState.name
Issue-Related: 20261103T071133Z-ec2-describeinstances-and-launch-template-projections-omit-modeled.md
Issue-Commit: 0465eb394723049460d943d66b3411d762c72467

# EC2 DescribeInstances refuses the WorkSpaces-Instances-reported instance id as InvalidInstanceID.Malformed, so a torn-down WorkSpace Instance never reads back terminated

> _Note: this issue was identified against the code base at commit `0465eb394723049460d943d66b3411d762c72467`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
One root cause, on the ec2 endpoint, reached through the WorkSpaces Instances teardown flow.

Cause 1 of 1: ec2:DescribeInstances(InstanceIds=[<the EC2ManagedInstance.InstanceId a WorkSpace Instance reports>]) is refused 400 InvalidInstanceID.Malformed instead of resolving the backing instance and reporting its terminated state.

AWS: Smithy: DescribeInstances (service com.amazonaws.ec2#AmazonEC2, ec2Query). Input member InstanceIds targets InstanceIdStringList (xmlName "InstanceId"); output member Reservations targets ReservationList (xmlName "reservationSet", so the wire path is DescribeInstancesResponse.reservationSet.item.instancesSet.item.instanceState.name). errors=[] — EC2 declares no modeled error shapes, every failure is wrapped in the ec2Query envelope Response.Errors.Error with Code/Message, so the requirement for the refusal class and wording is the recorded AWS behavior. The operation's own modeled documentation states the two relevant rules: "If you specify an instance ID that is not valid, an error is returned" and "Recently terminated instances might appear in the returned results. This interval is usually less than one hour." A WorkSpace Instance's managed instance is launched in the caller's own account (workspacesinstances ManagedInstanceRequest mirrors RunInstances input and GetWorkspaceInstanceResponse$EC2ManagedInstance$InstanceId is documented as "Unique identifier of the managed EC2 instance"), so the id it reports is an EC2 instance id of that account: recorded behavior is HTTP 200 with a reservation holding that instance, and after the WorkSpace Instance is deleted the instance's instanceState.name reads "terminated".

Emulator: the same request answers HTTP 400 with Response.Errors.Error.Code "InvalidInstanceID.Malformed" and Response.Errors.Error.Message 'Invalid id: "<instance-id>"'. In that message the fixed text is `Invalid id: "` and the closing `"`; the only variable part is the instance id taken verbatim from the request's InstanceIds member. Because the call is refused, the response carries no reservationSet at all, so the step's poll for instanceState.name in ['terminated'] never saw any value (7 attempts, capped at 60s) and the teardown never reaches a verdict. Two things are wrong at once: (a) no EC2 instance is ever materialised in EC2's state for a WorkSpace Instance, so there is nothing for DescribeInstances to resolve and nothing for the delete to terminate; and (b) the id is inside the form EC2 issues — `i-` plus 17 lowercase hexadecimal digits with a leading `0` — so "malformed" is the wrong refusal class even for an instance that is genuinely absent. EC2's own reader agrees the form is valid (instance_id_is_well_formed); the Malformed verdict comes from the extra clause that treats a long-form identifier the service's generic id counter has already passed as naming another kind of resource. For a well-formed id the account holds no instance for, AWS's answer is InvalidInstanceID.NotFound ("The instance ID '<id>' does not exist"), not Malformed — so a client cannot even reach a NotFound here.

Why: the only reason the WorkSpaces Instances API hands back an EC2 instance id is so the caller can inspect and manage the underlying instance. A client that polls ec2:DescribeInstances to confirm a deprovision completed — the standard teardown assertion — gets a hard 400 whose code says its own id generation is broken, rather than a reservation it can read instanceState.name from. It therefore cannot distinguish "still terminating" from "gone", retries forever or aborts the teardown, and every cross-service flow on that id (describe, tag, stop, attach, terminate) is impossible. Misclassifying the refusal compounds it: a caller that handles InvalidInstanceID.NotFound as "already gone" takes the error path instead.

Fix: two components. In the WorkSpaces Instances launch/teardown path, the handler and state that mint the reported id must create the instance in the EC2 service's state from the supplied ManagedInstance and terminate it when the WorkSpace Instance is deleted, instead of fabricating an `i-` id locally. In the EC2 DescribeInstances path, the instance-id resolution step that precedes the lookup must not classify a long-form id that is inside the issued form as malformed on the strength of the generic id counter; an id EC2 can read but holds no instance for belongs in the not-found half.

Related open issues (neither carries this fixture's replay key, so neither covers this step): 20261107T120211Z-workspaces-instances-createworkspaceinstance-3-divergences-wrong-conflictexception-message-for-a.md cause 3 describes the same non-propagation and the same Malformed refusal but is a BEHAVIORAL finding filed under workspaces-instances, and 20261024T122725Z-workspaces-instances-createworkspaceinstance-event-propagation.md describes the non-propagation while predicting an InvalidInstanceID.NotFound refusal.

Traces: workspaces-instances-billing-configuration-monthly-reflected-in-get#teardown#8.

## Evidence
Wire replay of trace set simparity-wire-aws-v1, fixture workspaces-instances-billing-configuration-monthly-reflected-in-get, step teardown#8, service ec2, operation DescribeInstances, verdict mismatch, no owning issue and no stub.

Expected (recorded AWS): HTTP 200; $.DescribeInstancesResponse.reservationSet.item.instancesSet.item.instanceState.name polls to "terminated".

Actual (emulator): HTTP 200 -> 400. Body is the ec2Query error envelope:
  $.Response.Errors.Error.Code   = "InvalidInstanceID.Malformed"
  $.Response.Errors.Error.Message = 'Invalid id: "i-00000000000000001"'
The quoted id is the value the request carried in InstanceIds (itself the EC2ManagedInstance.InstanceId the WorkSpace Instance reported); `Invalid id: "` and the trailing `"` are the fixed text. Per the replay detail, $ diverged (expected no error envelope, got the one above) and the instanceState.name poll was "not in ['terminated'] after 7 attempt(s)" with the path absent from the response, the poll capped at 60s.

The id is `i-` + 17 lowercase hexadecimal digits with a leading `0`, i.e. inside the long resource-id form EC2 issues and reads (EC2's own instance_id_is_well_formed accepts it; AWS reserves InvalidInstanceID.Malformed for ids outside the form, documenting "provide the full instance ID in the request, in the form i-xxxxxxxxxxxxxxxxx"). An absent-but-well-formed id is InvalidInstanceID.NotFound at AWS, so the emulator is wrong about the class as well as about the instance's existence.

## Steps to Reproduce
1. Boot the emulator from this checkout so both workspaces-instances and ec2 are mounted, and replay trace set simparity-wire-aws-v1.
2. Replay fixture workspaces-instances-billing-configuration-monthly-reflected-in-get and let it run to step teardown#8 (the representative binding for this issue).
3. That step calls ec2:DescribeInstances with InstanceIds set to the EC2ManagedInstance.InstanceId the WorkSpace Instance reported, and polls $.DescribeInstancesResponse.reservationSet.item.instancesSet.item.instanceState.name for "terminated".
4. Observe HTTP 400 with Response.Errors.Error.Code "InvalidInstanceID.Malformed" and Message 'Invalid id: "<that id>"'; the reservationSet is absent, so the poll exhausts its attempts.
5. Equivalent manual reproduction: create a WorkSpace Instance with a MONTHLY billing configuration, poll GetWorkspaceInstance until ProvisionState is ALLOCATED, read EC2ManagedInstance.InstanceId, call ec2:DescribeInstances(InstanceIds=[that id]) — it is refused instead of resolving — then DeleteWorkspaceInstance and call it again; nothing was ever launched, so nothing is reported terminated.
6. The step is closed only when ec2:DescribeInstances on that id answers HTTP 200 with a reservation holding the instance, and that instance's instanceState.name reaches "terminated" after the WorkSpace Instance is deleted.
