Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.alias-reassignment-rejected ds.certificate-registration-commonname-round-trip ds.ds-trust-to-an-unresolvable-remote-domain-fails-and-leaves-no-trust ds.simple-ad-small-lifecycle-to-active-then-deleted
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.alias-reassignment-rejected|run#13|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r}"
Issue-Key: ds.certificate-registration-commonname-round-trip|run#23|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Key: ds.ds-trust-to-an-unresolvable-remote-domain-fails-and-leaves-no-trust|run#35|AssertionError|assert control.get("DnsIpAddrs"), (
Issue-Key: ds.simple-ad-small-lifecycle-to-active-then-deleted|run#37|AssertionError|assert desc.get("OsVersion"), (
Issue-Commit: 09ede02538826dd718336321255b799d2c318d86

# ds: 4 net-new causes — CreateAlias re-alias refusal is the wrong error class, the certificate-not-found message is the error shape's documentation sentence, and DescribeDirectories omits DnsIpAddrs and OsVersion

> _Note: this issue was identified against the code base at commit `09ede02538826dd718336321255b799d2c318d86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
FOUR net-new root causes in the Directory Service (ds) control plane, all grounded in failing checks the suite lists under `### ds` that are neither ALREADY FILED nor OWNED by an open issue. All four are in one service, so they are bundled here; a fixer must close ALL FOUR before submitting. Numbered causes below, each with its own failing check ids.

Cause 1 of 4: a second CreateAlias on a directory that already has an alias is refused with the wrong error class and an emulator-authored message.
AWS: Smithy — CreateAlias(DirectoryId ^d-[0-9a-f]{10}$ required, Alias AliasName len 1..62 required) models ClientException, EntityAlreadyExistsException, EntityDoesNotExistException, InvalidParameterException (all client, HTTP 400). Recorded behavior: AWS distinguishes the two collisions — an alias ANOTHER directory already reserved is EntityAlreadyExistsException, while a directory that already carries an alias of its own is 400 InvalidParameterException with the message template, whose text is entirely FIXED (no id, no alias interpolated): 'The directory in the request already has an alias. That alias must be deleted before a new alias can be created. ' followed by this service's ' : RequestId: <this call's request id>' suffix (the request id is the only variable part). The refusal changes nothing: DescribeDirectories still reports the first alias and the AccessUrl derived from it.
Emulator: answers 400 EntityAlreadyExistsException with 'Directory <directory id> already has the alias <alias> : RequestId: <id>' — wrong error class, an entirely different sentence, and it interpolates the directory id and the stored alias into a message AWS keeps constant (services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_alias.rs, the already-has-an-alias branch at lines 193-202, which reuses the same `already_exists` builder as the alias-taken branch at lines 187-192).
Why: the error class is the only thing telling a caller apart 'pick another alias, this one is taken by someone else' (retryable with a new value) from 'this directory can never be re-aliased' (not retryable at all); collapsing both into EntityAlreadyExistsException makes a client retry forever with new alias values, and no client matching AWS's fixed sentence matches.
Fix: CreateAlias' handler — the directory-already-has-an-alias branch must refuse with InvalidParameterException carrying the constant sentence, leaving the alias-taken branch on EntityAlreadyExistsException.
Traces: ds.alias-reassignment-rejected.

Cause 2 of 4: the certificate-not-found refusal serves the ERROR SHAPE'S DOCUMENTATION sentence instead of AWS's message, so neither id appears.
AWS: Smithy — DescribeCertificate and DeregisterCertificate both take DirectoryId (^d-[0-9a-f]{10}$) and CertificateId (^c-[0-9a-f]{10}$) and both model CertificateDoesNotExistException (client, HTTP 400). Recorded behavior: the message template is the fixed text 'Certificate ' <certificate id> ' does not exist for directoryId: ' <directory id>, then the ' : RequestId: <id>' suffix; the two ids are values taken from the request and the literal member label 'directoryId:' is fixed text.
Emulator: answers the right class with the shape's own DOC string — 'The certificate is not present in the system for describe or deregister activities. : RequestId: <id>'. The source says so explicitly: services/.../src/state.rs lines 4440-4450 define CERTIFICATE_NOT_PRESENT as that documentation sentence and comment that it is 'the error shape's OWN sentence … FIXED text, with neither id interpolated'. A shape's `doc` trait is prose describing the error class, not the message AWS sends.
Why: this is the refusal served on the documented deregister-then-read path, and it is the one place a caller learns WHICH certificate of WHICH directory is gone; clients that parse the id after the 'directoryId:' label, or match AWS's sentence to distinguish a deregistered certificate from an unsupported operation, get neither.
Note: open issue 20261102T102235Z-ds-eight-refusal-messages-are-emulator-authored-instead.md records this same refusal site (its cause 2) against a DIFFERENT observed text ('… does not exist for directory <id>'), carries only the ds.certificate-lifecycle-register-describe-deregister key, and does not cover the check below; the emulator now serves a third wording, so that entry must not be closed by matching its quoted 'actual' string.
Traces: ds.certificate-registration-commonname-round-trip.

