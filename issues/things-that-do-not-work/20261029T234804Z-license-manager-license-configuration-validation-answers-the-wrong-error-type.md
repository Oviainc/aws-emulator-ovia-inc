Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-license-configuration-request-validation
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-license-configuration-request-validation
Issue-Shape-Id: com.amazonaws.licensemanager#AWSLicenseManager
Issue-Key: license-manager|CreateLicenseConfiguration|4xx->4xx|$.Message
Issue-Key: license-manager|CreateLicenseConfiguration|4xx->4xx|$.__type
Issue-Key: license-manager|CreateLicenseConfiguration|4xx->2xx|$.Message
Issue-Key: license-manager|CreateLicenseConfiguration|4xx->2xx|$.__type
Issue-Key: license-manager|CreateLicenseConfiguration|4xx->2xx|status
Issue-Key: license-manager|GetLicenseConfiguration|4xx->4xx|$.Message
Issue-Key: license-manager|UpdateLicenseConfiguration|4xx->4xx|$.Message
Issue-Key: license-manager|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: license-manager|ListLicenseConfigurations|4xx->4xx|$.Message
Issue-Key: license-manager|ListLicenseConfigurations|4xx->2xx|$.Message
Issue-Key: license-manager|ListLicenseConfigurations|4xx->2xx|$.__type
Issue-Key: license-manager|ListLicenseConfigurations|4xx->2xx|status
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# License configuration validation answers the wrong error type, wrong messages, and skips two checks

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLicenseConfiguration

## Description
The license-configuration slice of AWS License Manager refuses bad requests with its own invented wording and its own error shape, and in two cases does not refuse them at all. Eight distinct causes, all in the request-validation path of the license-configuration operations. A fixer must close ALL EIGHT before submitting.

Cause 1 of 8: CreateLicenseConfiguration omitting a @required member answers the wrong error shape.
AWS: Smithy: CreateLicenseConfiguration.Name and .LicenseCountingType are @required; the operation models InvalidParameterValueException (httpError 400, member Message). A request that omits one is refused 400 with __type 'InvalidParameterValueException' and Message 'Required parameter <member> must not be null.' — fixed text 'Required parameter ' ... ' must not be null.' with the omitted member's lowerCamel wire name ('name', 'licenseCountingType') as the only variable part. Note ValidationException is NOT in this operation's modelled error list at all (the model binds it only to ListTagsForResource).
Emulator: answers 400 with __type 'ValidationException' and NO Message member at all (replay: "$.__type: value_mismatch (expected 'InvalidParameterValueException', got 'ValidationException')", "$.Message: missing").
Why: a client that branches on the error code never sees InvalidParameterValueException and falls into its unknown-error path even though the status is right; and with Message absent there is nothing to tell the operator which parameter was missing.
Fix: the centralized required-member refusal for this service must map a missing @required member onto the operation's modelled InvalidParameterValueException with the AWS message template, instead of emitting a generic ValidationException with no message body — i.e. the service's deserialize/validate stage, not the handle body in ops/create_license_configuration.rs.
Traces: license-manager-license-configuration-request-validation#task#4 (representative), #task#5.

Cause 2 of 8: negative LicenseCount is refused with invented wording.
AWS: Smithy: CreateLicenseConfiguration.LicenseCount targets BoxLong; the modelled refusal is InvalidParameterValueException 400 with Message "Field 'licenseCount' cannot be lesser than 0." — entirely fixed text apart from the quoted member name, and it does NOT echo the submitted value.
Emulator: 400 with Message 'LicenseCount must not be negative, got -5' — PascalCase member name, different sentence, and it appends the request's value.
Why: AWS's message is part of the contract clients log, assert on and surface to users; the emulator's wording also leaks the submitted value into an error class that never carries it.
Fix: the license-count bound inside crate::state::configuration_input_rejection (services/service-com-amazonaws-licensemanager-awslicensemanager/src/state.rs), reached from ops/create_license_configuration.rs.
Traces: license-manager-license-configuration-request-validation#task#6.

