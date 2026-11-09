Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.traffic-policy-metadata-and-comment-update route53.traffic-policy-version-lifecycle
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.traffic-policy-metadata-and-comment-update|run#21|AssertionError|assert err.get("Message") == "The policy does not exist.", err
Issue-Key: route53.traffic-policy-version-lifecycle|run#27|AssertionError|assert err.get("Message") == "The policy does not exist.", err
Issue-Commit: 681c355fd2a263b0dd1e310a79fbd88918464860

# Route 53 NoSuchTrafficPolicy is answered with an empty Message: AWS sends 'The policy does not exist.'

> _Note: this issue was identified against the code base at commit `681c355fd2a263b0dd1e310a79fbd88918464860`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrafficPolicy

## Description
One cause, observed on two operations that refuse a traffic policy version the store does not hold.

Cause 1 of 1 — the modeled `NoSuchTrafficPolicy` refusal carries no Message, where Route 53 sends a fixed sentence.

AWS: Smithy: `NoSuchTrafficPolicy` is a client error with httpError 404, bound to GetTrafficPolicy, UpdateTrafficPolicyComment, CreateTrafficPolicyVersion, DeleteTrafficPolicy and ListTrafficPolicyVersions; its single member is `message` (ErrorMessage), and the shape doc is "No traffic policy exists with the specified ID." (the RULE, not the wire sentence). Recorded behavior: when a request names an (Id, Version) pair that does not exist — a version never created, or a version a previous DeleteTrafficPolicy removed — Route 53 answers 400/`NoSuchTrafficPolicy` with Error.Message = "The policy does not exist.". That message is entirely FIXED text: it has no variable part at all — it never echoes the policy id, the version, or the operation. Both failing checks assert exactly that string after already matching Type=Sender and Code=NoSuchTrafficPolicy.

Emulator: the status and the error code match, but `Error.Message` is the empty string: the replayed botocore Error dict is `{'Message': '', 'Code': 'NoSuchTrafficPolicy', 'Type': 'Sender'}`. The refusal is minted messageless on purpose — `no_such_traffic_policy()` in the service's state module constructs `NoSuchTrafficPolicy { message: None }` and documents the omission as deliberate ("MESSAGELESS … the `Error` element carries `Type` and `Code` and no `Message` child"), so the serializer emits no Message element and botocore normalizes the absent child to ''. That premise is wrong for this error class: Route 53 does send a Message here.

Why: the Message is the only human-readable part of the refusal. A client that logs or surfaces `Error.Message` shows an empty reason for a 404 that AWS explains, and any caller (or test) that branches on, asserts, or string-matches "The policy does not exist." sees a mismatch even though the code is right — so not-found handling that works against AWS fails against the emulator.

Fix: the shared refusal constructor for this error class — `no_such_traffic_policy()` in services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs must populate `message` with AWS's fixed sentence, so every operation that raises `NoSuchTrafficPolicy` (GetTrafficPolicy and UpdateTrafficPolicyComment are the two exercised here) words it identically. Note the sibling `no_such_traffic_policy_instance()` is messageless on the same stated premise and should be re-checked against a recording, but only the traffic-policy path is grounded by these checks. Related but NOT covering this: open issue 20261029T220729Z-route53-route-53-refusal-messages-are-emulator-authored-instead covers CreateTrafficPolicy's InvalidTrafficPolicyDocument clause wording, a different error shape.

Traces: route53.traffic-policy-metadata-and-comment-update (GetTrafficPolicy with Version = created version + 99), route53.traffic-policy-version-lifecycle (UpdateTrafficPolicyComment on a version just deleted by DeleteTrafficPolicy).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, 2 mismatches, both after Code/Type already matched:

1. route53.traffic-policy-metadata-and-comment-update — check line 58:
   assert err.get("Message") == "The policy does not exist."
   AssertionError: {'Message': '', 'Code': 'NoSuchTrafficPolicy', 'Type': 'Sender'}
   Path: CreateTrafficPolicy(Name, Document) -> GetTrafficPolicy(Id, Version=version+99).

2. route53.traffic-policy-version-lifecycle — check line 78:
   assert err.get("Message") == "The policy does not exist."
   AssertionError: {'Message': '', 'Code': 'NoSuchTrafficPolicy', 'Type': 'Sender'}
   Path: CreateTrafficPolicy -> CreateTrafficPolicyVersion -> DeleteTrafficPolicy(v2) -> UpdateTrafficPolicyComment(Id, Version=v2).

Expected (AWS): HTTP 400, Error.Type=Sender, Error.Code=NoSuchTrafficPolicy, Error.Message="The policy does not exist." (fixed text, no variable parts).
Actual (emulator): HTTP 400, Error.Type=Sender, Error.Code=NoSuchTrafficPolicy, no Message element at all (botocore reports '').

Source of the omission: services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs, `pub fn no_such_traffic_policy() -> NoSuchTrafficPolicy { NoSuchTrafficPolicy { message: None } }` (~line 2886), reached from services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/get_traffic_policy.rs:162.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the behavioral checks route53.traffic-policy-metadata-and-comment-update and route53.traffic-policy-version-lifecycle.
3. By hand, with a route53 client pointed at the emulator:
   a. create_traffic_policy(Name=<unique>, Document='{"AWSPolicyFormatVersion":"2015-10-01","RecordType":"A","Endpoints":{"e1":{"Type":"value","Value":"192.0.2.1"}},"StartEndpoint":"e1"}') and note Id and Version.
   b. get_traffic_policy(Id=<id>, Version=<version>+99) and inspect the ClientError's response['Error'].
   c. create_traffic_policy_version(Id=<id>, Document=<any valid doc>), delete_traffic_policy(Id=<id>, Version=<v2>), then update_traffic_policy_comment(Id=<id>, Version=<v2>, Comment='x') and inspect response['Error'].
4. In both (b) and (c) the Error carries Code=NoSuchTrafficPolicy with Message='' instead of AWS's 'The policy does not exist.'.
