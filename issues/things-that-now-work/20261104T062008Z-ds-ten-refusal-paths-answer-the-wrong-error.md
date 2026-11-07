Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.add-ip-routes-duplicate-cidr-rejected ds.add-ip-routes-overlapping-cidr-rejected ds.concurrent-add-ip-routes-conflict-detected ds.share-directory-already-shared-rejected ds.concurrent-share-directory-race-conflict ds.concurrent-update-domain-controllers-limit-exceeded ds.conditional-forwarder-duplicate-domain-rejected ds.create-trust-duplicate-name-conflict ds.ldaps-toggle-server-managed-state ds.directory-data-access-toggle-server-managed ds.get-snapshot-limits-reflects-snapshot-count ds.visibility-directory-unavailable-during-creation-blocked ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched
Issue-Key: ds.add-ip-routes-duplicate-cidr-rejected|run#12|AssertionError|assert code == "ClientException" and message == expected, (
Issue-Key: ds.add-ip-routes-overlapping-cidr-rejected|run#9|AssertionError|assert error.get("Code") == "ClientException", f"unexpected error: {error}"
Issue-Key: ds.concurrent-add-ip-routes-conflict-detected|run#12|AssertionError|assert loser.get("Code") == "ClientException", f"unexpected loser error code {loser!r}"
Issue-Key: ds.share-directory-already-shared-rejected|_assert_error#8|AssertionError|assert got_message == message, f"expected message {message!r}, got {got_message!r}"
Issue-Key: ds.concurrent-share-directory-race-conflict|run#16|AssertionError|assert failures[0][2] == (
Issue-Key: ds.concurrent-update-domain-controllers-limit-exceeded|run#16|AssertionError|assert solo_message == expected_message, f"expected {expected_message!r}, got {solo_message!r}"
Issue-Key: ds.conditional-forwarder-duplicate-domain-rejected|run#8|AssertionError|assert message.startswith(f"Failed to create zone {remote_domain} on server ") and message.endswith(
Issue-Key: ds.create-trust-duplicate-name-conflict|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ds.ldaps-toggle-server-managed-state|run#8|AssertionError|assert message == expected, (
Issue-Key: ds.directory-data-access-toggle-server-managed|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ds.get-snapshot-limits-reflects-snapshot-count|run#10|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: ds.visibility-directory-unavailable-during-creation-blocked|run#10|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got: {code}"
Issue-Key: ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched|run#14|AssertionError|assert refused.get("Message") == f"User already exists in directory {directory_id}.", (
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ds: ten refusal paths answer the wrong error class or emulator-authored message (AddIpRoutes, ShareDirectory, UpdateNumberOfDomainControllers, CreateConditionalForwarder, CreateTrust, EnableLDAPS, EnableDirectoryDataAccess, DescribeSnapshots, ds-data CreateUser)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddIpRoutes

## Description
Behavioral replay of simparity-behavioral-aws-v1 against this checkout left a large set of ds failures after skipping every failure the report marks ALREADY FILED (scaffold stubs) or OWNED by an open issue. This issue bundles TEN net-new root causes that share one shape: the directory-service refusal the emulator builds is NOT the refusal AWS builds — the error CLASS is wrong, or the message is the emulator's own prose instead of AWS's fixed text. A fixer must close ALL TEN causes before submitting. Related open issues that cover the SAME ground for OTHER checks but carry no replay key for the checks listed here (so they do not cover them): 20261102T102235Z-ds-eight-refusal-messages-are-emulator-authored-instead.md, 20261029T200700Z-ds-10-net-new-refusal-classification-and-message.md, 20261024T081601Z-ds-directory-service-control-plane-7-behavioral-divergences-error.md.

In every message template below, the fixed text is a requirement verbatim; the variable parts (directory ids, account ids, CIDRs, domain names, limits, user names) come from the request or from stored state, and the trailing ' : RequestId: <id>' carries a per-run id that is NOT itself a requirement.

