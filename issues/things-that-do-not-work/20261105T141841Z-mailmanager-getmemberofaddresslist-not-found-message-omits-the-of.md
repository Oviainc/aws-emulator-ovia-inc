Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mailmanager.deregister-member-is-idempotent-for-unknown-and-removed-addresses mailmanager.member-lookup-requires-exact-post-normalization-casing
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager|GetMemberOfAddressList|ResourceNotFoundException
Issue-Key: mailmanager.member-lookup-requires-exact-post-normalization-casing|run#10|AssertionError|assert message == expected_message, f"unexpected message {message!r}"
Issue-Commit: b1e0a1b04e1a2083a7e2ad43b19651cd6178021a

# mailmanager GetMemberOfAddressList not-found message omits the ' of AWS account <accountId>' clause

> _Note: this issue was identified against the code base at commit `b1e0a1b04e1a2083a7e2ad43b19651cd6178021a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMemberOfAddressList

## Description
1 cause.

NOTE — overlaps open issue 20261105T093406Z-mailmanager-getmemberofaddresslist-returns-404-and-omits-the-aws.md, which was filed from the WIRE corpus (fixture mailmanager-address-list-member-roundtrip, steps task#5/task#8) and carries wire replay keys only. That issue does not carry either behavioral check id below, so these two behavioral checks are unowned and are filed here; a fixer closing the two together should re-replay both the wire steps and the check ids in trace_ids. Unlike that wire issue, the behavioral checks accept the emulator's error CODE (ResourceNotFoundException) and assert only the message text, so the requirement grounded here is the message template, not the HTTP status.

Cause 1 of 1: GetMemberOfAddressList, asked for an address that is NOT a member of an existing address list, returns a ResourceNotFoundException whose message stops after the address-list id and drops the AWS-account clause AWS appends. It diverges identically for an address that was never registered, for one removed by DeregisterMemberFromAddressList, and for a case variant of a registered address.

  AWS: recorded behavior only. The Smithy model (services/service-com-amazonaws-mailmanager-mailmanagersvc/smithy-model/com-amazonaws-mailmanager-mailmanagersvc.json) binds GetMemberOfAddressList to AccessDeniedException (403), ResourceNotFoundException (404), ThrottlingException (429) and ValidationException (400) and does not state the message text; input AddressListId is @required, @length 1..255, @pattern ^[a-zA-Z0-9-]+$, and Address is @required, @length 3..320, @sensitive, so the probed inputs are syntactically valid and this is not a syntactic rejection. The recorded refusal carries Error.Code 'ResourceNotFoundException' and Error.Message 'Could not find Address in AddressList <AddressListId> of AWS account <accountId>'. The FIXED text is exactly 'Could not find Address in AddressList ' ... ' of AWS account '; the two VARIABLE parts are the AddressListId copied verbatim from the request and the AWS account id that owns the address list, taken from the emulator's request/state context. Concrete ids and account numbers seen in any one run fill that template and are not part of it. Address is @sensitive, so the message names no address at all.

  Emulator: Error.Code 'ResourceNotFoundException' (accepted) but Error.Message 'Could not find Address in AddressList <AddressListId>' — the trailing ' of AWS account <accountId>' clause is absent entirely. Observed as 'Could not find Address in AddressList 6189d19113d8c0f7910a2dec89025cc1' and 'Could not find Address in AddressList 8d4a24a37afe1633910a2dec89025cc1' in the two checks; the leading fixed text matches byte for byte, which pins the defect to the message template, not to the lookup (the lookup itself is correct: the deregistered address and the case variants are all reported absent).

  Why: a client, log scraper or test that matches AWS's documented message — which names both the address list and the owning account — fails to match, so the refusal cannot be attributed to an account in multi-account tooling, and any caller that parses the account out of the message to decide which credential/account the miss belongs to gets nothing.

  Fix: the GetMemberOfAddressList handler in the mailmanager service crate — its member-lookup miss branch, which must build the message with both variable parts, appending ' of AWS account ' plus the account id from the request/state context instead of stopping after the address-list id. Any sibling member-read path that formats the same not-found text should use the same template.

  Traces: behavioral checks mailmanager.deregister-member-is-idempotent-for-unknown-and-removed-addresses (miss after DeregisterMemberFromAddressList) and mailmanager.member-lookup-requires-exact-post-normalization-casing (miss on a case variant of a registered address). Message is formatted in services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/get_member_of_address_list.rs lines 195-203.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, replayed against the emulator built from this checkout (mailmanager answered: 3/8 checks passed, so the service is serving).

Cause 1 — two [mismatch] checks, both failing on the same assertion of the same message template:

1) mailmanager.deregister-member-is-idempotent-for-unknown-and-removed-addresses
   check_mailmanager_deregister_member_is_idempotent_for_unknown_and_removed_addresses.py line 38:
     assert err["Message"] == expected_message
     AssertionError: unexpected message 'Could not find Address in AddressList 6189d19113d8c0f7910a2dec89025cc1'
   expected_message = f"Could not find Address in AddressList {al_id} of AWS account {ctx.account}"
   Error.Code asserted as 'ResourceNotFoundException' and matched; only the message diverged.

2) mailmanager.member-lookup-requires-exact-post-normalization-casing
   check_mailmanager_member_lookup_requires_exact_post_normalization_casing.py line 53:
     assert message == expected_message
     AssertionError: unexpected message 'Could not find Address in AddressList 8d4a24a37afe1633910a2dec89025cc1'
   same expected template; Error.Code 'ResourceNotFoundException' matched.

Expected (fixed text + request/state values): 'Could not find Address in AddressList ' + <AddressListId from the request> + ' of AWS account ' + <account id owning the list>.
Actual: 'Could not find Address in AddressList ' + <AddressListId from the request>, with nothing after it.

Model evidence (smithy_query, GetMemberOfAddressList): input {AddressListId required, length 1..255, pattern ^[a-zA-Z0-9-]+$; Address required, length 3..320, sensitive}; output {Address required, CreatedTimestamp required}; errors AccessDeniedException/403, ResourceNotFoundException/404, ThrottlingException/429, ValidationException/400; @readonly. The model does not state message text, so the template above is from the recording.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. CreateAddressList with any valid name and keep the returned AddressListId.
3. Miss-after-deregistration path: RegisterMemberToAddressList(AddressListId, Address="<local>@example.com"), confirm GetMemberOfAddressList succeeds, then DeregisterMemberFromAddressList for that same Address, then call GetMemberOfAddressList(AddressListId, Address=<same address>).
   Expected: Error.Code 'ResourceNotFoundException' with Error.Message 'Could not find Address in AddressList <that AddressListId> of AWS account <the caller's account id>'.
   Observed: Error.Code 'ResourceNotFoundException' with Error.Message 'Could not find Address in AddressList <that AddressListId>' — the ' of AWS account <accountId>' clause is missing.
4. Case-variant path: RegisterMemberToAddressList with a mixed-case local part (e.g. "CaseProbe-xxx@example.com"), then GetMemberOfAddressList for the all-lowercase and the all-uppercase variant of that same address. Both are correctly reported absent with 'ResourceNotFoundException', and both messages diverge identically — truncated after the address-list id.
5. Replay to confirm: behavioral checks mailmanager.deregister-member-is-idempotent-for-unknown-and-removed-addresses and mailmanager.member-lookup-requires-exact-post-normalization-casing.
