Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection
Issue-Step: setup#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: ec2|RunInstances|2xx->4xx|status
Issue-Commit: beb9379647f6ddd981e0914c2206ccd791a0c1ce

# EC2 RunInstances refuses the recorded ELB-Classic setup launch with HTTP 400 where AWS answers 200

> _Note: this issue was identified against the code base at commit `beb9379647f6ddd981e0914c2206ccd791a0c1ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
One cause. Cause 1 of 1: a RunInstances launch that AWS accepts is refused by the emulator with an ec2Query client error (fixture elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection, step setup#5).

AWS: RunInstances launches the requested instances and answers HTTP 200 with a Reservation (RunInstancesOutput: ReservationId, OwnerId, Groups, Instances, each Instance starting in state 'pending'). Smithy: com.amazonaws.ec2#RunInstances declares errors: [] — EC2 models no error shapes at all, so on the wire the ec2Query <Errors><Error><Code>/<Message> pair IS the whole refusal contract, and whether the call is refused at all is part of the contract. The model documents no precondition that the recorded launch violates: 'If you don't specify a subnet ID, we choose a default subnet from your default VPC for you', 'If you don't specify a security group ID, we use the default security group for the VPC', and ImageId is documented only as 'The ID of the AMI. An AMI ID is required to launch an instance and must be specified here or in a launch template.' Recorded behavior only for the specific launch: the recording shows this setup call answering 200 with a reservation, which is why the fixture's later ELB-Classic registration/health steps have an instance to register at all. The general requirement: a launch whose parameters AWS accepts must be answered 200 with a reservation, and only a parameter EC2 itself rejects may produce an ec2Query Error — the code and message then being EC2's own for that class.

Emulator: the step is answered HTTP 400 instead of 200 (replay detail: 'status: expected 200, got 400'), so no reservation and no instance is minted. The replay record for this cluster reports the status divergence only ('keys=status'), so the emitted Code/Message text is not carried in the divergence; the handler reaches one of its own pre-store refusal gates and returns a client error before any instance record is written. Hypothesis (not stated by the model or the recording): the refusal is the ImageId form gate in services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs — `if !issued_image_id_is_well_formed(&image_id) { return Err(invalid_ami_id_malformed_expecting_ami(&image_id)) }` — which runs a literal 'ami-' prefix test over the ImageId before anything is resolved or looked up; other 400 gates on the same pre-store path (InvalidParameterValue on minCount/maxCount/privateIpAddress/userData/tagSpecifications/securityGroups, InvalidParameterCombination, InstanceLimitExceeded, the availability_zone_of_region check) are the alternative candidates. Whichever gate fires, the correct behavior for this request is a 200 reservation.

Why: a client relying on AWS's behavior cannot launch the instance at all, and because the refusal is a 4xx client error it tells the caller its own request is invalid, so no retry, backoff or repair path recovers. Every downstream workflow that needs a real instance id — here registering instances with a classic load balancer and then polling their health — is built on a value that never exists, so the whole scenario is unreachable through the emulator even though the operation is implemented.

Fix: the pre-store input validation at the head of the RunInstances handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs) — the gate that fires for this recorded launch must not refuse a request EC2 accepts; resolve or default the member it is gating on (AMI reference resolution before any id-form test, zone/subnet/security-group defaulting) and keep the ec2Query refusal for inputs EC2 itself rejects.

Related open issue that does NOT cover this step: 20261029T230911Z-ec2-runinstances-refuses-the-resolve-ssm-ami-reference.md is a BEHAVIORAL issue keyed on ec2|RunInstances|InvalidAMIID.Malformed and grounded in the check ec2.asgeni-secondary-eni-cross-az-attach-rejected; it carries no replay key for this wire fixture step, and the replay reports this cluster as unowned.

Traces: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection#setup#5.

## Evidence
Replay cluster (verdict mismatch, service ec2, operation RunInstances): expected_status 200, actual_status 400, detail 'status: expected 200, got 400', divergence_key 'service=ec2|RunInstances|status=200->400|keys=status', affected ['elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection#setup#5'].
Expected (AWS, recorded): HTTP 200 with a RunInstancesOutput reservation (ReservationId, OwnerId, Instances with a pending instance) so the fixture's later ELB-Classic registration and DescribeInstanceHealth steps have a real instance id.
Actual (emulator): HTTP 400 — an ec2Query <ErrorResponse>/<Errors><Error> client refusal; no reservation, no instance minted. The divergence carries the status only, not the Code/Message text.
Model evidence: com.amazonaws.ec2#RunInstances errors: [] (EC2 models no error shapes; code+message on the wire are the entire refusal contract). RunInstancesRequest$ImageId documents only that an AMI id is required. Code evidence for the candidate gate: services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs — ImageId well-formedness test calling invalid_ami_id_malformed_expecting_ami, followed by the InvalidParameterValue / InvalidParameterCombination / InstanceLimitExceeded gates, all before the store is touched.

## Steps to Reproduce
1. Replay fixture elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection against the emulator and stop at step setup#5 (the ec2:RunInstances call that provisions the instance the fixture later registers with the classic load balancer).
2. Expected: HTTP 200 with a reservation whose instance is in state 'pending'. Actual: HTTP 400 with an ec2Query Error, so no instance exists.
3. Inspect the returned <Code>/<Message> to identify which pre-store gate in the RunInstances handler fired, then re-replay setup#5 after that gate accepts the recorded parameters; the step must answer 200 and the downstream registration/health steps of the same fixture must then run against a real instance id.
