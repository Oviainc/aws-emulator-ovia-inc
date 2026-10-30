Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record
Issue-Key: cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record|run#32|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: 8d2d24246537bab720d4405ea7ed505663212620

# CloudTrail LookupEvents never serves the iam.amazonaws.com CreateAccessKey / CreateLoginProfile management records in the partition leader region, so credential-redaction of the audit record can never be observed (check exhausts its budget)

> _Note: this issue was identified against the code base at commit `8d2d24246537bab720d4405ea7ed505663212620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Cause 1 of 1 — CloudTrail event history contains no record of an IAM credential write, so LookupEvents can never return the CreateAccessKey / CreateLoginProfile management events (and therefore never demonstrates that the secret and the password are omitted from the record).

AWS: IAM is a global service whose management events are filed in the partition leader region (us-east-1), and CloudTrail indexes them into event history within a couple of minutes. A LookupEvents call in us-east-1 with LookupAttributes=[{AttributeKey: EventName, AttributeValue: CreateAccessKey}] over a window containing the call must return an Event whose CloudTrailEvent JSON carries eventSource "iam.amazonaws.com", readOnly false, awsRegion "us-east-1", and responseElements.accessKey with EXACTLY the members accessKeyId, createDate, status, userName — accessKeyId being the id the API just minted, userName the requesting user, status "Active"; the member secretAccessKey is OMITTED entirely (not a placeholder) and the secret value appears nowhere in the serialised record. The matching CreateLoginProfile record must carry requestParameters exactly {"userName": <the user>, "passwordResetRequired": <the request's value>} — the password member omitted entirely — and responseElements.loginProfile naming that user, with no password member. A non-global call made in another region (here DescribeTrails) must be audited in THAT region with awsRegion equal to it. Smithy: LookupEvents input LookupAttributes/StartTime/EndTime/MaxResults(1-50), output Events (list) + NextToken, paginated on NextToken; the record itself is an opaque JSON string in Event.CloudTrailEvent, so its field content is "recorded behavior only" plus the documented redaction rule (https://docs.aws.amazon.com/IAM/latest/UserGuide/cloudtrail-integration.html).

Emulator: the check creates a user, mints an access key, creates a login profile, makes a regional DescribeTrails control call, then polls LookupEvents in us-east-1 (and in the comparison region) on a 10 s interval. No record ever appears, so the poll loop runs to its 255 s budget and the check is killed by the harness: `Error: Check timed out after 60.0s`. This is consistent with every other cloudtrail__iam check in this same run, which fail with an empty Events list for IAM writes/reads. Hypothesis: the emulator records no management event at all for iam.amazonaws.com (and nothing in the leader region), rather than recording one with a leaked secret; a second, non-exclusive hypothesis for the timeout is that the freshly minted access key cannot sign sts:GetCallerIdentity within the check's 60 s usability loop — both are emulator-side, and the first is what the surrounding failures in this suite show.

Why: a client that audits credential creation (compliance pipelines, detective controls, "who created this access key" forensics) gets nothing back from event history for the most security-sensitive IAM writes. It also makes the redaction contract unverifiable: an emulator that later starts recording these events must omit secretAccessKey and password entirely, and any implementation that fills them in would leak live credentials into a log customers export and share.

Fix: CloudTrail's event-recording/ingest path for IAM management events (the leader-region event store feeding LookupEvents) plus the serializer that builds the CloudTrailEvent JSON for CreateAccessKey and CreateLoginProfile — it must populate requestParameters/responseElements from the request and the minted entity while dropping the secretAccessKey and password members, and file the record under awsRegion us-east-1 while non-global calls are filed under their own region.

Related open issues that do NOT cover this check (they carry other replay keys): 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md and 20261030T124208Z-cloudtrail-records-no-iam-amazonaws-com-getuser-event.md (GetUser read, not the credential writes or the redaction contract).

Traces: cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record.

## Evidence
Expected: within ~140 s of the calls, LookupEvents(StartTime=<call time - 2m>, EndTime=now+5m, MaxResults=50, LookupAttributes=[{AttributeKey:'EventName', AttributeValue:'CreateAccessKey'}]) against us-east-1 returns an Event whose CloudTrailEvent JSON mentions the created user and has eventSource='iam.amazonaws.com', readOnly=false, responseElements.accessKey keys sorted == ['accessKeyId','createDate','status','userName'] with accessKeyId == the id returned by CreateAccessKey, userName == the created user, status == 'Active', no 'secretAccessKey' member and the secret string absent from the whole serialised record; the same lookup for 'CreateLoginProfile' returns a record with requestParameters == {'userName': <user>, 'passwordResetRequired': False}, no password anywhere, responseElements.loginProfile.userName == <user>, awsRegion == 'us-east-1'; and the comparison-region DescribeTrails call is audited in that region with awsRegion == <comparison region> and requestParameters == {'trailNameList': [<token>]}.

Actual: no matching event is ever returned by either region's LookupEvents; the check's bounded poll loop (10 s interval, 255 s budget) never satisfies its first assertion and the run is aborted with 'Error: Check timed out after 60.0s'. The assertions it would have reached (DATAPLANE_UNIMPLEMENTED: 'CloudTrail in us-east-1 produced no record of the iam:CreateAccessKey for <user> after N polls over Ns') are never reported because the budget outlives the harness timeout. Fixed text of the requirement is the member-name sets and the eventSource/readOnly/awsRegion values; the user name, access key id, secret, password and token are values taken from the request and the emulator's state, not literals.

## Steps to Reproduce
1. iam.create_user(UserName=<unique>) and note the user ARN.
2. key = iam.create_access_key(UserName=<unique>)['AccessKey'] — keep AccessKeyId and SecretAccessKey.
3. iam.create_login_profile(UserName=<unique>, Password=<strong>, PasswordResetRequired=False).
4. In a non-leader region, call cloudtrail.describe_trails(trailNameList=['<unique-token>']) as a regional control.
5. Against cloudtrail in us-east-1, poll lookup_events(StartTime=<step 2 time - 2 min>, EndTime=now+5 min, MaxResults=50, LookupAttributes=[{'AttributeKey':'EventName','AttributeValue':'CreateAccessKey'}]), paging NextToken to exhaustion, and look for an Event whose CloudTrailEvent mentions the user name. Repeat for 'CreateLoginProfile'.
6. Observe the Events list is empty for both (and for the regional DescribeTrails control in its own region), so the loop never terminates early and the check is killed at the harness timeout.
7. Replay: behavioral check cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record.
