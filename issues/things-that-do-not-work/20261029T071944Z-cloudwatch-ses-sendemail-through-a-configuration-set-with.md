Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare
Issue-Key: cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare|_poll_until#16|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 37c02434d582c8370b608107fbfd29504c12ec2b

# cloudwatch__ses: SendEmail through a configuration set with an enabled CloudWatch event destination still publishes no AWS/SES Send datapoint — the declared message-tag dimension never resolves (1 cause)

> _Note: this issue was identified against the code base at commit `37c02434d582c8370b608107fbfd29504c12ec2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
One net-new root cause, grounded in the single failing cloudwatch__ses check. The replay report marks it neither "ALREADY FILED" nor "OWNED by", so it is filed here. Related open issue: 20261026T065236Z-cloudwatch-ses-ses-configuration-set-cloudwatch-event-destinations-deliver-nothing.md — it names this check id, but its evidence is a different, EARLIER stall (both cloudwatch__ses checks were then killed by the harness at the 60 s cap, with the SES identity-verification/sending surface itself named as a candidate stall point, and the related open issue 20261029T065537Z-sesv2-the-whole-ses-sending-surface-is-unreachable.md records that surface as unreachable). On this run the sending surface works: the check gets a verified sending domain, SendEmail is accepted and returns a non-empty MessageId, and the run now dies strictly LATER, inside the CloudWatch read loop. That narrows the defect to the publishing step and is the behavior described below.

Cause 1 of 1: a configuration set whose enabled event destination carries a CloudWatchDestination with one DimensionConfigurations entry publishes NO datapoint to the AWS/SES namespace when a message is sent through that configuration set, so the declared dimension's Send series never exists.
Failing check id: cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare.

AWS: recorded behavior only for the cross-service effect — the Smithy models cover the envelopes, not the delivery. Smithy (com.amazonaws.sesv2): EventDestinationDefinition carries Enabled, MatchingEventTypes (EventTypes) and CloudWatchDestination; CloudWatchDestination.DimensionConfigurations is @required, each CloudWatchDimensionConfiguration requiring DimensionName, DimensionValueSource (enum: messageTag/emailHeader/linkTags) and DefaultDimensionValue. Recorded behavior: after sesv2:SendEmail with ConfigurationSetName=<cs> and EmailTags naming the declared message tag, SES's publishing pipeline delivers exactly ONE datapoint to CloudWatch namespace AWS/SES, MetricName 'Send', dimensioned by every dimension the destination DECLARED and resolved from that send (here only {campaign=<the EmailTags value>}), readable with cloudwatch:GetMetricStatistics(Namespace='AWS/SES', MetricName='Send', Dimensions=<that combination>, Period=60, Statistics=['Sum']) with Sum == 1. The paired negative is equally recorded: a dimension the destination did NOT declare (ses:configuration-set here) is never attached to the datapoint at all — not even filled with DefaultDimensionValue — so no series exists for the declared dimension PLUS an undeclared one.
Emulator: the positive control never materialises. SendEmail returns a MessageId (the check's assertion on it passes), then the check polls _metric_sum -> cloudwatch:GetMetricStatistics for AWS/SES 'Send' with Dimensions=[{campaign=<value>}] every 2 s and gets an empty Datapoints list for the whole run, until the poll ends with TimeoutError: "AWS/SES Send metric resolved for the DECLARED dimension campaign='<value>': the run budget ended first". No status or error code diverges — both SES calls are accepted with 200 — the divergence is the missing cross-service effect: zero AWS/SES datapoints where AWS delivers one. Because the positive series never exists, the asserted ABSENCE of the undeclared-dimension series is unverifiable too: an emulator that later published Send datapoints carrying undeclared dimensions would look identical to a client today.
Why: a client that wires SES observability the documented way — one configuration set, one enabled CloudWatch event destination, dimensions declared for its message tags — reads an empty AWS/SES namespace on this emulator and concludes its sends failed or its destination is misconfigured, when SES accepted both calls. Per-campaign attribution, dashboards and every alarm over AWS/SES Send are silently dead, and the rule that only DECLARED dimensions are published cannot be exercised at all.
Fix: the SES event-publishing path — the CloudWatchDestination branch of the configuration-set event-destination handler and its state in the sesv2 crate (services/service-com-amazonaws-sesv2-simpleemailservice-v2), which must, on an accepted SendEmail through a configuration set holding an Enabled destination whose MatchingEventTypes include SEND, publish one AWS/SES 'Send' datapoint into the cloudwatch crate's metric store dimensioned by exactly the destination's declared DimensionConfigurations resolved from that send's EmailTags/headers (DefaultDimensionValue when the send supplies none) and by nothing else.
Traces: cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1: 186/294 passed (63.3%); per-service line `cloudwatch__ses 0/1 (0%) 1 failed`. The check is in the failing list as [error] and is NOT marked "ALREADY FILED" or "OWNED by".

Failing detail (verbatim tail):
  File "check_cloudwatch__ses_cloudwatch_destination_never_resolves_a_dimension_it_did_not_declare.py", line 214, in run
    declared_total = _poll_until(
  File "...", line 127, in _poll_until
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: AWS/SES Send metric resolved for the DECLARED dimension campaign='<campaign value minted by the run>': the run budget ended first

The traceback is from line 214 of the check body, which is AFTER sesv2:SendEmail and after `assert message_id` — so the send was accepted and returned a MessageId on this emulator; only the metric read fails.

Expected (AWS): cloudwatch:GetMetricStatistics(Namespace='AWS/SES', MetricName='Send', Dimensions=[{Name:'campaign', Value:<the EmailTags value>}], StartTime=<send-60s>, EndTime=<now+60s>, Period=60, Statistics=['Sum']) returns a non-empty Datapoints list whose Sum totals exactly 1; the same query with an extra Dimension {Name:'ses:configuration-set', Value:<cs>} returns NO datapoints at all (held for the check's absence window).
Actual (emulator): Datapoints is empty for the declared-dimension query for the entire run, so the first (positive-control) poll never satisfies `v is not None` and the negative half is never reached. Status/error code: none — CreateConfigurationSet, CreateConfigurationSetEventDestination and SendEmail all return 200; the divergence is a missing effect, not an error payload.

Note: the fixed/open issue history for this check describes a stall at the SES identity-verification/sending stage; that stage now passes, and the remaining divergence is located at the SES->CloudWatch publish.

## Steps to Reproduce
1. Point sesv2, route53 and cloudwatch clients at the emulator.
2. Obtain a sending domain: sesv2:CreateEmailIdentity EmailIdentity=<domain>, publish the returned DkimAttributes.Tokens as <token>._domainkey.<domain> CNAME -> <token>.dkim.amazonses.com in a route53 public hosted zone for <domain>, and wait until sesv2:GetEmailIdentity reports VerifiedForSendingStatus true and DkimAttributes.Status == 'SUCCESS'.
3. sesv2:CreateConfigurationSet ConfigurationSetName=<cs>.
4. sesv2:CreateConfigurationSetEventDestination ConfigurationSetName=<cs>, EventDestinationName='sim2real-cw-abs', EventDestination={"Enabled": true, "MatchingEventTypes": ["SEND"], "CloudWatchDestination": {"DimensionConfigurations": [{"DimensionName": "campaign", "DimensionValueSource": "MESSAGE_TAG", "DefaultDimensionValue": "none"}]}} — note ses:configuration-set is deliberately NOT declared.
5. sesv2:SendEmail FromEmailAddress=<anything>@<domain>, Destination.ToAddresses=['success@simulator.amazonses.com'], Content.Simple with any subject/body, ConfigurationSetName=<cs>, EmailTags=[{"Name": "campaign", "Value": <campaign>}]. It returns a non-empty MessageId.
6. Poll cloudwatch:GetMetricStatistics Namespace='AWS/SES', MetricName='Send', Dimensions=[{"Name":"campaign","Value":<campaign>}], StartTime=<send-60s>, EndTime=<now+60s>, Period=60, Statistics=['Sum'].
   Expected: Datapoints becomes non-empty with Sum == 1. Observed: Datapoints stays empty for the whole budget.
7. Then query the same metric with Dimensions=[{campaign=<campaign>}, {"Name":"ses:configuration-set","Value":<cs>}]. Expected: no datapoints, held. Observed: unverifiable, since the declared series does not exist either.
8. Or replay cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare.
