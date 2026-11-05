Issue-Source: trace-sweep-divergence
Issue-Trace: mailmanager-address-list-member-roundtrip
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mailmanager-address-list-member-roundtrip
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager|GetMemberOfAddressList|4xx->4xx|$.Message
Issue-Key: mailmanager|GetMemberOfAddressList|4xx->4xx|status
Issue-Commit: 659c03df077d127acba792265868903833854a26

# mailmanager GetMemberOfAddressList returns 404 and omits the AWS account from the not-found message

> _Note: this issue was identified against the code base at commit `659c03df077d127acba792265868903833854a26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMemberOfAddressList

## Description
1 cause.

Cause 1 of 1: GetMemberOfAddressList for an address that is not a member of an existing address list is answered with the wrong HTTP status and with a not-found message that drops the AWS-account clause AWS includes.

  AWS: recorded behavior only. The Smithy model (services/service-com-amazonaws-mailmanager-mailmanagersvc/smithy-model/com-amazonaws-mailmanager-mailmanagersvc.json) binds GetMemberOfAddressList to AccessDeniedException (403), ResourceNotFoundException (404), ThrottlingException (429) and ValidationException (400), and does not say which class covers a non-member address; GetMemberOfAddressList.AddressListId is @required with @length 1..255 and @pattern ^[a-zA-Z0-9-]+$ and Address is @required, @length 3..320, @sensitive, so the probed input is syntactically valid and this is not a syntactic rejection. The recorded AWS response is HTTP 400 with Message 'Could not find Address in AddressList <AddressListId> of AWS account <accountId>'. Fixed text, exactly: 'Could not find Address in AddressList ' ... ' of AWS account ' — the two variable parts are the AddressListId copied verbatim from the request and the AWS account id owning the address list, taken from the emulator's state/request context. The recording's AddressListId f499445b1fe8d6c0910a2dec89025cc1 and account 000000000000 are that run's values filling the template, not part of it. Note the recorded status is 400 even though the message is phrased as a not-found; the fixture does not assert the error code/__type for this step, so only the status and the message text are requirements here.

  Emulator: HTTP 404 with Message 'Could not find Address in AddressList <AddressListId>' — the status is wrong (404 instead of 400) and the message is truncated: the ' of AWS account <accountId>' clause is missing entirely.

  Why: a client that branches on the HTTP status or on the modeled error class to tell a caller-input validation failure (400) from a resource-gone condition (404) takes the wrong branch — membership probes that treat 404 as 'definitely absent, stop' versus 400 as 'bad request, surface to the caller' diverge, and SDK retry/error-mapping keyed on the 4xx class constructs the wrong typed exception. Any client, log scraper or test that matches the documented AWS message, which names both the address list and the owning account, fails to match because the account clause is absent, so the response cannot be attributed to an account in multi-account tooling.

  Fix: the GetMemberOfAddressList handler in the mailmanager service crate — its member-lookup miss path. That branch must (a) map a non-member address to the 400 error class AWS uses rather than the 404 ResourceNotFoundException path, and (b) build the message with both variable parts, appending ' of AWS account ' plus the account id from the request/state context instead of stopping after the address-list id.

  Traces: mailmanager-address-list-member-roundtrip#task#5 and mailmanager-address-list-member-roundtrip#task#8 (same cluster, 2 affected steps).

Related but distinct open issues, neither of which covers this cause: 20261031T222211Z-mailmanager-address-list-import-job-lookups-raise-404.md (import-job lookups, CreateAddressListImportJob/GetAddressListImportJob) and 20261029T234859Z-mailmanager-archive-search-and-export-reads-refused-with.md (archive search/export reads). This is the address-list MEMBER read path and is unowned.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health answered 200).

Cause 1 — fixture mailmanager-address-list-member-roundtrip, step task#5, operation GetMemberOfAddressList, verdict mismatch, cluster key service=mailmanager|GetMemberOfAddressList|status=400->404|keys=$.Message,status; affected steps: mailmanager-address-list-member-roundtrip#task#5, mailmanager-address-list-member-roundtrip#task#8 (affected_count 2):
  status: expected 400, got 404
  $.Message: value_mismatch (expected 'Could not find Address in AddressList f499445b1fe8d6c0910a2dec89025cc1 of AWS account 000000000000', got 'Could not find Address in AddressList f499445b1fe8d6c0910a2dec89025cc1')

The divergence is confined to the status and the trailing ' of AWS account <accountId>' clause; the leading fixed text matches byte for byte, which pins the defect to the status selection and the message template rather than to the lookup itself.

Model evidence: GetMemberOfAddressList input {AddressListId required, length 1..255, pattern ^[a-zA-Z0-9-]+$; Address required, length 3..320, sensitive}; errors AccessDeniedException/403, ResourceNotFoundException/404, ThrottlingException/429, ValidationException/400; readonly.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay fixture mailmanager-address-list-member-roundtrip.
3. Step task#5: create an address list, then call GetMemberOfAddressList on that existing AddressListId with an Address (3..320 chars) that was never added as a member. Expected: HTTP 400 with Message 'Could not find Address in AddressList <that AddressListId> of AWS account <the caller's account id>'. Observed: HTTP 404 with Message 'Could not find Address in AddressList <that AddressListId>' (no account clause).
4. Step task#8 of the same fixture repeats the same read after the member has been deregistered and diverges identically.
