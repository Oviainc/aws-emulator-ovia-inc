Issue-Source: trace-sweep-divergence
Issue-Trace: mq-rmq-traj-broker-name-where-the-id-belongs
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-rmq-traj-broker-name-where-the-id-belongs mq-rmq-traj-rabbitmq-configuration-review mq-rmq-traj-recreate-rabbitmq-broker-refusals
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|DescribeBroker|4xx->4xx|$.message
Issue-Key: mq|ListBrokers|4xx->2xx|$.errorAttribute
Issue-Key: mq|ListBrokers|4xx->2xx|$.message
Issue-Key: mq|ListBrokers|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mq|ListBrokers|4xx->2xx|status
Issue-Key: mq|ListBrokers|4xx->4xx|$.message
Issue-Key: mq|UpdateConfiguration|4xx->2xx|$.errorAttribute
Issue-Key: mq|UpdateConfiguration|4xx->2xx|$.message
Issue-Key: mq|UpdateConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mq|UpdateConfiguration|4xx->2xx|status
Issue-Key: mq|DescribeConfigurationRevision|4xx->2xx|$.errorAttribute
Issue-Key: mq|DescribeConfigurationRevision|4xx->2xx|$.message
Issue-Key: mq|DescribeConfigurationRevision|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mq|DescribeConfigurationRevision|4xx->2xx|status
Issue-Key: mq|CreateBroker|4xx->2xx|$.errorAttribute
Issue-Key: mq|CreateBroker|4xx->2xx|$.message
Issue-Key: mq|CreateBroker|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mq|CreateBroker|4xx->2xx|status
Issue-Key: mq|CreateBroker|4xx->4xx|$.errorAttribute
Issue-Key: mq|CreateBroker|4xx->4xx|$.message
Issue-Key: mq|CreateBroker|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mq|CreateBroker|4xx->4xx|status
Issue-Commit: 981c4cbbb98c58ce8ac9493ce56831a4fdef61bb

# mq refuses invalid requests with the wrong status, error and message, or not at all