Cause 1 of 10: AddIpRoutes refuses an already-routed CIDR with the wrong error class and its own message.
AWS: Smithy AddIpRoutes models ClientException, EntityAlreadyExistsException, EntityDoesNotExistException, InvalidParameterException, IpRouteLimitExceededException (all client, HTTP 400). Recorded behavior: re-adding a CIDR the directory already routes is refused HTTP 400 ClientException with the message template 'The IP route(s) [<cidrs, comma-separated>] already exist. If you want to recreate them, please delete the existing routes first.' — fixed text exactly as written, variable part the offending CIDR list taken from the request.
Emulator: HTTP 400 EntityAlreadyExistsException with 'Directory <directory id> already routes the address block <cidr>' — wrong class and entirely different fixed text.
Why: a client branching on ClientException to distinguish 'route already present' from a genuine entity-already-exists conflict takes the wrong branch, and no client can parse the emulator's wording to recover the offending CIDR list.
Fix: the AddIpRoutes handler's duplicate/overlap detection branch and the ds refusal-message builder it calls.
Traces: ds.add-ip-routes-duplicate-cidr-rejected, ds.add-ip-routes-overlapping-cidr-rejected, ds.concurrent-add-ip-routes-conflict-detected.

Cause 2 of 10: ShareDirectory's already-shared refusal message is re-worded and leaks the shared-directory id.
AWS: Smithy ShareDirectory models DirectoryAlreadySharedException (client, 400) with members Message and RequestId. Recorded behavior: a second HANDSHAKE share to an account the directory is already shared with is refused with message template 'Directory <directory id> is already shared with Account <account id>.' — note the capital 'Account', the terminating period, and that NO shared-directory id appears. Variable parts: the directory id and the target account id from the request/state.
Emulator: 'Directory <directory id> is already shared with account <account id> as <shared directory id>' — lowercase 'account', no terminating period, and an extra ' as d-xxxxxxxxxx' clause AWS never emits.
Why: exact-match assertions and message-parsing clients break, and the emulator discloses an id AWS withholds in this refusal.
Fix: the ShareDirectory handler's already-shared branch / its message builder.
Traces: ds.share-directory-already-shared-rejected, ds.concurrent-share-directory-race-conflict.

Cause 3 of 10: UpdateNumberOfDomainControllers' over-limit refusal message is the emulator's own prose.
AWS: Smithy UpdateNumberOfDomainControllers models DomainControllerLimitExceededException (client, 400); DesiredNumber has @range min 2. Recorded behavior: a request above the per-directory ceiling is refused with 'Domain controllers limit exceeded. A maximum of <limit> domain controllers may be requested.' — fixed text as written, variable part the limit (20 in the recording); the requested number does NOT appear.
Emulator: 'The maximum allowed number of domain controllers per directory was exceeded. The default limit per directory is 20 domain controllers; 21 were requested.'
Why: a client that matches the documented sentence to surface a quota error never matches; the emulator also reports the request value AWS omits.
Fix: the UpdateNumberOfDomainControllers handler's limit branch and its message builder.
Traces: ds.concurrent-update-domain-controllers-limit-exceeded.

Cause 4 of 10: CreateConditionalForwarder's duplicate-zone refusal message is not AWS's zone-exists text.
AWS: Smithy CreateConditionalForwarder models EntityAlreadyExistsException (client, 400). Recorded behavior: a second forwarder for a RemoteDomainName that already has one is refused EntityAlreadyExistsException with the template 'Failed to create zone <remote domain> on server <the directory DNS server>. A conditional forwarding zone already exists for that name. ' (the recording shows the two spaces before the ' : RequestId: ' separator). Variable parts: the remote domain from the request and the directory's DNS server address from state.
Emulator: "Directory <directory id> already has a conditional forwarder for the remote domain '<domain>'" — different fixed text, quotes the domain, and never names the DNS server.
Why: a caller cannot tell from the emulator which DNS server rejected the zone, and text assertions fail.
Fix: the CreateConditionalForwarder handler's duplicate branch and its message builder.
Traces: ds.conditional-forwarder-duplicate-domain-rejected.

