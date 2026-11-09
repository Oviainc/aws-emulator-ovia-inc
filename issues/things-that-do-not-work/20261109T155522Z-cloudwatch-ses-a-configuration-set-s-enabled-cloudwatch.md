Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions
Issue-Key: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions|_poll_until#17|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 5b2a9eedb8fa287cac2318fd09ae07f52de03ab6

# cloudwatch__ses: a configuration set's enabled CloudWatch event destination publishes no AWS/SES Send datapoint — declared MESSAGE_TAG and EMAIL_HEADER dimensions never resolve

> _Note: this issue was identified against the code base at commit `5b2a9eedb8fa287cac2318fd09ae07f52de03ab6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
One net-new root cause, grounded in the one cloudwatch__ses check this replay does NOT mark "ALREADY FILED" or "OWNED by". The sibling check in the same group (cloudwatch__ses.cloudwatch-destination-never-resolves-a-dimension-it-did-not-declare) is OWNED by 20261029T071944Z-cloudwatch-ses-sendemail-through-a-configuration-set-with.md and is skipped here; that issue carries only that check's replay key, so it does not cover this one, which exercises the POSITIVE direction of the same publishing path (all declared dimensions, including an EMAIL_HEADER-sourced one, must resolve onto one datapoint). Also related and not covering it: 20261109T135435Z-firehose-s3-sesv2-a-configuration-set-s-kinesisfirehosedestination.md (the Firehose branch of the same event-destination pipeline).

Cause 1 of 1: a message sent through a configuration set whose Enabled event destination declares a CloudWatchDestination with three DimensionConfigurations — one resolving `ses:configuration-set` from MESSAGE_TAG, one resolving a custom `campaign` tag from MESSAGE_TAG, and one resolving `X-Custom-Header` from EMAIL_HEADER — publishes NO datapoint at all into the AWS/SES namespace, so the Send series for that resolved dimension combination never exists.
Failing check id: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions.
AWS: recorded behavior only for the cross-service effect — the Smithy models cover the envelopes, not the metric delivery. Smithy (com.amazonaws.sesv2): SendEmail POST /v2/email/outbound-emails, input {Content @required (EmailContent), FromEmailAddress, Destination, EmailTags (MessageTagList), ConfigurationSetName, ...}, output {MessageId (OutboundMessageId)}, code 200; CloudWatchDestination.DimensionConfigurations is @required and each CloudWatchDimensionConfiguration requires DimensionName, DimensionValueSource (MESSAGE_TAG | EMAIL_HEADER | LINK_TAG) and DefaultDimensionValue. Recorded behavior: after an accepted SendEmail carrying ConfigurationSetName=<cs> and EmailTags naming the declared tag, plus a MIME header matching the EMAIL_HEADER-sourced dimension, SES's event-publishing pipeline delivers exactly ONE datapoint to CloudWatch Namespace 'AWS/SES', MetricName 'Send', dimensioned by every DECLARED dimension resolved from that send — `ses:configuration-set` = the configuration set name, the MESSAGE_TAG dimension = the matching EmailTags value, and the EMAIL_HEADER dimension = the value of that header on the message (DefaultDimensionValue when the send supplies none) — readable with cloudwatch GetMetricStatistics(Namespace='AWS/SES', MetricName='Send', Dimensions=<that exact combination>, Period=60, Statistics=['Sum']) with Sum == 1.
Emulator: nothing is refused. sesv2 CreateConfigurationSet, CreateConfigurationSetEventDestination and SendEmail all answer 200 and SendEmail returns a non-empty MessageId (the check's assertion on it passes). But cloudwatch GetMetricStatistics for AWS/SES 'Send' with the resolved dimension combination returns an empty Datapoints list for the entire 300 s poll (15 s interval), so the poll ends with AssertionError "AWS/SES Send metric resolved for dimensions ses:configuration-set=<cs>, campaign=<tag value>, X-Custom-Header=<header value> not met within 300.0s. Last result: None" — the probe's last result is None, i.e. no datapoint ever. No HTTP status, error code or error message diverges; the divergence is the missing cross-service effect: zero AWS/SES datapoints where AWS publishes one.
Why: a client that wires SES observability the documented way — one configuration set, one Enabled CloudWatch event destination, dimensions declared for its message tags and MIME headers — reads an empty AWS/SES namespace on this emulator even though SES accepted every call, and cannot tell a lost send from a misconfigured destination. Per-campaign and per-header attribution, every dashboard over AWS/SES Send and every alarm built on it are silently dead; and because no datapoint is ever published, the companion rule (that only DECLARED dimensions are attached) is unverifiable too — an emulator attaching undeclared dimensions would look identical.
Fix: the SES event-publishing path — the CloudWatchDestination branch of the configuration-set event-destination handler and its state in the sesv2 crate (services/service-com-amazonaws-sesv2-simpleemailservice-v2) — must, on an accepted SendEmail through a configuration set holding an Enabled destination whose MatchingEventTypes include SEND, publish one 'Send' datapoint into the cloudwatch crate's metric store under Namespace 'AWS/SES', dimensioned by exactly the destination's declared DimensionConfigurations resolved per DimensionValueSource (MESSAGE_TAG from EmailTags, EMAIL_HEADER from the message's headers, falling back to DefaultDimensionValue) and by nothing else.
Traces: cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions.

## Evidence
Replayed behavioral suite simparity-behavioral-aws-v1: 95/132 passed (72.0%); per-service line `cloudwatch__ses      0/2 (0%) 2 failed` (the other of the two is OWNED by 20261029T071944Z-... and skipped).

Failing detail (verbatim tail):
  File "check_cloudwatch__ses_cloudwatch_destination_registers_send_metric_with_resolved_dimensions.py", line 152, in run
    total = _poll_until(
  File "...", line 51, in _poll_until
    raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
  AssertionError: AWS/SES Send metric resolved for dimensions ses:configuration-set='sim2real-behav-cwdim-477492ce2f98', campaign='sim2real-behav-campaign-c2ddeacb4324', X-Custom-Header='sim2real-behav-hdrval-2ca06abce8f3' not met within 300.0s. Last result: None

The traceback is at line 152, AFTER create_configuration_set, create_configuration_set_event_destination and send_email and after `assert message_id` — so all three SES calls were accepted with 200 and a MessageId was returned; only the metric read fails. The sim2real-* names in the message are per-run minted values (the configuration-set name, the EmailTags value and the X-Custom-Header value from the request), not part of any requirement.

Expected (AWS): cloudwatch GetMetricStatistics(Namespace='AWS/SES', MetricName='Send', Dimensions=[{ses:configuration-set=<cs>},{campaign=<tag value>},{X-Custom-Header=<header value>}], StartTime=<send-60s>, EndTime=<now+60s>, Period=60, Statistics=['Sum']) returns a non-empty Datapoints list totalling Sum == 1.
Actual (emulator): Datapoints stays empty for the whole 300 s window (probe returns None on every one of the 15 s-interval polls). Status/error codes: none diverge — every SES and CloudWatch call answers 2xx.

## Steps to Reproduce
1. Build and boot the emulator; point sesv2, route53 and cloudwatch clients at it.
2. Obtain a verified sending domain <domain> (sesv2 GetEmailIdentity reports VerifiedForSendingStatus true for it).
3. sesv2 CreateConfigurationSet ConfigurationSetName=<cs>.
4. sesv2 CreateConfigurationSetEventDestination ConfigurationSetName=<cs>, EventDestinationName='sim2real-cw', EventDestination={"Enabled": true, "MatchingEventTypes": ["SEND"], "CloudWatchDestination": {"DimensionConfigurations": [{"DimensionName":"ses:configuration-set","DimensionValueSource":"MESSAGE_TAG","DefaultDimensionValue":"none"},{"DimensionName":"campaign","DimensionValueSource":"MESSAGE_TAG","DefaultDimensionValue":"none"},{"DimensionName":"X-Custom-Header","DimensionValueSource":"EMAIL_HEADER","DefaultDimensionValue":"none"}]}}.
5. sesv2 SendEmail FromEmailAddress=<any>@<domain>, Destination.ToAddresses=['success@simulator.amazonses.com'], Content.Simple with a Subject, a Text Body and Headers=[{"Name":"X-Custom-Header","Value":<header value>}], ConfigurationSetName=<cs>, EmailTags=[{"Name":"campaign","Value":<campaign>}]. It returns a non-empty MessageId.
6. Poll cloudwatch GetMetricStatistics Namespace='AWS/SES', MetricName='Send', Dimensions=[{"Name":"ses:configuration-set","Value":<cs>},{"Name":"campaign","Value":<campaign>},{"Name":"X-Custom-Header","Value":<header value>}], StartTime=<send-60s>, EndTime=<now+60s>, Period=60, Statistics=['Sum'].
   Expected: Datapoints becomes non-empty with Sum == 1.
   Observed: Datapoints stays empty for the whole window.
7. Or re-replay cloudwatch__ses.cloudwatch-destination-registers-send-metric-with-resolved-dimensions.
