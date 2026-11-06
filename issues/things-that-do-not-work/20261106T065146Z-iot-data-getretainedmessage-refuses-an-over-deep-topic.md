Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-data.publish-topic-depth-limit-rejected
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data.publish-topic-depth-limit-rejected|run#17|AssertionError|assert absent == "ResourceNotFoundException", (
Issue-Commit: 502370b3ab52e8f9258a465387106a9118ec6835

# iot-data GetRetainedMessage refuses an over-deep topic with 400 InvalidRequestException instead of answering 404 ResourceNotFoundException for a topic that holds no retained message

> _Note: this issue was identified against the code base at commit `502370b3ab52e8f9258a465387106a9118ec6835`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRetainedMessage

## Description
One net-new root cause in the iot-data (IoT Data Plane) retained-message READ path.

Cause 1 of 1: GetRetainedMessage applies the PUBLISH-side topic-name ceilings to the read request and refuses it, instead of reporting that the topic holds no retained message.

AWS: Smithy models GetRetainedMessage with input `topic` (shape com.amazonaws.iotdataplane#Topic, @httpLabel @required) and declares NO length, pattern or depth constraint on `Topic`; the operation models both InvalidRequestException (HTTP 400, "The request is not valid.") and ResourceNotFoundException (HTTP 404, "The specified resource does not exist."). The model therefore says nothing about a topic-depth rule on this operation, so for the depth behavior this is recorded behavior only: the recording shows that a Publish to a topic with more than 8 `/`-separated levels is refused 400 InvalidRequestException with the message "Topic can't contain more than 8 parts" (fixed text; `8` is the broker's published part limit, not a per-run value), and that a subsequent GetRetainedMessage for that SAME over-deep topic is answered with the operation's 404 ResourceNotFoundException — i.e. the read path reports the topic as holding no retained message rather than re-validating the name. Its message is the service's fixed retained-message not-found text; the variable part of a GetRetainedMessage refusal is only the topic taken from the request, and nothing in the recorded 404 echoes the caller's value. (Hypothesis: the data plane resolves the retained-message key before weighing it against the broker's publish-time topic-name ceilings, so a name it cannot have stored anything under is simply absent.)

Emulator: GetRetainedMessage runs `crate::state::topic_name_refusal(&input.topic)` BEFORE consulting the store and returns 400 InvalidRequestException with "Topic can't contain more than 8 parts" for any topic over the part ceiling, so the check that asserts the refused publish stored nothing observes error code `InvalidRequestException` where AWS answers `ResourceNotFoundException`. The same pre-store guard also refuses a topic longer than 256 bytes (message "Topic can't contain more than 256 bytes") and a topic carrying an MQTT wildcard ("Invalid topic filter") on this read path; the depth case is the one the failing check exercises, and it is the identical code path.

Why: 400-vs-404 is the whole signal a client branches on. A client (or a test asserting an effect is absent) reads "no retained message exists here" from 404 ResourceNotFoundException and "your request was malformed, never retry it as-is" from 400 InvalidRequestException. Against the emulator, probing a topic for a retained message that was never stored raises a request-malformed error instead, SDK error-type matching on ResourceNotFoundException never fires, and an absence check cannot distinguish "nothing was stored" from "the read was rejected".

Fix: the read path's topic handling in the GetRetainedMessage handler — services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/get_retained_message.rs:171-182 — must not borrow the publish-time topic-name refusal; the store lookup (same file, 184-191) should decide the answer, so a topic the store holds nothing for yields the operation's modelled ResourceNotFoundException with the service's fixed not-found text from that crate's src/state.rs catalog (src/state.rs:201). `crate::state::topic_name_refusal` (src/state.rs:182-198) stays the publish-path rule.

Related but not covering: the open issue 20261029T224815Z-iot-data-validation-refusals-come-from-the-generic.md concerns clientId/thingName/shadowName constraint refusals coming from the generic Smithy validator; it does not touch GetRetainedMessage or the topic ceilings, so this cause is not covered by it.

Traces: behavioral check iot-data.publish-topic-depth-limit-rejected.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), service heading `### iot-data`, check [mismatch] iot-data.publish-topic-depth-limit-rejected:

  File "check_iot_data_publish_topic_depth_limit_rejected.py", line 105, in run
    assert absent == "ResourceNotFoundException", (
  AssertionError: the refused publish must not have stored a retained message at
  'sim2real-behav-iotdata-depth-<id>/a/b/c/d/e/f/g/h', got 'InvalidRequestException'

Sequence the check already got right before this point (so these are NOT the divergence):
  - Publish(topic=<base>/a/b/c/d/e/f/g, retain=true, qos=1) accepted; GetRetainedMessage returns it (8 levels is legal).
  - Publish(topic=<base>/a/b/c/d/e/f/g/h, retain=true, qos=1) refused 400 InvalidRequestException, message "Topic can't contain more than 8 parts" — matches AWS.

The divergence:
  GetRetainedMessage(topic=<base>/a/b/c/d/e/f/g/h)
    expected: HTTP 404, Error.Code ResourceNotFoundException (the service's fixed retained-message not-found message)
    actual:   HTTP 400, Error.Code InvalidRequestException, message "Topic can't contain more than 8 parts"

The topic prefix 'sim2real-behav-iotdata-depth-<id>' is this run's generated name and is not part of the requirement; the requirement is the 9-level depth and the resulting status/error class. Emulator source confirming the cause: services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/get_retained_message.rs:177-182 returns InvalidRequestException from crate::state::topic_name_refusal before the store lookup at line 187.

## Steps to Reproduce
1. Pick a topic with 9 `/`-separated levels, e.g. T = "<unique>/a/b/c/d/e/f/g/h".
2. iot-data:Publish(topic=T, payload=b"x", retain=true, qos=1) -> expect HTTP 400 InvalidRequestException, message "Topic can't contain more than 8 parts" (emulator already matches).
3. iot-data:GetRetainedMessage(topic=T) -> AWS answers HTTP 404 with Error.Code ResourceNotFoundException (nothing was ever retained for T). Observe the emulator answers HTTP 400 with Error.Code InvalidRequestException and message "Topic can't contain more than 8 parts".
4. Same with a topic of legal depth but over 256 bytes, e.g. "<unique>/" + "z"*256: GetRetainedMessage on it takes the identical pre-store refusal path (message "Topic can't contain more than 256 bytes") instead of a not-found answer.
