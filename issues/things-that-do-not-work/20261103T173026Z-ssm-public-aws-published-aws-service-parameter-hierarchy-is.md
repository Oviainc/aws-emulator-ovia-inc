Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-catalog-get-parameters-by-path
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-catalog-get-parameters-by-path ssm-public-parameter-ami-lookup-and-errors
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetParametersByPath|2xx->2xx|$.Parameters[*]
Issue-Key: ssm|GetParameters|2xx->2xx|$.InvalidParameters[*]
Issue-Key: ssm|GetParameters|2xx->2xx|$.Parameters[*]
Issue-Commit: 133bbb645e9618d24e6206d20cee2b832cb9c7e9

# Public AWS-published /aws/service parameter hierarchy is empty for path and batch reads

> _Note: this issue was identified against the code base at commit `133bbb645e9618d24e6206d20cee2b832cb9c7e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParametersByPath

## Description
The emulator serves no AWS-published public parameters under /aws/service, so hierarchy enumeration returns an empty page and batch reads report every public name as invalid. The replay reports these clusters as unowned; the existing open issues 20261030T030533Z-ssm-getparameter-cannot-resolve-aws-published-aws-parameters.md, 20261103T154947Z-ssm-getparameter-cannot-resolve-aws-published-public-aws.md and 20261029T204746Z-pcs-the-aws-published-public-ssm-parameter-namespace-aws.md cover the single-name GetParameter read and the /aws/service/pcs namespace, not the GetParametersByPath enumeration or the GetParameters batch split below. 4 CAUSES. The requirement is the SHAPE of the catalog, not any recorded value: each entry is a Parameter with Name = the full public path, Type 'String', DataType 'text', a Version and LastModifiedDate, and ARN of the form arn:aws:ssm:<region>::parameter<name> (note the empty account segment that public parameters use). Specific image ids and versions are per-environment and are not asserted.

Cause 1 of 4 - GetParametersByPath over /aws/service/ami-amazon-linux-latest returns nothing. AWS: 200 with the Amazon Linux AMI alias parameters of that hierarchy (the recording enumerates 57, e.g. /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-6.1-arm64, .../al2023-ami-kernel-6.1-x86_64, .../amzn-ami-hvm-x86_64-s3), each shaped as above (recorded behavior only - the public catalog is service-published data, not modelled; Smithy: GetParametersByPath returns ParameterList). Emulator: 200 with $.Parameters empty. Why: everything that discovers a current AMI through the Parameter Store alias hierarchy - EC2 RunInstances with a resolve:ssm reference, CloudFormation SSM parameter types, launch templates - finds no candidates and cannot proceed. Fix: seed the public /aws/service hierarchy into the SSM service state and resolve it from the GetParametersByPath handler (including the account-less ARN form). Traces: ssm-catalog-get-parameters-by-path#task#1.

Cause 2 of 4 - the same empty result for /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended. AWS: 200 with that hierarchy's parameters (ecs_agent_version, ecs_runtime_version, image_id, image_name, image_version and siblings - 8 in the recording). Emulator: empty $.Parameters. Why: ECS-optimized AMI discovery cannot resolve. Fix: as cause 1. Traces: ssm-catalog-get-parameters-by-path#task#2.

Cause 3 of 4 - the same empty result for /aws/service/bottlerocket/aws-ecs-2/x86_64/latest. AWS: 200 with image_id and image_version for that path. Emulator: empty $.Parameters. Fix: as cause 1. Traces: ssm-catalog-get-parameters-by-path#task#3.

Cause 4 of 4 - GetParameters mis-splits public names between Parameters and InvalidParameters. AWS: 200 where every resolvable public name appears in $.Parameters (shaped as above) and ONLY the unresolvable name appears in $.InvalidParameters (Smithy: GetParameters output Parameters: ParameterList, InvalidParameters: ParameterNameList; the operation does not error for unknown names). Emulator: 200 with $.Parameters empty and ALL submitted names, resolvable or not, listed in $.InvalidParameters. Why: a batch resolver cannot distinguish a genuinely bad name from one the emulator simply has no data for, and treats the whole batch as a caller error. Fix: GetParameters resolution against the public hierarchy, and the Parameters/InvalidParameters partition in its handler. Traces: ssm-public-parameter-ami-lookup-and-errors#task#3.

## Evidence
Replay of simparity-wire-aws-v1. Representative (ssm-catalog-get-parameters-by-path#task#1, GetParametersByPath): expected 200 / actual 200, but all 57 expected entries reported 'missing' - e.g. expected {'ARN': 'arn:aws:ssm:us-west-2::parameter/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-6.1-arm64', 'DataType': 'text', 'Name': '/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-6.1-arm64', 'Type': 'String', 'Value': '<shaped-not-asserted>', 'Version': '<shaped-not-asserted>', 'LastModifiedDate': '<shaped-not-asserted>'}, got None. ssm-catalog-get-parameters-by-path#task#2: 8 entries under /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended all missing. #task#3: 2 entries under /aws/service/bottlerocket/aws-ecs-2/x86_64/latest missing. ssm-public-parameter-ami-lookup-and-errors#task#3 (GetParameters): $.Parameters[0] and [1] missing (al2023-ami-kernel-default-x86_64 and amzn2-ami-hvm-x86_64-gp2), while $.InvalidParameters came back with all three submitted names instead of only '/aws/service/ami-amazon-linux-latest/sim2real-probe-missing-ami-family'. Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (GetParametersByPath, GetParameters, Parameter).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1. 2. Representative: fixture ssm-catalog-get-parameters-by-path step task#1 - GetParametersByPath Path='/aws/service/ami-amazon-linux-latest' Recursive=true; observe $.Parameters is empty. 3. Steps task#2 and task#3 of the same fixture for the ECS-optimized and Bottlerocket hierarchies. 4. Fixture ssm-public-parameter-ami-lookup-and-errors step task#3 - GetParameters with two real public AMI alias names plus one nonexistent one; observe all three in InvalidParameters.