Cause 5 of 10: CreateTrust's duplicate-trust refusal message is re-worded.
AWS: recorded behavior — a second trust for the same remote domain is refused with 'Trust already exists for directory: <directory id> and remote domain: <remote domain>.' (fixed text with the two labelled fields and the terminating period).
Emulator: "Directory <directory id> already has a trust relationship with the remote domain '<domain>'".
Why: message-matching clients and runbooks that key on AWS's labelled form fail.
Fix: the CreateTrust handler's duplicate-detection branch and its message builder.
Traces: ds.create-trust-duplicate-name-conflict.

Cause 6 of 10: EnableLDAPS' no-certificate refusal message is the emulator's own prose.
AWS: Smithy EnableLDAPS models NoAvailableCertificateException (client, 400). Recorded behavior: enabling LDAPS with no registered ClientLDAPS certificate is refused with 'Cannot enable LDAPS since there is no valid certificate available.' — fixed text, no variable part other than the request-id suffix.
Emulator: 'LDAPS cannot be enabled for directory <directory id> because at least one valid ClientLDAPS certificate must be registered in the system'.
Why: the operator-facing text differs from AWS's and exact-match assertions fail.
Fix: the EnableLDAPS handler's certificate-availability branch and its message builder.
Traces: ds.ldaps-toggle-server-managed-state.

Cause 7 of 10: EnableDirectoryDataAccess' already-enabled refusal message is re-worded.
AWS: Smithy EnableDirectoryDataAccess models DirectoryInDesiredStateException (client, 400), documented 'The directory is already updated to desired update type settings.' Recorded behavior: enabling data access on a directory that already has it is refused with 'Directory <directory id> is already in the desired state.' — fixed text, variable part the directory id.
Emulator: 'Directory <directory id> data access is already Enabled'.
Why: the refusal cannot be matched against AWS's text, and the emulator's wording implies a state name AWS does not report here.
Fix: the EnableDirectoryDataAccess handler's in-desired-state branch and its message builder.
Traces: ds.directory-data-access-toggle-server-managed.

Cause 8 of 10: DescribeSnapshots classifies a deleted snapshot id as EntityDoesNotExistException where AWS answers InvalidParameterException.
AWS: Smithy DescribeSnapshots models ClientException, EntityDoesNotExistException, InvalidNextTokenException and InvalidParameterException (all client, 400). Recorded behavior: describing a snapshot id that was deleted answers HTTP 400 InvalidParameterException (NOT EntityDoesNotExistException), and the snapshot is absent from the directory-scoped listing.
Emulator: HTTP 400 EntityDoesNotExistException.
Why: a client that treats EntityDoesNotExistException as 'the DIRECTORY is gone' (its meaning on the other ds read paths) aborts a snapshot workflow where AWS only tells it the snapshot id is no longer a valid parameter.
Fix: DescribeSnapshots' snapshot-resolution branch in its handler — classify an unresolvable snapshot id as InvalidParameterException.
Traces: ds.get-snapshot-limits-reflects-snapshot-count.

Cause 9 of 10: a mutation against a directory still in Creating is refused DirectoryUnavailableException where AWS answers InvalidParameterException.
AWS: Smithy CreateConditionalForwarder models BOTH DirectoryUnavailableException and InvalidParameterException (client, 400). Recorded behavior: CreateConditionalForwarder issued while DescribeDirectories still reports the directory as Creating is refused InvalidParameterException whose message states the directory is not in Active or Impaired state; the identical call succeeds once the directory is Active.
Emulator: refuses with DirectoryUnavailableException.
Why: the two classes mean different things to a caller — InvalidParameterException with the state sentence tells it to wait for Active, while DirectoryUnavailableException reads as a transient service condition and is retried blindly.
Fix: the stage guard shared by the conditional-forwarder mutation path — classify a not-yet-Active directory as InvalidParameterException naming the required states.
Traces: ds.visibility-directory-unavailable-during-creation-blocked.

