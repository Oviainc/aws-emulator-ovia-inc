Issue-Source: trace-sweep-divergence
Issue-Trace: mq-rmq-traj-rabbitmq-configuration-review
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-rmq-traj-rabbitmq-configuration-review mq-rmq-traj-recreate-rabbitmq-broker-refusals
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|UpdateConfiguration|2xx->2xx|$.latestRevision.revision
Issue-Key: mq|DescribeConfigurationRevision|2xx->2xx|$.description
Issue-Key: mq|DescribeBrokerInstanceOptions|2xx->2xx|$.brokerInstanceOptions[*].storageType
Issue-Related: 20261031T013311Z-mq-configuration-revision-reads-diverge-from-recorded-amazon.md
Issue-Commit: 981c4cbbb98c58ce8ac9493ce56831a4fdef61bb

# mq response bodies carry wrong revision state and uppercase instance-option storageType

> _Note: this issue was identified against the code base at commit `981c4cbbb98c58ce8ac9493ce56831a4fdef61bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConfiguration

## Description
THREE causes where an mq call answers with the right status and shape but wrong member VALUES. All three are body/state fidelity in the mq service and must all be fixed before closing. Model references are to services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json.

Cause 1 of 3: UpdateConfiguration reports a latestRevision.revision one higher than Amazon MQ, because the emulator advances the configuration's revision counter for writes Amazon MQ never accepted (fixture mq-rmq-traj-rabbitmq-configuration-review, step task#5).
AWS: Smithy models UpdateConfiguration as PUT /v1/configurations/{ConfigurationId} -> 200 with output member LatestRevision (jsonName "latestRevision", ConfigurationRevision, member Revision). Recorded behavior: revision numbers are consecutive integers starting at 1 and advance ONLY when the service accepts a configuration document; after the configuration's creation revision and one accepted update, the accepted update reports revision 2. The recorded 2 is this fixture's position in that sequence, not a constant.
Emulator: reports 3 for the same write. Replay: $.latestRevision.revision: value_mismatch (expected 2, got 3). The extra revision is the one the emulator minted for the preceding malformed-document update that AWS refused with 400 (filed as Cause 4 of the mq request-rejection bundle in this same run) - but the invariant to restore here is the counter itself: only an accepted write may consume a revision number.
Why: revision numbers are the handles clients use to read, diff and roll back configurations, and they are also what CreateBroker/UpdateBroker reference via ConfigurationId.Revision. A client that records the revision returned by its write and later reads or pins it is off by one against AWS for the rest of the configuration's life.
Fix: the configuration-revision state/write path - allocate the next revision number only when a configuration document is accepted, and report that number in UpdateConfiguration's latestRevision.

Cause 2 of 3: DescribeConfigurationRevision returns the configuration's initial document and an empty description instead of the data and description recorded by the update that produced that revision (fixture mq-rmq-traj-rabbitmq-configuration-review, step task#6).
AWS: Smithy models DescribeConfigurationRevision as GET /v1/configurations/{ConfigurationId}/revisions/{ConfigurationRevision} -> 200 with Data (jsonName "data") and Description (jsonName "description"); UpdateConfiguration's input carries Data and Description. Recorded behavior: reading revision N returns the document and description supplied by the write that created revision N - here the base64 of the RabbitMQ (cuttlefish) document 'consumer_timeout = 3600000' and the description 'raise consumer timeout' the update sent. Those literals are this fixture's request values; the requirement is that the read reflects the creating write.
Emulator: returns base64 of a default ActiveMQ document ('<?xml version="1.0"?><broker xmlns="http://activemq.apache.org/schema/core"/>') and an empty description. Replay: $.data: value_mismatch; $.description: value_mismatch (expected 'raise consumer timeout', got ''). Hypothesis: the revision the emulator serves for this number is the placeholder it created at CreateConfiguration time (compounded by the off-by-one of Cause 1), and Description is never persisted onto the revision record at all - the empty description is independent of the numbering.
Why: a client that writes a configuration and reads the revision back gets neither its document nor its description; config-as-code reconciliation, drift detection and revision diffing all see an engine-mismatched placeholder (an ActiveMQ XML body on a RabbitMQ configuration), and the description a human wrote to label the change is lost.
Fix: the configuration-revision state plus DescribeConfigurationRevision's handler - persist the accepted Data and Description on the revision record keyed by its revision number, and serve those on read. Related but distinct: 20261031T013311Z-mq-configuration-revision-reads-diverge-from-recorded-amazon.md covers canonicalization of the stored document and ListConfigurationRevisions' nextToken; it does not cover serving the wrong revision's data or the dropped description.

Cause 3 of 3: DescribeBrokerInstanceOptions serves brokerInstanceOptions[].storageType in upper case where Amazon MQ serves it lower case (fixture mq-rmq-traj-recreate-rabbitmq-broker-refusals, step task#3).
AWS: Smithy models BrokerInstanceOption.StorageType (jsonName "storageType") as the BrokerStorageType enum whose values are EBS and EFS, but recorded behavior for this listing shows Amazon MQ emits the lower-case spelling ('ebs', likewise 'efs') in DescribeBrokerInstanceOptions' response - the wire value, not the enum name, is the requirement here.
Emulator: emits 'EBS'. Replay: $.brokerInstanceOptions[0].storageType: value_mismatch (expected 'ebs', got 'EBS').
Why: a client that compares the listed storageType against a value it got from this same listing, uses it as a map key, or round-trips it is case-sensitive; an SDK with a strict enum deserializer treats the upper-case spelling as a different (unknown-vs-known) value than AWS returns, so instance-option filtering silently matches nothing.
Fix: DescribeBrokerInstanceOptions' serializer for the instance-option catalog - emit storageType with the casing AWS uses for this member. Note 20261030T033328Z-mq-describebrokerinstanceoptions-serves-a-hand-built-instance-option.md owns the catalog CONTENT divergence (the hand-built option list) and the replay marked that cluster owned; this casing cluster is reported with no owner and is not covered by it.

All three clusters came back with owned_by empty and stub_path empty.

## Evidence
Replay summary: trace_set simparity-wire-aws-v1, emulator_up true, 8 fixtures, 50 steps, 27 passed, 23 mismatched, score 54.0%, 16 divergence clusters.

Cause 1 - mq-rmq-traj-rabbitmq-configuration-review#task#5, UpdateConfiguration, expected 200 / actual 200:
  $.latestRevision.revision: value_mismatch (expected 2, got 3)

Cause 2 - mq-rmq-traj-rabbitmq-configuration-review#task#6, DescribeConfigurationRevision, expected 200 / actual 200:
  $.data: value_mismatch (expected 'Y29uc3VtZXJfdGltZW91dCA9IDM2MDAwMDAK' = 'consumer_timeout = 3600000\n', got 'PD94bWwgdmVyc2lvbj0iMS4wIj8+PGJyb2tlciB4bWxucz0iaHR0cDovL2FjdGl2ZW1xLmFwYWNoZS5vcmcvc2NoZW1hL2NvcmUiLz4K' = '<?xml version="1.0"?><broker xmlns="http://activemq.apache.org/schema/core"/>\n')
  $.description: value_mismatch (expected 'raise consumer timeout', got '')
  The document text and description are this fixture's request values; the requirement is that the revision read returns what the creating write supplied.

Cause 3 - mq-rmq-traj-recreate-rabbitmq-broker-refusals#task#3, DescribeBrokerInstanceOptions, expected 200 / actual 200:
  $.brokerInstanceOptions[0].storageType: value_mismatch (expected 'ebs', got 'EBS')

## Steps to Reproduce
1. git clone the repo, boot the emulator built from that checkout, and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1 - fixture mq-rmq-traj-rabbitmq-configuration-review step task#5: create a RabbitMQ configuration, submit an update whose document the engine cannot parse (AWS refuses it with 400), then submit a valid update. Read latestRevision.revision on the valid update's 200: AWS reports 2, the emulator reports 3 because it consumed a revision for the refused write.
3. Cause 2 - same fixture step task#6: GET /v1/configurations/{id}/revisions/{revision} for the revision the accepted update reported. Base64-decode `data` and read `description`: AWS returns the cuttlefish document and description the update sent; the emulator returns a default ActiveMQ XML body and an empty description.
4. Cause 3 - fixture mq-rmq-traj-recreate-rabbitmq-broker-refusals step task#3: GET /v1/broker-instance-options and inspect brokerInstanceOptions[].storageType. AWS serves the lower-case spelling ('ebs'/'efs'); the emulator serves 'EBS'.
