Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-m7g-az-and-subnet-discovery-mistakes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-m7g-az-and-subnet-discovery-mistakes ec2-dvpc-traj-default-vpc-id-agrees-across-account-attributes-and-filter ec2-dvpc-traj-filter-and-attribute-name-validation ec2-trajwire-deb-deb-instance-type-casing-and-absent-ami ec2-trajwire-deb-deb-key-pair-named-before-created ec2-vpce-traj-endpoint-lookup-mistakes ec2-vvd-traj-vpc-tag-spec-mistakes-and-filter-lookup ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypeOfferings|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeInstanceTypeOfferings|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeAccountAttributes|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeAccountAttributes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|RunInstances|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateKeyPair|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateVpcEndpoint|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateVpcEndpoint|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateVpc|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateVpc|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|ImportKeyPair|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|ImportKeyPair|4xx->4xx|$.Response.Errors.Error.Message
Issue-Related: 20261029T200817Z-ec2-parameter-limit-refusals-report-emulator-authored-limits.md
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 invalid enum and attribute values are refused as ValidationError instead of AWS parameter errors

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypeOfferings

## Description
When a request carries a value outside a modeled enum (or an unknown attribute name), the emulator answers its generic constraint error — code 'ValidationError' with the Smithy-style sentence '1 validation error detected: Value '<v>' at '<path>' failed to satisfy constraint: Member must satisfy enum value set: [...]' — and dumps the whole enum. AWS refuses these with a per-operation parameter error whose code is InvalidParameterValue / InvalidParameter / InvalidVpcEndpointType and whose message names only the offending value and, where applicable, the short accepted set. Status (400) is right in every case; code and message are wrong in the same way, so it is one defect in the enum-rejection path.

Cause 1 of 7: DescribeInstanceTypeOfferings LocationType.
AWS: Smithy: DescribeInstanceTypeOfferings.LocationType targets enum LocationType — an unmodeled value is refused 400 Code 'InvalidParameterValue', Message 'Invalid value for location type: az. Must be one of [availability-zone, availability-zone-id, outpost, region]'. Fixed text 'Invalid value for location type: ' + the request's value + '. Must be one of [' + AWS's accepted set in AWS's order + ']'.
Emulator: 400 Code 'ValidationError', Message "1 validation error detected: Value 'az' at 'locationType' failed to satisfy constraint: Member must satisfy enum value set: [region, availability-zone, availability-zone-id, outpost]".
Why: a client that keys on InvalidParameterValue sees an unknown code; the emulator's message also presents a different ordering and a different sentence, so operator-facing diagnostics do not match AWS.
Fix: the operation's validation in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instance_type_offerings.rs must own the enum refusal rather than deferring to the generic Smithy constraint reporter.
Traces: ec2-m7g-az-and-subnet-discovery-mistakes#task#4.

Cause 2 of 7: DescribeAccountAttributes with an unknown attribute name.
AWS: recorded behavior only — 400 Code 'InvalidParameter', Message "The property 'default_vpc' is not valid" (fixed text 'The property ' + quoted request value + ' is not valid').
Emulator: 400 Code 'ValidationError', "1 validation error detected: Value 'default_vpc' at 'attributeNames.1.member' failed to satisfy constraint: Member must satisfy enum value set: [supported-platforms, default-vpc]".
Why: wrong code class (InvalidParameter vs ValidationError) and a message that leaks an index path AWS never emits.
Fix: validation in describe_account_attributes.rs.
Traces: ec2-dvpc-traj-default-vpc-id-agrees-across-account-attributes-and-filter#task#1 (cluster also covers ec2-dvpc-traj-filter-and-attribute-name-validation#task#3).

Cause 3 of 7: RunInstances with a mis-cased instance type.
AWS: recorded behavior only — 400 Code 'InvalidParameterValue', Message "Invalid value 'T3.micro' for InstanceType." (fixed text 'Invalid value ' + quoted request value + ' for InstanceType.').
Emulator: 400 Code 'ValidationError' and a message that enumerates the entire InstanceType enum (hundreds of values).
Why: AWS instance types are case-sensitive and the recorded refusal is short and actionable; the emulator's multi-kilobyte enum dump plus wrong code breaks both programmatic handling and log hygiene.
Fix: validation in run_instances.rs.
Traces: ec2-trajwire-deb-deb-instance-type-casing-and-absent-ami#task#3 (cluster also covers task#5).