Cause 10 of 10: ds-data CreateUser's duplicate-SAMAccountName ConflictException message is re-worded.
AWS: Smithy (directoryservicedata) CreateUser models ConflictException (client, HTTP 409), documented for 'a resource that conflicts with an existing object'. Recorded behavior: creating a user whose SAMAccountName already exists is refused 409 ConflictException with 'User already exists in directory <directory id>.' — fixed text, variable part the directory id; the ORIGINAL user object must be left byte-identical.
Emulator: '<sam account name> already names an object in directory <directory id>' — different fixed text, and it names the SAMAccountName instead of the object class AWS reports.
Why: a client that matches AWS's sentence to distinguish a duplicate USER from any other conflict (e.g. a group-membership conflict) cannot classify the refusal.
Fix: the ds-data CreateUser handler's conflict branch and its ConflictException message builder.
Traces: ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched.

## Evidence
Suite simparity-behavioral-aws-v1 against this checkout: Total 56/178 passed (31.5%); 95 mismatched, 27 errored, 0 skipped, 0 unloadable. Per service: ds 53/155 (34%, 102 failed), ds__ec2 3/21 (14%, 18 failed), ds__logs 0/1, ds__sns 0/1.

Expected vs actual, per cause (verbatim from the replay report):
1) ds.add-ip-routes-duplicate-cidr-rejected — expected ClientException 'The IP route(s) [203.0.113.0/24] already exist. If you want to recreate them, please delete the existing routes first. : RequestId: <id>'; got code='EntityAlreadyExistsException' message='Directory d-003ba5fe5a already routes the address block 203.0.113.0/24 : RequestId: <id>'. Same pair on ds.add-ip-routes-overlapping-cidr-rejected ('unexpected error: {Code: EntityAlreadyExistsException ...}') and ds.concurrent-add-ip-routes-conflict-detected ('unexpected loser error code ... EntityAlreadyExistsException', CIDR 198.51.100.0/24).
2) ds.share-directory-already-shared-rejected — expected 'Directory d-003ba5fe5a is already shared with Account 111122223333. : RequestId: <id>'; got 'Directory d-003ba5fe5a is already shared with account 111122223333 as d-007740c861 : RequestId: <id>'. ds.concurrent-share-directory-race-conflict reports the identical message shape for target 111122223333.
3) ds.concurrent-update-domain-controllers-limit-exceeded — expected 'Domain controllers limit exceeded. A maximum of 20 domain controllers may be requested. : RequestId: <id>'; got 'The maximum allowed number of domain controllers per directory was exceeded. The default limit per directory is 20 domain controllers; 21 were requested. : RequestId: <id>'.
4) ds.conditional-forwarder-duplicate-domain-rejected — the check asserts message.startswith('Failed to create zone <domain> on server ') and endswith('. A conditional forwarding zone already exists for that name.  : RequestId: <id>'); got "Directory d-003ba5fe5a already has a conditional forwarder for the remote domain 'dup-forwarder-<run>.test' : RequestId: <id>".
5) ds.create-trust-duplicate-name-conflict — expected 'Trust already exists for directory: d-003ba5fe5a and remote domain: <domain>. : RequestId: <id>'; got "Directory d-003ba5fe5a already has a trust relationship with the remote domain '<domain>' : RequestId: <id>".
6) ds.ldaps-toggle-server-managed-state — expected 'Cannot enable LDAPS since there is no valid certificate available. : RequestId: <id>'; got 'LDAPS cannot be enabled for directory d-003ba5fe5a because at least one valid ClientLDAPS certificate must be registered in the system : RequestId: <id>'.
7) ds.directory-data-access-toggle-server-managed — expected 'Directory d-003ba5fe5a is already in the desired state. : RequestId: <id>'; got 'Directory d-003ba5fe5a data access is already Enabled : RequestId: <id>'.
8) ds.get-snapshot-limits-reflects-snapshot-count — 'expected InvalidParameterException for deleted snapshot s-003ba5fe5a, got EntityDoesNotExistException'.
9) ds.visibility-directory-unavailable-during-creation-blocked — 'expected InvalidParameterException, got: DirectoryUnavailableException' (CreateConditionalForwarder while the directory is Creating; check docstring states AWS's message says the directory 'is not in Active or Impaired state').
10) ds.ds-data-duplicate-sam-account-name-rejected-and-original-untouched — check asserts Message == 'User already exists in directory d-003ba5fe5a.'; got 'sim2realOrig already names an object in directory d-003ba5fe5a'.

