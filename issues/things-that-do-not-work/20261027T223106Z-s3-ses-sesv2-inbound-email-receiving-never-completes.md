Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption
Issue-Key: s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule|run#23|Timeout|time.sleep(10)
Issue-Key: s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers|run#24|Timeout|time.sleep(10)
Issue-Key: s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption|run#33|Timeout|time.sleep(10)
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# s3__ses__sesv2: inbound email receiving never completes — mail sent to an address on a verified domain never lands in the active receipt rule set's S3Action bucket, so all three inbound e2e checks hang to the harness cap

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
Cause 1 of 1: the SES inbound (email-receiving) data plane never produces a delivered message, so every check that drives the receive path end to end never terminates.
AWS: SES email receiving is a modelled, account-level data plane. A domain verified for the account (sesv2 CreateEmailIdentity + the Easy DKIM CNAMEs, plus an MX record pointing at inbound-smtp.<region>.amazonaws.com) receives mail; the account's single active receipt rule set (SES v1 SetActiveReceiptRuleSet / DescribeActiveReceiptRuleSet) is evaluated against the recipient, and a matching rule's S3Action (Smithy com.amazonaws.ses#S3Action: BucketName required, ObjectKeyPrefix, KmsKeyArn) writes the raw RFC 5322 message as an object under <ObjectKeyPrefix><messageId> in that bucket. The stored message carries the sender's Subject, the recipient in To, a DKIM-Signature with d=<verified domain>, and — when scanning is on — the Authentication-Results and X-SES-Spam-Verdict/X-SES-Virus-Verdict headers SES adds; with KmsKeyArn the object is encrypted with that key. SES also writes one AMAZON_SES_SETUP_NOTIFICATION object when a rule set is first activated. All of this is recorded behavior of the receive path (no single API operation returns it; it is observed through s3:ListObjectsV2/GetObject on the destination bucket).
Emulator: nothing is ever delivered. All three checks build the pipeline with ordinary API calls that succeed (create/verify the identity, create the bucket and its AllowSESPuts policy, CreateReceiptRuleSet + CreateReceiptRule with the S3Action, SetActiveReceiptRuleSet, then sesv2 SendEmail which returns a MessageId), then poll s3.list_objects_v2(Bucket=<bucket>, Prefix='inbox/') for up to 300s for a non-setup-notification key. No object ever appears, so each check is still inside its bounded wait when the suite kills it: "Check timed out after 60.0s" for all three, with no exception from any API call. Hypothesis: the emulator has no inbound-mail pipeline at all — a receipt rule is stored as inert configuration and SendEmail does not loop a message addressed to a locally verified domain back through the active rule set. (A second possible stall point is the stand-in domain's verification loop the checks run first; that loop is bounded too, and the auto-verification defect filed as 20261025T214303Z-ses-identity-verification-is-auto-completed-and-the.md would make it return immediately rather than hang.)
Why: the entire receiving half of SES is unusable against the emulator. A client that sends to one of its own verified domains and waits for the message to appear in S3 — the canonical inbound-processing pattern — waits forever, with no error to distinguish 'not delivered yet' from 'never will be'; nothing about S3Action (object key prefix, KMS encryption, byte-exact raw MIME) or the headers SES adds can be observed or tested.
Fix: the inbound delivery path that ties the three crates together — the ses v1 receipt-rule state must be evaluated for messages accepted for a locally verified domain (by sesv2/ses SendEmail/SendRawEmail to a recipient on that domain), and a matching rule's S3Action must write the raw message into the named bucket under its ObjectKeyPrefix (honouring KmsKeyArn) with the Subject/To/DKIM-Signature and scan headers SES adds; DescribeActiveReceiptRuleSet must be the set that is consulted.
Traces: s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule, s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers, s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption.

Note on attribution: these check ids carry the composite service token 's3__ses__sesv2'; all three constituent crates (service-com-amazonaws-s3-amazons3, service-com-amazonaws-ses-simpleemailservice, service-com-amazonaws-sesv2-simpleemailservice-v2) are bootstrapped, and the missing behavior belongs to the SES receive path.

## Evidence
Expected (AWS): after sesv2.send_email(FromEmailAddress=<sender@verified-domain>, Destination={'ToAddresses':[<recipient@verified-domain>]}, Content={'Simple':{'Subject':{'Data':<token>}, ...}}) with the active receipt rule set holding Rule{'Recipients':[<recipient>], 'Actions':[{'S3Action':{'BucketName':<bucket>,'ObjectKeyPrefix':'inbox/'}}]}, exactly one object other than 'inbox/AMAZON_SES_SETUP_NOTIFICATION' appears under inbox/ within the check's 300s wait, and GetObject on it yields a raw message whose Subject equals the probe token, whose To contains the recipient, and which carries a DKIM-Signature with d=<domain>.
Actual (emulator): every API call in the setup and the send succeeds (a MessageId is returned), but list_objects_v2(Bucket=<bucket>, Prefix='inbox/') stays empty; the checks are killed inside the poll loop — replay reports, for each of the three, only "Error: Check timed out after 60.0s" with no ClientError raised anywhere. The same absence blocks the header-assertion check (Authentication-Results / X-SES-Spam-Verdict never observable) and the ObjectKeyPrefix+KMS check (no object to read back).

## Steps to Reproduce
1. Boot the emulator and point boto3 at it (clients 'route53', 'ses', 'sesv2', 's3'; a receiving region such as us-east-1/us-west-2/eu-west-1).
2. Verify a domain D: sesv2.create_email_identity(EmailIdentity=D), publish its DkimAttributes.Tokens as CNAMEs plus an MX 10 inbound-smtp.<region>.amazonaws.com in a Route53 public hosted zone for D, and wait until get_email_identity reports VerifiedForSendingStatus true.
3. s3.create_bucket(Bucket=B) and put a bucket policy allowing ses.amazonaws.com s3:PutObject on arn:aws:s3:::B/inbox/*.
4. ses.create_receipt_rule_set(RuleSetName=RS); ses.create_receipt_rule(RuleSetName=RS, Rule={'Name':'to-s3','Enabled':True,'ScanEnabled':False,'Recipients':[f'probe@{D}'],'Actions':[{'S3Action':{'BucketName':B,'ObjectKeyPrefix':'inbox/'}}]}); ses.set_active_receipt_rule_set(RuleSetName=RS).
5. sesv2.send_email(FromEmailAddress=f'sender@{D}', Destination={'ToAddresses':[f'probe@{D}']}, Content={'Simple':{'Subject':{'Data':'probe-token'},'Body':{'Text':{'Data':'inbound probe'}}}}).
6. Poll s3.list_objects_v2(Bucket=B, Prefix='inbox/') — observe it never returns a key other than (at most) 'inbox/AMAZON_SES_SETUP_NOTIFICATION', so no raw message is ever stored.
7. Re-replay the checks listed in trace_ids.