> _Note: this issue was identified against the code base at commit `981c4cbbb98c58ce8ac9493ce56831a4fdef61bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBroker

## Description
EIGHT causes in the mq request-rejection path: requests Amazon MQ refuses are answered 200 by the emulator, and the refusals it does produce carry the emulator's own wording/attribute instead of Amazon MQ's. All eight are in the same family (per-operation input validation and the BadRequestException/NotFoundException payload it emits) and must all be fixed before closing. Model references below are to services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json. Amazon MQ's error payloads are BadRequestException/NotFoundException/ConflictException, each modelled with members message (jsonName "message"), errorAttribute (jsonName "errorAttribute") and the x-amzn-errortype header naming the shape; message text and errorAttribute are therefore part of the contract, not cosmetic.

Cause 1 of 8: DescribeBroker's NotFoundException message percent-DECODES the broker-id path segment, where Amazon MQ echoes the segment exactly as it arrived on the wire (fixture mq-rmq-traj-broker-name-where-the-id-belongs, step task#2).
AWS: Smithy models DescribeBroker as GET /v1/brokers/{BrokerId} with BrokerId an httpLabel, errors NotFoundException (404). Recorded behavior for the text: status 404, NotFoundException, message with the fixed text "Cannot find broker ID [" + <the BrokerId path segment verbatim, still percent-encoded as the client sent it> + "]. Verify that your broker exists or check broker permissions." In the recording the caller passed an ARN where the broker id belongs, so the SDK percent-encoded the colons into the label and AWS reflected that encoded segment back unchanged (recorded: 'arn%3Aaws%3Amq%3A...%3Abroker%3A<name>%3Ab-<uuid>'). The ARN/name/uuid are values from the request, not requirements.
Emulator: answers 404 NotFoundException with the same template but inserts the DECODED label ('arn:aws:mq:...:broker:<name>:b-<uuid>'), i.e. the colons are restored. Replay: $.message: value_mismatch, expected ...%3Aaws%3Amq%3A..., got ...:aws:mq:....
Why: a client (or a test) matching the error text against the identifier it actually sent, or parsing the bracketed segment back out of the message, sees a different id than AWS reports; tooling that re-URL-decodes the message double-decodes it.
Fix: DescribeBroker's not-found error construction - format the message from the RAW path segment before URI decoding, not from the decoded label.

Cause 2 of 8: ListBrokers ACCEPTS maxResults below Amazon MQ's floor of 5 instead of rejecting it (fixture mq-rmq-traj-broker-name-where-the-id-belongs, step task#3).
AWS: Smithy models ListBrokers as GET /v1/brokers with MaxResults (httpQuery maxResults, @range min 1 max 100) and error BadRequestException (400). The recording shows Amazon MQ enforces a tighter floor than the modelled range: maxResults=4 is refused with 400 BadRequestException, errorAttribute "maxResults", message "Invalid max results value [4]. Number of broker instances should be between [5] and [100]." The same floor appears in the model elsewhere (DescribeBrokerInstanceOptions' output MaxResults targets __integerMin5Max100, @range 5..100). Fixed text: "Invalid max results value [" + <the rejected value from the request> + "]. Number of broker instances should be between [5] and [100]."; only the rejected value varies.
Emulator: returns 200 with a broker listing. Replay: status expected 400, got 200; $.message, $.errorAttribute and the x-amzn-errortype header all absent.
Why: a client probing the paging floor (or a test asserting the rejection) succeeds against the emulator and fails against AWS, and pagination code calibrated on the emulator issues page sizes AWS will not serve.
Fix: ListBrokers' input validation - enforce the 5..100 maxResults window and emit BadRequestException with the message/errorAttribute above.

Cause 3 of 8: ListBrokers' maxResults-too-large rejection uses the emulator's own wording and the wrong limits (fixture mq-rmq-traj-broker-name-where-the-id-belongs, step task#4).
AWS: as Cause 2 - 400 BadRequestException, errorAttribute "maxResults", message "Invalid max results value [101]. Number of broker instances should be between [5] and [100]." (fixed text as quoted; [101] is the request's value).
Emulator: right status (400) and right errorAttribute, but message "maxResults must be between 1 and 100, got 101" - different wording and it advertises a floor of 1 rather than 5.
Why: any client surfacing or matching the service message gets text no AWS SDK/console would ever produce, and the stated valid window is wrong, so a client that retries with the advertised minimum (1) is refused by AWS.
Fix: ListBrokers' validation error serializer - one shared message template for both bounds.

Cause 4 of 8: UpdateConfiguration ACCEPTS a configuration document that does not parse for the configuration's engine instead of refusing it (fixture mq-rmq-traj-rabbitmq-configuration-review, step task#4).
AWS: Smithy models UpdateConfiguration as PUT /v1/configurations/{ConfigurationId} with required Data (jsonName "data") and errors BadRequestException (400) / NotFoundException (404); output member Warnings (__listOfSanitizationWarning) shows the service parses and sanitizes the submitted document rather than storing it opaquely. Recorded behavior: a document whose base64 decodes to text the engine's parser rejects is refused with 400 BadRequestException, errorAttribute "data", message "The specified configuration data could not be parsed for reason [invalid token at line 1]. Check your syntax." Fixed text: "The specified configuration data could not be parsed for reason [" + <the parser's reason, e.g. invalid token at line N> + "]. Check your syntax."; only the bracketed reason varies with the input.
Emulator: answers 200 and records a new revision. Replay: status expected 400, got 200; message/errorAttribute/x-amzn-errortype absent. The emulator validates only that Data is non-empty and base64-decodable (no engine-level parse).
Why: a client that relies on the service to reject a malformed configuration stores garbage, and the revision it then reads back does not exist on AWS at all - this cause is also what shifts the revision numbering reported in the companion issue on revision bodies (see the mq revision/instance-option bundle filed in this same run).
Fix: UpdateConfiguration's validation - parse the decoded document against the configuration's engine type (ActiveMQ XML / RabbitMQ cuttlefish) before accepting it, and emit BadRequestException with the template above.

Cause 5 of 8: DescribeConfigurationRevision answers 200 for a revision number that does not exist instead of 404 (fixture mq-rmq-traj-rabbitmq-configuration-review, step task#7).
AWS: Smithy models DescribeConfigurationRevision as GET /v1/configurations/{ConfigurationId}/revisions/{ConfigurationRevision}, errors include NotFoundException (404). Recorded behavior: a revision beyond the configuration's latest is refused with 404 NotFoundException, errorAttribute "configuration-revision", message "Revision [3] of configuration [c-22ba392b-8311-64d2-86d9-c51a1e0ff671] can't be found. Make sure it exists." Fixed text: "Revision [" + <requested revision> + "] of configuration [" + <configuration id from the request> + "] can't be found. Make sure it exists."; the recorded revision number and configuration id are values from the request, not requirements.
Emulator: returns 200 with a revision body. Replay: status expected 404, got 200; message/errorAttribute/x-amzn-errortype absent.
Why: a client cannot distinguish an existing revision from a missing one; a rollback/diff tool that walks revision numbers until it gets a 404 never terminates against the emulator. Hypothesis: the emulator has a revision 3 to serve because it accepted the malformed update of Cause 4, so the bounds check never fails - but the handler must still 404 on an out-of-range revision independently of that.
Fix: DescribeConfigurationRevision's handler - bound the requested revision against the configuration's recorded revision chain and emit NotFoundException with the template above.

Cause 6 of 8: CreateBroker ACCEPTS an engineVersion the engine type does not support (fixture mq-rmq-traj-recreate-rabbitmq-broker-refusals, step task#5).
AWS: Smithy models CreateBroker as POST /v1/brokers with required EngineType (enum ACTIVEMQ|RABBITMQ) and optional EngineVersion (__string), errors BadRequestException (400) / ConflictException (409). Recorded behavior: an unsupported version is refused with 400 BadRequestException, errorAttribute "engineVersion", message "Broker engine version [3.99] is invalid for broker engine type [RABBITMQ]. Valid values: [4.3, 4.2, 3.13]." Fixed text: "Broker engine version [" + <requested version> + "] is invalid for broker engine type [" + <requested engine type> + "]. Valid values: [" + <the service's supported-version list for that engine, newest first> + "]." The recorded list is this engine's catalog at recording time (the same catalog DescribeBrokerEngineTypes serves - still a stub, see 20261029T174752Z-mq-describebrokerenginetypes-stub.md); it is a value from the emulator's catalog, not a literal to hardcode.
Emulator: returns 200 and creates the broker. Replay: status expected 400, got 200; message/errorAttribute/x-amzn-errortype absent.
Why: a client gets a broker AWS would never have created, and the bogus broker then poisons later calls (it is exactly what makes Cause 7 answer 409).
Fix: CreateBroker's validation - check EngineVersion against the engine-version catalog for the requested EngineType.

Cause 7 of 8: CreateBroker does not validate engine/host-instance-type compatibility, and checks broker-name uniqueness first, so an incompatible request is answered 409 ConflictException instead of 400 BadRequestException (fixture mq-rmq-traj-recreate-rabbitmq-broker-refusals, step task#6; cluster also covers task#7, task#9, task#10).
AWS: Smithy CreateBroker errors BadRequestException (400) and ConflictException (409); BrokerStorageType's own doc ("EFS is not supported for RabbitMQ engine type") shows engine/host compatibility is service-enforced. Recorded behavior: RabbitMQ with hostInstanceType mq.t2.micro is refused with 400 BadRequestException, errorAttribute "hostInstanceType", message "Broker engine type [RabbitMQ] does not support host instance type [mq.t2.micro]." Fixed text: "Broker engine type [" + <engine type as the service spells it in this message, mixed case e.g. RabbitMQ> + "] does not support host instance type [" + <requested hostInstanceType> + "]." Because AWS rejects the request on the parameter, the broker name is never consumed and a repeat with the same name is refused the same way.
Emulator: 409 ConflictException, errorAttribute "brokerName", message 'a broker named "<name>" already exists in this account', x-amzn-errortype ConflictException - the name collides because the emulator had already created brokers from requests AWS refuses (Cause 6). Replay: status expected 400, got 409 on all four steps of the cluster.
Why: a client gets the wrong error class (a conflict it must resolve by renaming, instead of a parameter error it must fix), the wrong errorAttribute to point its user at, and an unretryable situation: on AWS the name is still free.
Fix: CreateBroker's validation - add the engine/host-instance-type compatibility check and run parameter validation BEFORE the name-uniqueness/conflict check, so a refused request never consumes the name.

Cause 8 of 8: CreateBroker's user-password rejection uses the emulator's own wording and points at the wrong attribute (fixture mq-rmq-traj-recreate-rabbitmq-broker-refusals, step task#8).
AWS: Smithy User.Password is required and documented "must be at least 12 characters long, must contain at least 4 unique characters, and must not contain commas, colons, or equal signs (,:=)". Recorded behavior: 400 BadRequestException, errorAttribute "users[0].password" (the indexed path of the offending member), message "Password is invalid. Enter a password 12 to 250 characters long." - that text is entirely fixed; it does not quote the password, its length, or the bound that failed.
Emulator: right status, but message "users[0]: password must be at least 12 characters long, got 5 characters" and errorAttribute "users" (the list, not the indexed member).
Why: the message leaks the supplied password's length and is text no SDK would surface, and the coarse errorAttribute stops a client from highlighting which user in the list was rejected.
Fix: CreateBroker's user validation/error serializer - emit the fixed AWS text and the indexed errorAttribute users[<i>].password.

None of these eight clusters is owned: the replay reported owned_by empty and no stub_path for each, while the sibling mq clusters in the same run (DescribeBrokerInstanceOptions catalog, DescribeConfigurationRevision data / ListConfigurationRevisions nextToken, CreateTags/DeleteTags content-length) are owned by 20261030T033328Z-mq-describebrokerinstanceoptions-serves-a-hand-built-instance-option.md, 20261031T013311Z-mq-configuration-revision-reads-diverge-from-recorded-amazon.md and 20261030T154619Z-mq-tagging-operations-omit-content-length-from-their.md and are excluded here. The open issue 20261030T170907Z-mq-configuration-operations-createconfiguration-wrongly-refuses-a-duplicate.md covers CreateConfiguration duplicate-name behavior and UpdateConfiguration serving no revision, not the validation gaps above.

## Evidence
Replay summary: trace_set simparity-wire-aws-v1, emulator_up true, 8 fixtures, 50 steps, 27 passed, 23 mismatched, score 54.0%, 16 divergence clusters.

Cause 1 - mq-rmq-traj-broker-name-where-the-id-belongs#task#2, DescribeBroker, expected 404 / actual 404:
  $.message: value_mismatch (expected 'Cannot find broker ID [arn%3Aaws%3Amq%3Aus-west-2%3A000000000000%3Abroker%3Asim2real-probe-rmq-broker-7d41c2e9%3Ab-00000000-0000-0000-0000-000000000000]. Verify that your broker exists or check broker permissions.', got 'Cannot find broker ID [arn:aws:mq:us-west-2:000000000000:broker:sim2real-probe-rmq-broker-7d41c2e9:b-00000000-0000-0000-0000-000000000000]. Verify that your broker exists or check broker permissions.')
  Fixed text: 'Cannot find broker ID [' ... ']. Verify that your broker exists or check broker permissions.'; the bracketed segment is the request's raw path label.

Cause 2 - mq-rmq-traj-broker-name-where-the-id-belongs#task#3, ListBrokers, expected 400 / actual 200:
  status: expected 400, got 200
  $.message: missing (expected 'Invalid max results value [4]. Number of broker instances should be between [5] and [100].')
  $.errorAttribute: missing (expected 'maxResults')
  $headers.x-amzn-errortype: missing

Cause 3 - mq-rmq-traj-broker-name-where-the-id-belongs#task#4, ListBrokers, expected 400 / actual 400:
  $.message: value_mismatch (expected 'Invalid max results value [101]. Number of broker instances should be between [5] and [100].', got 'maxResults must be between 1 and 100, got 101')

Cause 4 - mq-rmq-traj-rabbitmq-configuration-review#task#4, UpdateConfiguration, expected 400 / actual 200:
  status: expected 400, got 200
  $.message: missing (expected 'The specified configuration data could not be parsed for reason [invalid token at line 1]. Check your syntax.')
  $.errorAttribute: missing (expected 'data')
  $headers.x-amzn-errortype: missing

Cause 5 - mq-rmq-traj-rabbitmq-configuration-review#task#7, DescribeConfigurationRevision, expected 404 / actual 200:
  status: expected 404, got 200
  $.message: missing (expected "Revision [3] of configuration [c-22ba392b-8311-64d2-86d9-c51a1e0ff671] can't be found. Make sure it exists.")
  $.errorAttribute: missing (expected 'configuration-revision')
  $headers.x-amzn-errortype: missing

Cause 6 - mq-rmq-traj-recreate-rabbitmq-broker-refusals#task#5, CreateBroker, expected 400 / actual 200:
  status: expected 400, got 200
  $.message: missing (expected 'Broker engine version [3.99] is invalid for broker engine type [RABBITMQ]. Valid values: [4.3, 4.2, 3.13].')
  $.errorAttribute: missing (expected 'engineVersion')
  $headers.x-amzn-errortype: missing

Cause 7 - mq-rmq-traj-recreate-rabbitmq-broker-refusals#task#6, CreateBroker, expected 400 / actual 409 (cluster of 4: task#6, task#7, task#9, task#10):
  status: expected 400, got 409
  $.message: value_mismatch (expected 'Broker engine type [RabbitMQ] does not support host instance type [mq.t2.micro].', got 'a broker named "sim2real-probe-rmq-rebuild-7d41c2e9" already exists in this account')
  $.errorAttribute: value_mismatch (expected 'hostInstanceType', got 'brokerName')
  $headers.x-amzn-errortype: shape_mismatch (got 'ConflictException')

Cause 8 - mq-rmq-traj-recreate-rabbitmq-broker-refusals#task#8, CreateBroker, expected 400 / actual 400:
  $.message: value_mismatch (expected 'Password is invalid. Enter a password 12 to 250 characters long.', got 'users[0]: password must be at least 12 characters long, got 5 characters')
  $.errorAttribute: value_mismatch (expected 'users[0].password', got 'users')

## Steps to Reproduce
1. git clone the repo, boot the emulator built from that checkout, and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1 - fixture mq-rmq-traj-broker-name-where-the-id-belongs step task#2: GET /v1/brokers/{BrokerId} with a percent-encoded path segment (e.g. a broker ARN whose colons are encoded as %3A) for a broker that does not exist. Expect 404 NotFoundException whose message brackets the segment exactly as sent; the emulator brackets the decoded form.
3. Cause 2 - same fixture step task#3: GET /v1/brokers?maxResults=4. Expect 400 BadRequestException (errorAttribute maxResults, 'Number of broker instances should be between [5] and [100].'); the emulator returns 200.
4. Cause 3 - same fixture step task#4: GET /v1/brokers?maxResults=101. Compare the 400's message against the AWS template; the emulator emits its own wording with a floor of 1.
5. Cause 4 - fixture mq-rmq-traj-rabbitmq-configuration-review step task#4: PUT /v1/configurations/{id} with base64 data that does not parse for the configuration's engine. Expect 400 BadRequestException (errorAttribute data, 'could not be parsed for reason [...]. Check your syntax.'); the emulator returns 200 and records a revision.
6. Cause 5 - same fixture step task#7: GET /v1/configurations/{id}/revisions/{n} for an n beyond the latest revision. Expect 404 NotFoundException (errorAttribute configuration-revision); the emulator returns 200.
7. Cause 6 - fixture mq-rmq-traj-recreate-rabbitmq-broker-refusals step task#5: POST /v1/brokers with engineType RABBITMQ and engineVersion 3.99. Expect 400 BadRequestException (errorAttribute engineVersion, valid-values list); the emulator returns 200 and creates the broker.
8. Cause 7 - same fixture steps task#6/task#7/task#9/task#10: POST /v1/brokers with engineType RABBITMQ and hostInstanceType mq.t2.micro. Expect 400 BadRequestException (errorAttribute hostInstanceType); the emulator returns 409 ConflictException on brokerName because it had already created the brokers it should have refused.
9. Cause 8 - same fixture step task#8: POST /v1/brokers with users[0].password shorter than 12 characters. Compare the 400's message and errorAttribute against 'Password is invalid. Enter a password 12 to 250 characters long.' / 'users[0].password'.