Cause 3 of 8: an enforced license count with no positive LicenseCount is accepted instead of refused.
AWS: Smithy: CreateLicenseConfiguration.LicenseCountHardLimit (BoxBoolean) selects hard enforcement; LicenseCount (BoxLong) is optional in the model, so the rule is a service-side one. When enforcement is requested but LicenseCount is null or not greater than zero, AWS refuses the call 400 InvalidParameterValueException with Message 'License count should be non-null and greater than zero, if license count has been enforced.' — fully fixed text, no variable part.
Emulator: answers 200 and mints a license configuration (replay: "status: expected 400, got 200", __type and Message both missing). Six recorded steps are affected, so every spelling of the invalid combination settles.
Why: the caller gets an ARN for a configuration AWS would never have created — a hard-limit configuration with no limit — and every later Get/List/association built on that ARN diverges from real License Manager from then on.
Fix: add the enforcement-consistency guard to the CreateLicenseConfiguration validation path (crate::state::configuration_input_rejection, which today checks name/description/licenseCount/licenseExpiry/rules/productInfo/tags but never correlates LicenseCountHardLimit with LicenseCount).
Traces: license-manager-license-configuration-request-validation#task#7 (representative), #task#8 through #task#12.

Cause 4 of 8: GetLicenseConfiguration on an unknown ARN uses invented wording.
AWS: Smithy: GetLicenseConfiguration.LicenseConfigurationArn is @required; the operation models InvalidParameterValueException 400. An ARN that resolves to no configuration is refused with Message 'Invalid license configuration ARN <arn>.' — fixed text 'Invalid license configuration ARN ' + the ARN exactly as the request supplied it + '.'.
Emulator: 400 with Message "no license configuration exists with the ARN '<arn>'" — different sentence, lower case, ARN single-quoted, no terminating period.
Why: clients and operators matching on AWS's refusal text do not recognise the emulator's, so a negative-path assertion that passes against AWS fails here.
Fix: the not-found refusal in the GetLicenseConfiguration handler (ops/get_license_configuration.rs) and whatever shared formatter builds that sentence.
Traces: license-manager-license-configuration-request-validation#task#13 (representative), #task#16.

Cause 5 of 8: UpdateLicenseConfiguration on an unknown ARN uses invented wording.
AWS: Smithy: UpdateLicenseConfiguration.LicenseConfigurationArn is @required; the operation models InvalidParameterValueException 400. AWS refuses an ARN it cannot resolve (or cannot resolve for this owner/status) with Message 'Please check license configurations for ownership and status.' — fully fixed text that deliberately names NO ARN.
Emulator: 400 with Message "no license configuration exists with the ARN '<arn>'" — different sentence, and it echoes the ARN AWS withholds.
Why: besides breaking text-matching clients, the emulator discloses the resolved-against ARN in a refusal AWS keeps deliberately opaque about ownership.
Fix: the not-found/ownership refusal in the UpdateLicenseConfiguration handler (ops/update_license_configuration.rs).
Traces: license-manager-license-configuration-request-validation#task#14.

Cause 6 of 8: ListTagsForResource on an unknown ARN uses invented wording.
AWS: Smithy: ListTagsForResource.ResourceArn is @required; the operation models InvalidParameterValueException 400. An unresolvable resource ARN is refused with Message 'Invalid resource ARN <arn>.' — fixed text 'Invalid resource ARN ' + the request's ARN + '.'.
Emulator: 400 with Message "no resource exists with the ARN '<arn>'".
Why: same contract break as above, on the tagging read path.
Fix: the resource-resolution refusal in the ListTagsForResource handler (ops/list_tags_for_resource.rs).
Traces: license-manager-license-configuration-request-validation#task#15.

