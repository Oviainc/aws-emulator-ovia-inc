Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.add-tags-to-resource-on-a-creating-directory-rejected-client-exception ds__ec2.enable-ldaps-on-a-creating-directory-rejected-directory-unavailable ds__ec2.register-certificate-non-pem-body-rejected-invalid-certificate-before-stage-check ds__ec2.register-certificate-valid-pem-on-a-creating-directory-rejected-directory-unavailable ds__ec2.reset-user-password-on-a-creating-directory-rejected-directory-unavailable ds__ec2.create-microsoft-ad-password-containing-admin-rejected-invalid-parameter ds__ec2.create-microsoft-ad-with-one-subnet-rejected-invalid-parameter ds__ec2.create-microsoft-ad-with-two-subnets-in-one-az-rejected-client-exception
Issue-Key: ds__ec2.add-tags-to-resource-on-a-creating-directory-rejected-client-exception|run#5|AssertionError|assert d["Stage"] == "Creating", f"precondition: the directory should be Creating, is {d['Stage']!r}"
Issue-Key: ds__ec2.enable-ldaps-on-a-creating-directory-rejected-directory-unavailable|run#5|AssertionError|assert d["Stage"] == "Creating", f"precondition: the directory should be Creating, is {d['Stage']!r}"
Issue-Key: ds__ec2.register-certificate-non-pem-body-rejected-invalid-certificate-before-stage-check|run#5|AssertionError|assert d["Stage"] == "Creating", f"precondition: the directory should be Creating, is {d['Stage']!r}"
Issue-Key: ds__ec2.register-certificate-valid-pem-on-a-creating-directory-rejected-directory-unavailable|run#5|AssertionError|assert d["Stage"] == "Creating", f"precondition: the directory should be Creating, is {d['Stage']!r}"
Issue-Key: ds__ec2.reset-user-password-on-a-creating-directory-rejected-directory-unavailable|run#5|AssertionError|assert d["Stage"] == "Creating", f"precondition: the directory should be Creating, is {d['Stage']!r}"
Issue-Key: ds__ec2.create-microsoft-ad-password-containing-admin-rejected-invalid-parameter|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ds__ec2.create-microsoft-ad-with-one-subnet-rejected-invalid-parameter|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ds__ec2.create-microsoft-ad-with-two-subnets-in-one-az-rejected-client-exception|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Directory Service: a new Microsoft AD is instantly Active instead of passing through Creating, and CreateMicrosoftAD validates neither the password nor VpcSettings (4 causes)

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD

## Description
Four independent fidelity defects in AWS Directory Service's CreateMicrosoftAD/DescribeDirectories surface. All four must be closed before submitting. The service answers in all four cases (no scaffold stub involved).

Cause 1 of 4: a freshly created Microsoft AD directory is reported Active immediately.
AWS: CreateMicrosoftAD returns a DirectoryId whose DirectoryDescription.Stage is 'Requested' for a few seconds and then 'Creating' for the ~20+ minutes the domain controllers take to come up, reaching 'Active' only afterwards. Smithy: DescribeDirectories returns DirectoryDescriptions with a Stage member; the staged lifecycle itself is recorded behavior only.
Emulator: polling DescribeDirectories past 'Requested' already reports Stage 'Active'.
Why: every stage-gated behaviour becomes unreachable — the five checks here exist to assert that AddTagsToResource, EnableLDAPS, RegisterCertificate and ResetUserPassword are refused (ClientException / DirectoryUnavailableException, HTTP 400, e.g. 'The operation is not supported for directories in this state. Directory Status: Creating') while the directory is still Creating. A client that waits for Active, or that handles the in-between refusals, is never exercised and an emulator user cannot reproduce the production race at all.
Fix: directory lifecycle/stage progression in the CreateMicrosoftAD handler and the directory state it writes, plus the stage gate the dependent operations read.
Traces: ds__ec2.add-tags-to-resource-on-a-creating-directory-rejected-client-exception, ds__ec2.enable-ldaps-on-a-creating-directory-rejected-directory-unavailable, ds__ec2.register-certificate-non-pem-body-rejected-invalid-certificate-before-stage-check, ds__ec2.register-certificate-valid-pem-on-a-creating-directory-rejected-directory-unavailable, ds__ec2.reset-user-password-on-a-creating-directory-rejected-directory-unavailable.