Cause 3 of 4: DescribeDirectories never reports DnsIpAddrs, so an Active Managed Microsoft AD reports no domain controllers at all.
AWS: Smithy — DirectoryDescription$DnsIpAddrs is a list of IpAddr ('The IP addresses of the DNS servers for the directory. For a Simple AD or Microsoft AD directory, these are the IP addresses of the … directory servers'). Recorded behavior: once a Managed Microsoft AD reaches Stage=Active, DescribeDirectories reports DnsIpAddrs holding one address per subnet the create placed controllers in, each contained in that subnet's CIDR; the member is legitimately empty only while the directory is still Creating.
Emulator: the member is absent from every description — an Active MicrosoftAD comes back with no DnsIpAddrs key at all, so a check whose POSITIVE CONTROL is merely 'the directory still reports its domain controllers' fails even though the directory is Active.
Why: DnsIpAddrs is the only way a client learns where to point resolvers or conditional forwarders; everything downstream of it (trust creation against a remote domain, DNS reconciliation, operator runbooks) has nothing to read, and a caller that waits for the addresses to appear waits forever.
Fix: `Directory::to_description` in src/state.rs (lines ~2550-2577 leave dns_ip_addrs on `..DirectoryDescription::default()`) plus the controller placement that must derive one address per requested subnet as the directory goes Active.
Note: open issue 20261024T081028Z-ds-directory-service-a-microsoft-ad-never-provisions-its.md describes the same missing derivation reached through two ds__ec2 checks and carries only their replay keys; it does not cover the ds check below, which fails on the plain ds-only read.
Traces: ds.ds-trust-to-an-unresolvable-remote-domain-fails-and-leaves-no-trust.