Cause 7 of 8: ListLicenseConfigurations MaxResults out of range uses invented wording.
AWS: Smithy: ListLicenseConfigurations.MaxResults targets BoxInteger with no modelled @range, so the 1..100 bound is service-side; the operation models InvalidParameterValueException 400. A MaxResults outside it is refused with Message 'Invalid parameter value. Valid range is from 1 to 100 for MaxResults' — fixed text including the limits and the PascalCase member name, with NO trailing period and no echo of the submitted value.
Emulator: 400 with Message 'MaxResults must be between 1 and 100, got 0' — different sentence and it appends the request's value.
Why: paginating clients that assert on the refusal text (and operators reading it) see a message real License Manager never produces.
Fix: the MaxResults bound check in the ListLicenseConfigurations handler (ops/list_license_configurations.rs) / its shared pagination validation.
Traces: license-manager-license-configuration-request-validation#task#17 (representative), #task#19, #task#20.

Cause 8 of 8: ListLicenseConfigurations never bounds the LicenseConfigurationArns list, so an out-of-range list is served.
AWS: Smithy: ListLicenseConfigurations.LicenseConfigurationArns targets StringList with no modelled @length, so the 1..100 bound is service-side; the operation models InvalidParameterValueException 400. A list size outside that range is refused 400 with Message 'Invalid list size for parameter LicenseConfigurationArns. Valid range is range 1 to 100.' — fully fixed text (AWS's own doubled 'range is range' wording and the member name included).
Emulator: answers 200 with a normal page (replay: "status: expected 400, got 200", __type and Message missing) — the list size is never checked.
Why: a client that sends an over-long or empty ARN selection gets a silently different result set instead of the refusal it would get from AWS, so the bug surfaces only in production.
Fix: add the list-size bound to the ListLicenseConfigurations validation path (ops/list_license_configurations.rs), alongside the MaxResults bound of Cause 7.
Traces: license-manager-license-configuration-request-validation#task#18.

None of the 15 open license-manager issues covers this path: they are all against license-manager-linux-subscriptions and license-manager-user-subscriptions operations (ListLinuxSubscriptionInstances, RegisterIdentityProvider, AssociateUser, CreateLicenseServerEndpoint, StopProductSubscription, ...), not the self-managed license-configuration operations above.

## Evidence
Replay of fixture license-manager-license-configuration-request-validation (trace set simparity-wire-aws-v1): 22 steps, 1 passed, 21 mismatched, score 4.5%. All 8 divergence clusters are license-manager.

Cause 1 — task#4 (+task#5), CreateLicenseConfiguration, expected 400 / actual 400: $.__type expected 'InvalidParameterValueException', got 'ValidationException'; $.Message expected 'Required parameter name must not be null.', got None (member absent).
Cause 2 — task#6, CreateLicenseConfiguration, 400/400: $.Message expected "Field 'licenseCount' cannot be lesser than 0.", got 'LicenseCount must not be negative, got -5'.
Cause 3 — task#7 (+task#8..#task#12), CreateLicenseConfiguration: status expected 400, got 200; $.__type expected 'InvalidParameterValueException', got None; $.Message expected 'License count should be non-null and greater than zero, if license count has been enforced.', got None.
Cause 4 — task#13 (+task#16), GetLicenseConfiguration, 400/400: $.Message expected 'Invalid license configuration ARN <arn>.', got "no license configuration exists with the ARN '<arn>'".
Cause 5 — task#14, UpdateLicenseConfiguration, 400/400: $.Message expected 'Please check license configurations for ownership and status.', got "no license configuration exists with the ARN '<arn>'".
Cause 6 — task#15, ListTagsForResource, 400/400: $.Message expected 'Invalid resource ARN <arn>.', got "no resource exists with the ARN '<arn>'".
Cause 7 — task#17 (+task#19, #task#20), ListLicenseConfigurations, 400/400: $.Message expected 'Invalid parameter value. Valid range is from 1 to 100 for MaxResults', got 'MaxResults must be between 1 and 100, got 0'.
Cause 8 — task#18, ListLicenseConfigurations: status expected 400, got 200; $.__type expected 'InvalidParameterValueException', got None; $.Message expected 'Invalid list size for parameter LicenseConfigurationArns. Valid range is range 1 to 100.', got None.

In the ARN-bearing messages the ARN is the one the request supplied, not a fixed value; the recorded runs used arn:aws:license-manager:us-west-2:000000000000:license-configuration:lic-00000000000000000000000000000000 as the unresolvable ARN, and '-5' / '0' as the out-of-range LicenseCount / MaxResults. Those are request values filling the template, not part of it.

Model evidence (services/service-com-amazonaws-licensemanager-awslicensemanager/smithy-model/com-amazonaws-licensemanager-awslicensemanager.json): CreateLicenseConfiguration errors = AccessDeniedException(401), AuthorizationException(403), InvalidParameterValueException(400), RateLimitExceededException(429), ResourceLimitExceededException(400), ServerInternalException(500) — ValidationException is NOT among them. InvalidParameterValueException is a client error, httpError 400, with a single member Message, doc 'One or more parameter values are not valid.' CreateLicenseConfiguration input: Name required, LicenseCountingType required (enum Core|Instance|Socket|vCPU), LicenseCount BoxLong, LicenseCountHardLimit BoxBoolean. ListLicenseConfigurations input: LicenseConfigurationArns StringList, MaxResults BoxInteger, NextToken, Filters — no modelled range or length, so both bounds are service-side rules.

Code evidence: ops/create_license_configuration.rs handle() validates only LicenseCountingType against LICENSE_COUNTING_TYPE_VALUES and then delegates to crate::state::configuration_input_rejection(name, description, license_count, license_expiry, license_rules, product_information_list, tags) — LicenseCountHardLimit is passed to the store (field license_count_hard_limit) but never correlated with license_count, which is why Cause 3 settles with 200. Its invalid_parameter() helper builds the service's own sentences rather than AWS's templates.

## Steps to Reproduce
Replay fixture license-manager-license-configuration-request-validation from trace set simparity-wire-aws-v1 against the emulator (all steps are in its 'task' phase):
1. task#4 / task#5 — POST CreateLicenseConfiguration omitting a @required member (Name; LicenseCountingType): emulator returns 400 ValidationException with no Message; AWS returns 400 InvalidParameterValueException, Message 'Required parameter name must not be null.'
2. task#6 — CreateLicenseConfiguration with LicenseCount negative: compare Message against "Field 'licenseCount' cannot be lesser than 0."
3. task#7..task#12 — CreateLicenseConfiguration with LicenseCountHardLimit requesting enforcement while LicenseCount is null or not greater than zero: emulator answers 200 with a fresh LicenseConfigurationArn; AWS answers 400 InvalidParameterValueException 'License count should be non-null and greater than zero, if license count has been enforced.'
4. task#13 / task#16 — GetLicenseConfiguration with an ARN that resolves to no stored configuration: compare Message against 'Invalid license configuration ARN <that ARN>.'
5. task#14 — UpdateLicenseConfiguration with the same unresolvable ARN: compare Message against 'Please check license configurations for ownership and status.'
6. task#15 — ListTagsForResource with the same unresolvable ARN: compare Message against 'Invalid resource ARN <that ARN>.'
7. task#17 / task#19 / task#20 — ListLicenseConfigurations with MaxResults outside 1..100 (0, 101, negative): compare Message against 'Invalid parameter value. Valid range is from 1 to 100 for MaxResults'
8. task#18 — ListLicenseConfigurations with a LicenseConfigurationArns list whose size is outside 1..100: emulator answers 200 with a page; AWS answers 400 InvalidParameterValueException 'Invalid list size for parameter LicenseConfigurationArns. Valid range is range 1 to 100.'
