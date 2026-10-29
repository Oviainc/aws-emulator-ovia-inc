Issue-Source: trace-sweep-divergence
Issue-Trace: iam-list-access-keys-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-list-access-keys-lifecycle iam-service-specific-credential-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|CreateAccessKey|2xx->2xx|$.CreateAccessKeyResponse.CreateAccessKeyResult.AccessKey.AccessKeyId
Issue-Key: iam|ListAccessKeys|2xx->2xx|$.ListAccessKeysResponse.ListAccessKeysResult.AccessKeyMetadata.member.AccessKeyId
Issue-Key: iam|CreateServiceSpecificCredential|2xx->2xx|$.CreateServiceSpecificCredentialResponse.CreateServiceSpecificCredentialResult.ServiceSpecificCredential.ServiceSpecificCredentialId
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Generated IAM credential identifiers do not use the AWS identifier prefixes

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessKey

## Description
Cause 1 of 2 - CreateAccessKey mints an access key id that is not an AWS access key id. AWS (recorded behavior only for the prefix; Smithy constrains only the generic shape - accessKeyIdType length 16..128, pattern ^[\\w]+$): a long-term IAM user access key id is a 20-character uppercase-alphanumeric string beginning with the AKIA resource-type prefix, which is how every consumer recognises it; the recorded value is redacted by the corpus precisely because it matches the AWS access-key form. The same id is then echoed by ListAccessKeys. Emulator: it mints ids beginning with the literal 'EMULATOR' (e.g. a 20-character 'EMULATORAK...' value), which does not carry an AWS resource-type prefix. Why: tooling that classifies a credential by its prefix (AKIA long-term vs ASIA temporary), secret scanners, and policies or logs keyed on the prefix all misclassify the key; a client cannot tell a long-term user key from any other string. Fix: the access-key id generator in the CreateAccessKey handler - emit the AKIA prefix plus 16 random uppercase-alphanumeric characters, and persist that id so ListAccessKeys returns it.
Traces: iam-list-access-keys-lifecycle#task#3 (CreateAccessKey), #task#4 (ListAccessKeys echoing the same id).

Cause 2 of 2 - CreateServiceSpecificCredential mints an id without the service-specific-credential prefix. AWS (recorded behavior only): a ServiceSpecificCredentialId is an uppercase-alphanumeric id beginning with the ACCA resource-type prefix. Emulator: it reuses the same 'EMULATORAK...' generator as the access-key path, so the id carries the access-key shape instead of the service-specific-credential one. Why: a client that stores or routes credentials by resource-type prefix cannot distinguish a service-specific credential from an access key, and the id does not round-trip against anything that validates the prefix. Fix: give the service-specific-credential handler its own id generator with the ACCA prefix.
Traces: iam-service-specific-credential-lifecycle#task#3.

Note: open issue 20261028T040721Z-cognito-identity-cognito-idp-iam-getcredentialsforidentity-vends-an reports the same 'EMUL'-prefixed generator on the STS/Cognito temporary-credential path; it does not cover the IAM CreateAccessKey or CreateServiceSpecificCredential paths and carries no replay key for these fixture steps.

## Evidence
Cause 1: expected (recorded AWS) CreateAccessKeyResult/AccessKey/AccessKeyId matching the AWS access-key form (the corpus stores it redacted as '[REDACTED:aws-access-key]', i.e. it matched the AKIA 20-character detector); actual 'EMULATORAKVFQCSSH6MQ'. ListAccessKeys reports the same mismatch on AccessKeyMetadata/member/AccessKeyId.
Cause 2: expected CreateServiceSpecificCredentialResult/ServiceSpecificCredential/ServiceSpecificCredentialId with the ACCA prefix ('ACCAEXAMPLEPLACEHOLDER1' in the recording); actual 'EMULATORAKHIMMBOUHNL'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-list-access-keys-lifecycle#task#3: CreateAccessKey for a user and read AccessKey/AccessKeyId - it begins 'EMULATOR', not 'AKIA'. #task#4 ListAccessKeys echoes it.
3. Replay iam-service-specific-credential-lifecycle#task#3: CreateServiceSpecificCredential - ServiceSpecificCredentialId begins 'EMULATOR', not 'ACCA'.