Model evidence (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): AddIpRoutes errors = ClientException, DirectoryUnavailableException, EntityAlreadyExistsException, EntityDoesNotExistException, InvalidParameterException, IpRouteLimitExceededException (400) + ServiceException (500); ShareDirectory errors include DirectoryAlreadySharedException and InvalidTargetException; UpdateNumberOfDomainControllers errors include DomainControllerLimitExceededException, DesiredNumber @range min 2; CreateConditionalForwarder errors include EntityAlreadyExistsException, DirectoryUnavailableException AND InvalidParameterException; EnableLDAPS errors include NoAvailableCertificateException; EnableDirectoryDataAccess errors include DirectoryInDesiredStateException ('The directory is already updated to desired update type settings.'); DescribeSnapshots errors include InvalidParameterException and EntityDoesNotExistException. ds-data model (services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/com-amazonaws-directoryservicedata-directoryservicedata.json): CreateUser errors = AccessDeniedException (403), ConflictException (409), DirectoryUnavailableException (400), ValidationException (400), ThrottlingException (429), InternalServerException (500). The exact message texts are recorded behavior (the checks measured them on real AWS), not model text.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite() to build, boot and replay against the emulator.
2. Cause 1: create an Active Managed Microsoft AD, AddIpRoutes(['203.0.113.0/24']), then AddIpRoutes the same CIDR again — expect 400 ClientException 'The IP route(s) [203.0.113.0/24] already exist. ...'; observe EntityAlreadyExistsException with 'Directory <id> already routes the address block ...'.
3. Cause 2: ShareDirectory(HANDSHAKE) to account 111122223333, then repeat the identical call — expect 'Directory <id> is already shared with Account 111122223333.'; observe the lowercase 'account' form with an extra ' as d-...' clause.
4. Cause 3: UpdateNumberOfDomainControllers(DesiredNumber=21) on an Active directory — expect 'Domain controllers limit exceeded. A maximum of 20 domain controllers may be requested.'.
5. Cause 4: CreateConditionalForwarder for a remote domain twice — expect 'Failed to create zone <domain> on server <dns ip>. A conditional forwarding zone already exists for that name.'.
6. Cause 5: CreateTrust twice for the same RemoteDomainName — expect 'Trust already exists for directory: <id> and remote domain: <domain>.'.
7. Cause 6: EnableLDAPS(Type='Client') with no registered certificate — expect 'Cannot enable LDAPS since there is no valid certificate available.'.
8. Cause 7: EnableDirectoryDataAccess twice — expect DirectoryInDesiredStateException 'Directory <id> is already in the desired state.'.
9. Cause 8: CreateSnapshot, DeleteSnapshot, then DescribeSnapshots(SnapshotIds=[<deleted id>]) — expect InvalidParameterException; observe EntityDoesNotExistException.
10. Cause 9: CreateMicrosoftAD and, while DescribeDirectories still reports Creating, call CreateConditionalForwarder — expect InvalidParameterException naming the Active/Impaired requirement; observe DirectoryUnavailableException.
11. Cause 10: enable directory data access, ds-data CreateUser(SAMAccountName='X'), then CreateUser('X') again — expect 409 ConflictException 'User already exists in directory <id>.'.
12. After fixing, re-replay every check id in trace_ids.