Cause 4 of 7: CreateKeyPair with an invalid key-name value.
AWS: recorded behavior only — 400 Code 'InvalidParameterValue'.
Emulator: 400 Code 'ValidationError' (message matched).
Why: the code is the part clients switch on, so the refusal is unrecognisable even though its text is right.
Fix: validation in create_key_pair.rs.
Traces: ec2-trajwire-deb-deb-key-pair-named-before-created#task#4.

Cause 5 of 7: CreateVpcEndpoint with a lower-cased endpoint type.
AWS: Smithy: CreateVpcEndpoint.VpcEndpointType targets enum VpcEndpointType — a value outside it is refused 400 Code 'InvalidVpcEndpointType', Message 'Unrecognized VPC endpoint type interface, expected one of [Gateway, Interface, GatewayLoadBalancer]' (fixed text + the request's value + AWS's accepted set).
Emulator: 400 Code 'ValidationError', "1 validation error detected: Value 'interface' at 'vpcEndpointType' failed to satisfy constraint: Member must satisfy enum value set: [Interface, Gateway, GatewayLoadBalancer, Resource, ServiceNetwork, Tunnel]".
Why: AWS has a dedicated code for this refusal; the emulator hides it and advertises endpoint types AWS does not accept here.
Fix: validation in create_vpc_endpoint.rs.
Traces: ec2-vpce-traj-endpoint-lookup-mistakes#task#14 (cluster also covers task#15/#16/#17).

Cause 6 of 7: CreateVpc tag specification with an unsupported resourceType.
AWS: recorded behavior only — 400 Code 'InvalidParameter', Message "'VPC' is not a valid taggable resource type for this operation." (fixed text with the request's resourceType quoted).
Emulator: 400 Code 'ValidationError' and a message enumerating every ResourceType value.
Why: AWS's rule is per-operation (which resource types that operation may tag), not a global enum membership test, so the emulator both misclassifies and misexplains the refusal.
Fix: tag-specification validation in create_vpc.rs.
Traces: ec2-vvd-traj-vpc-tag-spec-mistakes-and-filter-lookup#task#2 (cluster also covers task#3).

Cause 7 of 7: ImportKeyPair with non-ASCII public key material.
AWS: recorded behavior only — 400 Code 'InvalidParameterValue', Message 'Value for parameter PublicKeyMaterial is invalid. Character sets beyond ASCII are not supported.'
Emulator: 400 Code 'MalformedInput', Message 'invalid base64: failed to decode base64'.
Why: the emulator reports a transport-level decode failure for a content rule AWS states in terms of the PublicKeyMaterial parameter, so the caller cannot tell that the key bytes, not the encoding, are at fault.
Fix: validation in import_key_pair.rs.
Traces: ec2-asgssh-traj-key-pair-import-format-and-lookup-mistakes#task#4 (cluster also covers task#5/#6).

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-m7g-az-and-subnet-discovery-mistakes task#4 — expected 400 / 'InvalidParameterValue' / 'Invalid value for location type: az. Must be one of [availability-zone, availability-zone-id, outpost, region]'; actual 400 / 'ValidationError' / "1 validation error detected: Value 'az' at 'locationType' failed to satisfy constraint: Member must satisfy enum value set: [region, availability-zone, availability-zone-id, outpost]". Same substitution of the generic ValidationError for DescribeAccountAttributes (InvalidParameter), RunInstances (InvalidParameterValue), CreateKeyPair (InvalidParameterValue), CreateVpcEndpoint (InvalidVpcEndpointType), CreateVpc tag specs (InvalidParameter) and ImportKeyPair (InvalidParameterValue vs MalformedInput).

## Steps to Reproduce
1. Call DescribeInstanceTypeOfferings with LocationType=az against the emulator (fixture ec2-m7g-az-and-subnet-discovery-mistakes, step task#4).
2. Observe Code 'ValidationError' and the enum-set sentence instead of 'InvalidParameterValue' and AWS's 'Invalid value for location type: ...' sentence.
3. Repeat for each fixture#step in trace_ids: DescribeAccountAttributes with attribute-name default_vpc, RunInstances with InstanceType=T3.micro, CreateKeyPair with an invalid name, CreateVpcEndpoint with VpcEndpointType=interface, CreateVpc with TagSpecification.ResourceType=VPC, ImportKeyPair with non-ASCII key material.