Cause 4 of 4: DescribeDirectories never reports OsVersion for a Managed Microsoft AD.
AWS: Smithy — DirectoryDescription$OsVersion targets the OSVersion enum, whose values are SERVER_2012 and SERVER_2019 ('The operating system (OS) version of the directory'). Recorded behavior: a Managed Microsoft AD that has reached Active reports one of those two values; the member is not optional decoration for this directory type, it names the Windows Server version the directory runs on.
Emulator: the member is absent from the description (same `..default()` fall-through in `Directory::to_description`), so a caller reading OsVersion gets None.
Why: OsVersion is what a client keys OS-dependent behaviour on (schema extension compatibility, controller upgrade planning, compliance inventories); absent, it reads as 'unknown directory OS' for a directory AWS always labels.
Fix: `Directory::to_description` in src/state.rs together with the directory record — a created Managed Microsoft AD must carry and report an OSVersion enum value.
Traces: ds.simple-ad-small-lifecycle-to-active-then-deleted (this check's earlier AccessUrl cause was fixed under 20261107T045342Z-ds-7-net-new-causes-four-refusal-report.md, so the check now advances past AccessUrl and fails on OsVersion; that resolved report does not cover this member).

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against this checkout (Total 113/178 passed; ds 109/155, 46 failed). Request ids, directory/certificate ids, generated names, hosts and timestamps are per-run values and are not requirements.

Cause 1 — ds.alias-reassignment-rejected [mismatch]:
  AssertionError: expected InvalidParameterException, got 'EntityAlreadyExistsException'
  The check creates a Managed Microsoft AD, waits for Active, CreateAlias(<alias1>) (which succeeds and makes AccessUrl lead with the alias), then CreateAlias(<alias2>) on the same directory, expecting code InvalidParameterException and Message 'The directory in the request already has an alias. That alias must be deleted before a new alias can be created. : RequestId: <rid>', and finally that AccessUrl is unchanged.
  Emulator source: src/ops/create_alias.rs lines 193-202 — `already_exists(format!("Directory {directory_id} already has the alias {existing}"))`.

Cause 2 — ds.certificate-registration-commonname-round-trip [mismatch]:
  AssertionError: expected 'Certificate c-003ba5fe5a does not exist for directoryId: d-003ba5fe5a : RequestId: <rid>', got 'The certificate is not present in the system for describe or deregister activities. : RequestId: <rid>'
  The check RegisterCertificate's a self-signed ClientLDAPS PEM, waits for Registered, DeregisterCertificate's it, then polls DescribeCertificate until it answers CertificateDoesNotExistException (the code matches) and compares the message.
  Emulator source: src/state.rs lines 4440-4450, `pub const CERTIFICATE_NOT_PRESENT: &str = "The certificate is not present in the system for describe or deregister activities.";` — byte-identical to CertificateDoesNotExistException's `doc` trait in the Smithy model.

Cause 3 — ds.ds-trust-to-an-unresolvable-remote-domain-fails-and-leaves-no-trust [mismatch]:
  AssertionError: positive control failed: the directory must still report its domain controllers, got {'DirectoryId': 'd-003ba5fe5a', 'Name': '<generated>.example.com', 'ShortName': 'SIM2REAL', 'Size': 'Small', 'Edition': 'Standard', 'Alias': 'd-003ba5fe5a', 'AccessUrl': 'd-003ba5fe5a.awsapps.com', 'Stage': 'Active', 'Type': 'MicrosoftAD', 'VpcSettings': {'VpcId': 'vpc-…', 'SubnetIds': ['subnet-…','subnet-…']}, 'SsoEnabled': False, 'DesiredNumberOfDomainControllers': 2} — the description carries NO DnsIpAddrs key while Stage is Active.

Cause 4 — ds.simple-ad-small-lifecycle-to-active-then-deleted [mismatch]:
  AssertionError: a MicrosoftAD runs on a concrete Windows Server version and must report it, got None (the check reads DescribeDirectories after the directory reaches Active).
  Model: OSVersion enum = [SERVER_2012, SERVER_2019]; DirectoryDescription$OsVersion targets it.
  Emulator source for causes 3 and 4: src/state.rs `Directory::to_description` (lines ~2550-2577) sets 15 members and closes with `..DirectoryDescription::default()`, so dns_ip_addrs and os_version are always None; its own comment says the DNS addresses are 'left absent rather than invented'.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite(); the emulator serves ds (109/155 checks pass), so all four are fidelity divergences rather than a bootstrap gap.
2. Cause 1: CreateMicrosoftAD, wait for Stage=Active, CreateAlias(DirectoryId=<dir>, Alias=<alias1>), then CreateAlias(DirectoryId=<dir>, Alias=<alias2>) — expect 400 InvalidParameterException 'The directory in the request already has an alias. That alias must be deleted before a new alias can be created. '; observe EntityAlreadyExistsException 'Directory <dir> already has the alias <alias1>'. Confirm DescribeDirectories' AccessUrl still leads with <alias1>.
3. Cause 2: RegisterCertificate(DirectoryId=<active dir>, Type=ClientLDAPS, CertificateData=<self-signed PEM>), wait for State=Registered, DeregisterCertificate it, then DescribeCertificate(DirectoryId=<dir>, CertificateId=<id>) — expect CertificateDoesNotExistException 'Certificate <cert id> does not exist for directoryId: <dir id>'; observe the shape's documentation sentence.
4. Cause 3: CreateMicrosoftAD naming two subnets in different AZs, wait for Stage=Active, then DescribeDirectories(DirectoryIds=[<dir>]) — expect DnsIpAddrs with one address per chosen subnet, each inside that subnet's CIDR; observe the member absent.
5. Cause 4: same directory and read — expect OsVersion to be one of SERVER_2012 / SERVER_2019; observe it absent.
6. Re-replay ds.alias-reassignment-rejected, ds.certificate-registration-commonname-round-trip, ds.ds-trust-to-an-unresolvable-remote-domain-fails-and-leaves-no-trust and ds.simple-ad-small-lifecycle-to-active-then-deleted.