Cause 2 of 4: CreateMicrosoftAD accepts an admin password that contains the word 'admin'.
AWS: refused with InvalidParameterException (HTTP 400). Smithy: CreateMicrosoftAD declares InvalidParameterException and Password carries a @pattern for the 8-64/complexity part; the 'must not contain the word "admin"' clause is a business rule the model does not express, and the recorded refusal message states it ('Invalid password. The password must be between 8 and 64 characters, not contain the word "admin", and must have at least one character from three of these four categories: lowercase, uppercase, numeric, and special characters.').
Emulator: the call succeeds and a directory is created, so there is no error at all.
Why: a client learns at AD-join time, or never, that its administrator credential was rejected by the real service.
Fix: password validation in the CreateMicrosoftAD handler.
Traces: ds__ec2.create-microsoft-ad-password-containing-admin-rejected-invalid-parameter.

Cause 3 of 4: CreateMicrosoftAD accepts VpcSettings naming only one subnet.
AWS: refused with InvalidParameterException (HTTP 400); a Microsoft AD needs exactly two subnets in two different AZs. Recorded message: 'Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones.'
Emulator: the call succeeds.
Why: the single-AZ topology the service refuses is created, so the emulator hands back a directory whose HA shape is impossible in AWS.
Fix: VpcSettings.SubnetIds cardinality validation in the CreateMicrosoftAD handler.
Traces: ds__ec2.create-microsoft-ad-with-one-subnet-rejected-invalid-parameter.

Cause 4 of 4: CreateMicrosoftAD accepts two subnets that are in the same Availability Zone.
AWS: refused with ClientException (HTTP 400) — note the different error shape from cause 3. Recorded message: 'Invalid subnet ID(s). The two subnets must be in different Availability Zones.'
Emulator: the call succeeds.
Why: same as cause 3, and a client that branches ClientException vs InvalidParameterException to tell 'wrong count' from 'wrong zones' cannot.
Fix: subnet-to-AZ resolution and validation in the CreateMicrosoftAD handler.
Traces: ds__ec2.create-microsoft-ad-with-two-subnets-in-one-az-rejected-client-exception.

## Evidence
Cause 1 — AssertionError: precondition: the directory should be Creating, is 'Active' (five checks, at `assert d["Stage"] == "Creating"` after polling past 'Requested' with a 180s budget).
Cause 2 — AssertionError: the call succeeded; real AWS rejects it, so there is no error code (expected code InvalidParameterException).
Cause 3 — AssertionError: the call succeeded; real AWS rejects it, so there is no error code (expected InvalidParameterException, message 'Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones.').
Cause 4 — AssertionError: the call succeeded; real AWS rejects it, so there is no error code (expected ClientException, message 'Invalid subnet ID(s). The two subnets must be in different Availability Zones.').

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repros, all with a VPC and subnets created first: (1) ds:CreateMicrosoftAD with two subnets in two AZs, then poll ds:DescribeDirectories and read DirectoryDescriptions[0].Stage; then call ds:AddTagsToResource / ds:EnableLDAPS / ds:RegisterCertificate / ds:ResetUserPassword while it should still be Creating. (2) ds:CreateMicrosoftAD Password='Admin12345!' (contains 'admin'). (3) ds:CreateMicrosoftAD VpcSettings={'VpcId':<vpc>,'SubnetIds':[<one subnet>]}. (4) ds:CreateMicrosoftAD VpcSettings with two subnets that share one AZ.
