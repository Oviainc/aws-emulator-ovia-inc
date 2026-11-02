Issue-Source: trace-sweep-divergence
Issue-Trace: email-contact-list-and-contacts-management
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-contact-list-and-contacts-management
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|GetContactList|2xx->2xx|$.Tags
Issue-Key: sesv2|GetContactList|2xx->2xx|$headers.content-type
Issue-Key: sesv2|CreateContact|4xx->4xx|$.message
Issue-Key: sesv2|GetContact|2xx->2xx|$.TopicDefaultPreferences
Issue-Key: sesv2|GetContact|2xx->2xx|$headers.content-type
Issue-Key: sesv2|ListContacts|2xx->2xx|$.Contacts[*]
Issue-Key: sesv2|GetContact|4xx->4xx|$headers.content-type
Issue-Key: sesv2|DeleteContact|2xx->2xx|$headers.content-type
Issue-Key: sesv2|ListContactLists|2xx->2xx|$headers.content-type
Issue-Key: sesv2|DeleteContactList|2xx->2xx|$headers.content-type
Issue-Key: sesv2|DeleteContactList|4xx->4xx|$headers.content-type
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Contact plane fabricates TopicDefaultPreferences, drops empty Tags and mis-words refusals

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetContact

## Description
Ten net-new causes in the SESv2 contact-list/contact plane. Smithy: Contact {EmailAddress, TopicPreferences, TopicDefaultPreferences, UnsubscribeAll (default false), LastUpdatedTimestamp}; GetContactList returns Tags (TagList).

Cause 1 of 10: GetContactList omits the empty Tags list and uses the wrong content-type. AWS: a contact list created without tags is read back with Tags as an EMPTY LIST, and the response carries content-type application/x-amz-json-1.1 (recorded behavior only; the model declares restJson1). Emulator: omits Tags entirely and sends content-type application/json. Why: a client that reads response.Tags gets a null where AWS guarantees a list, so tag-diffing code breaks. Fix: the GetContactList serializer (always emit the TagList) and the service-wide response content-type. Traces: email-contact-list-and-contacts-management#task#2 (same cluster also at task#15).

Cause 2 of 10: CreateContact duplicate refusal message. AWS: 400 AlreadyExistsException with '<email-address> already exists in List.' — the fixed text is ' already exists in List.' and the single variable part is the EmailAddress from the request; the list name is NOT named. Emulator: 'Contact with email address: <email> already exists in list: <list-name>.', an emulator-authored sentence. Why: a client matching AWS's refusal text does not recognise the duplicate. Fix: the CreateContact handler's duplicate refusal message. Traces: email-contact-list-and-contacts-management#task#4.

Cause 3 of 10: GetContact returns a fabricated TopicDefaultPreferences. AWS: for a contact whose own TopicPreferences were supplied, TopicDefaultPreferences is ABSENT from the response (it is only populated where the list's defaults apply to the contact). Emulator: fills TopicDefaultPreferences with a copy of the list's topic defaults ([{TopicName, SubscriptionStatus}]), and sends the wrong content-type. Why: a client cannot distinguish a contact's explicit preference from an inherited default, so opt-in/opt-out reporting is wrong. Fix: the contact state/serializer for TopicDefaultPreferences in GetContact. Traces: email-contact-list-and-contacts-management#task#6 (same cluster also at task#10).

Cause 4 of 10: ListContacts carries the same fabricated TopicDefaultPreferences on every element. AWS: each Contacts[i] has TopicDefaultPreferences absent and only the contact's own TopicPreferences populated. Emulator: every element gains TopicDefaultPreferences [{TopicName, SubscriptionStatus: OPT_IN}] copied from the list defaults — including for a contact whose own preference is OPT_OUT. Why: a consumer paging contacts to compute who is opted in reads the list default as the contact's state. Fix: the ListContacts serializer / shared contact projection. Traces: email-contact-list-and-contacts-management#task#7.

Cause 5 of 10: content-type on the GetContact 404 (NotFoundException) response. AWS: application/x-amz-json-1.1; status and error code match. Emulator: application/json. Fix: service-wide response content-type. Traces: email-contact-list-and-contacts-management#task#8 (same cluster also at task#17).

Cause 6 of 10: content-type on the successful DeleteContact response (200). Traces: email-contact-list-and-contacts-management#task#11 (same cluster also at task#16).

Cause 7 of 10: ListContacts after UpdateContact still carries the fabricated TopicDefaultPreferences (the UnsubscribeAll=true path), i.e. the Cause 4 projection defect also applies to updated contacts. Traces: email-contact-list-and-contacts-management#task#12.

Cause 8 of 10: content-type on the successful ListContactLists response (200). Traces: email-contact-list-and-contacts-management#task#13.

Cause 9 of 10: content-type on the successful DeleteContactList response (200). Traces: email-contact-list-and-contacts-management#task#18.

Cause 10 of 10: content-type on the DeleteContactList 404 (NotFoundException) response. Traces: email-contact-list-and-contacts-management#task#19 (same cluster also at teardown#20).

The content-type causes share one root fix with the other SESv2 bundles filed in this sweep; they are listed here so this bundle's replay exercises them. No open issue covers the contact plane.

## Evidence
Replay of trace set simparity-wire-aws-v1.
Cause 1 (task#2, 200/200): $.Tags expected [], got absent; $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json'.
Cause 2 (task#4, 400/400): $.message expected 'sim2real-probe-sesv2-contact1-479efb51@example.com already exists in List.', got 'Contact with email address: sim2real-probe-sesv2-contact1-479efb51@example.com already exists in list: sim2real-probe-sesv2-contactlist-479efb51.'
Cause 3 (task#6, 200/200): $.TopicDefaultPreferences expected absent, got [{'TopicName': 'news', 'SubscriptionStatus': 'OPT_IN'}].
Cause 4 (task#7, 200/200): $.Contacts[0]/[1] differ only by TopicDefaultPreferences — expected None, got [{'TopicName': 'news', 'SubscriptionStatus': 'OPT_IN'}]; note Contacts[1]'s own TopicPreferences is OPT_OUT.
Cause 7 (task#12, 200/200): $.Contacts[0] expected TopicDefaultPreferences None, got [{'TopicName': 'news', 'SubscriptionStatus': 'OPT_IN'}] (UnsubscribeAll True on both sides).
Causes 5,6,8,9,10 (task#8, task#11, task#13, task#18, task#19): status matches (404/200/200/200/404); $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json'.

## Steps to Reproduce
1. Replay fixture email-contact-list-and-contacts-management (steps task#2, task#4, task#6, task#7, task#8, task#11, task#12, task#13, task#18, task#19).
2. CreateContactList with a topic 'news' having a default subscription status, then CreateContact with explicit TopicPreferences; GetContact and ListContacts and compare TopicDefaultPreferences.
3. CreateContact twice with the same EmailAddress and compare the 400 message.
4. GetContactList on an untagged list and check for Tags: [].
5. Inspect the content-type header on the DeleteContact/ListContactLists/DeleteContactList/GetContact-404 responses.
