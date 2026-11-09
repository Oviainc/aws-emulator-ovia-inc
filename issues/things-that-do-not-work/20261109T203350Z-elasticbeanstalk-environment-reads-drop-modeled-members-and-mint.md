Issue-Source: trace-sweep-divergence
Issue-Trace: elasticbeanstalk-python-environment-runs-uploaded-version
Issue-Step: task#24
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticbeanstalk-python-environment-runs-uploaded-version
Issue-Shape-Id: com.amazonaws.elasticbeanstalk#AWSElasticBeanstalkService
Issue-Key: elasticbeanstalk|DescribeEnvironments|2xx->2xx|$.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.Alerts
Issue-Key: elasticbeanstalk|DescribeEnvironments|2xx->2xx|$.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.CNAME
Issue-Key: elasticbeanstalk|DescribeEnvironments|2xx->2xx|$.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.EndpointURL
Issue-Key: elasticbeanstalk|DescribeEnvironments|2xx->2xx|$.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.EnvironmentId
Issue-Key: elasticbeanstalk|DescribeEnvironments|2xx->2xx|$.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.EnvironmentLinks
Issue-Key: elasticbeanstalk|DescribeEnvironments|2xx->2xx|$.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.MinCapacityEnabled
Issue-Key: elasticbeanstalk|DescribeEnvironmentResources|2xx->2xx|$.DescribeEnvironmentResourcesResponse.DescribeEnvironmentResourcesResult.EnvironmentResources.AutoScalingGroups.member.Name
Issue-Key: elasticbeanstalk|DescribeEnvironmentResources|2xx->2xx|$.DescribeEnvironmentResourcesResponse.DescribeEnvironmentResourcesResult.EnvironmentResources.RuntimeSources
Issue-Commit: 622e0f603a4886f3f4d7e4980155df7d3d956f5b

# elasticbeanstalk environment reads drop modeled members and mint off-shape ids and an unserved CNAME (2 causes)

> _Note: this issue was identified against the code base at commit `622e0f603a4886f3f4d7e4980155df7d3d956f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEnvironments

## Description
Two net-new causes in how the emulator describes a running Elastic Beanstalk environment. Both are read paths over an environment that was created and deployed in the same fixture, so a client that creates an environment and then reads it back cannot learn how to reach it or what is deployed in it.

Cause 1 of 2: DescribeEnvironments returns an EnvironmentDescription that omits modeled members and publishes an off-shape EnvironmentId and an unserved CNAME (fixture elasticbeanstalk-python-environment-runs-uploaded-version, step task#24; same cluster also at task#25).
AWS: Smithy EnvironmentDescription (com.amazonaws.elasticbeanstalk#EnvironmentDescription) models EndpointURL — "For load-balanced, autoscaling environments, the URL to the LoadBalancer. For single-instance environments, the IP address of the instance" — CNAME (target DNSCname, @length 1..255, "The URL to the CNAME for this environment"), EnvironmentLinks (list, "A list of links to other environments in the same group") and EnvironmentId ("The ID of this environment"). For MinCapacityEnabled and Alerts the on-disk model says nothing: recorded behavior only — the recording shows AWS emitting MinCapacityEnabled as the literal boolean text `false` for an environment with no minimum-capacity configuration, and Alerts as a present-but-empty list element. For a Ready, load-balanced environment AWS emits a non-empty EndpointURL, an EnvironmentId of the form `e-` followed by a short lowercase alphanumeric token (Hypothesis: 10 characters; the recorded assertion is a shape assertion, not a literal value), and a CNAME of the form `<cname-prefix>.<eba-subdomain-label>.<region>.elasticbeanstalk.com`, where the prefix comes from the request's CNAMEPrefix (defaulting to the environment name) and the `eba-...` label is the per-environment subdomain Beanstalk allocates. All of those variable parts come from the request and the emulator's own state; only the `.<region>.elasticbeanstalk.com` tail and the structure of the name are fixed.
Emulator: the response element carries no MinCapacityEnabled, no Alerts, no EndpointURL and no EnvironmentLinks at all (all four reported `missing`); EnvironmentId fails its shape assertion (emitted `e-` plus a 13-character hex token); and CNAME is emitted as `<cname-prefix>-<environment-id>.<region>.elasticbeanstalk.com` — the environment id is appended to the prefix and the `eba-<token>` subdomain label is absent. The replay additionally flags that CNAME `host_not_own`: the host the emulator publishes is not a host the emulator itself serves, so resolving/ following it reaches nothing.
Why: a client that reads an environment back gets no endpoint to call — EndpointURL is absent and the CNAME names a host nobody answers — so every "deploy then hit the app" workflow dead-ends. SDK/CLI consumers that render or branch on MinCapacityEnabled, Alerts or EnvironmentLinks see absent members where AWS always sends them, and tooling that parses or re-uses the environment id against the documented `e-...` shape rejects the emulator's id.
Fix: the DescribeEnvironments response builder/serializer for EnvironmentDescription (populate EndpointURL, EnvironmentLinks, MinCapacityEnabled and Alerts) plus the environment state that mints EnvironmentId and the environment CNAME, which must compose the `eba-` subdomain label and must publish a host this emulator actually answers on.
Traces: elasticbeanstalk-python-environment-runs-uploaded-version#task#24, elasticbeanstalk-python-environment-runs-uploaded-version#task#25.

Cause 2 of 2: DescribeEnvironmentResources omits EnvironmentResources.RuntimeSources entirely, and the Auto Scaling group name it reports is off-shape (fixture elasticbeanstalk-python-environment-runs-uploaded-version, step task#26).
AWS: for RuntimeSources the on-disk model's EnvironmentResourceDescription does not declare the member — recorded behavior only: AWS returns a RuntimeSources list whose element carries Parameter `AppSource` and a Versions list naming the application and the version label currently deployed in the environment (in the recording, the ApplicationName and VersionLabel created earlier in the same fixture — those two values are taken from the request/state, the `AppSource` parameter name is the fixed part). Smithy EnvironmentResourceDescription.AutoScalingGroups (AutoScalingGroupList, "The AutoScalingGroups used by this environment") is modeled, and the recorded group Name is asserted by shape: Beanstalk names it `awseb-<environment-id>-stack-AWSEBAutoScalingGroup-<token>`, so the embedded environment id must itself be a valid Beanstalk environment id.
Emulator: RuntimeSources is absent from EnvironmentResources (`missing`), so nothing in the resources view states which application version the environment is running; and AutoScalingGroups.member.Name fails its shape assertion because it embeds the same off-shape environment id produced in Cause 1 (`awseb-e-<13 hex chars>-stack-AWSEBAutoScalingGroup-<token>`).
Why: a client reconciling "which version is live in this environment" from DescribeEnvironmentResources finds no AppSource runtime source at all, and tooling that parses the generated Auto Scaling group name to recover the environment id gets a token that does not match AWS's id shape.
Fix: the DescribeEnvironmentResources handler/response builder (emit the AppSource RuntimeSources entry from the environment's deployed-version state) and the shared environment-id minting in environment state that the group name is derived from.
Traces: elasticbeanstalk-python-environment-runs-uploaded-version#task#26.

Neither cause is covered by the open issue 20261029T235806Z-elasticbeanstalk-elastic-beanstalk-platform-branch-catalog-returns-wrong-branch.md, which owns the ListPlatformBranches catalog divergences in this same corpus and nothing about environment reads.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 79.6%, 32 mismatched steps) — both clusters report verdict=mismatch with expected_status 200 and actual_status 200, i.e. the status is right and the body diverges.

Cause 1 — fixture elasticbeanstalk-python-environment-runs-uploaded-version, step task#24 (affected: task#24, task#25), operation DescribeEnvironments:
  $.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.MinCapacityEnabled: missing (expected 'false', got None)
  $.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.Alerts: missing (expected '', got None)
  $.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.EndpointURL: missing (expected '<shaped-not-asserted>', got None)
  $.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.EnvironmentLinks: missing (expected '', got None)
  $.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.EnvironmentId: value_mismatch (expected '<shaped-not-asserted>', got 'e-4f6f7d14402d4')
  $.DescribeEnvironmentsResponse.DescribeEnvironmentsResult.Environments.member.CNAME: host_not_own (expected '<cname-prefix>.eba-<token>.<region>.elasticbeanstalk.com' — recorded 'sim2real-probe-eb-32666f4d.eba-m2wv44gs.us-west-2.elasticbeanstalk.com' — got '<cname-prefix>-<environment-id>.<region>.elasticbeanstalk.com', recorded 'sim2real-probe-eb-32666f4d-4f6f7d14402d4.us-west-2.elasticbeanstalk.com')
The prefix, token, region and ids above are per-run values; the requirement is the member being present and the name's structure plus the host being one the emulator serves.

Cause 2 — fixture elasticbeanstalk-python-environment-runs-uploaded-version, step task#26, operation DescribeEnvironmentResources:
  $.DescribeEnvironmentResourcesResponse.DescribeEnvironmentResourcesResult.EnvironmentResources.RuntimeSources: missing (expected {'member': {'Versions': {'member': {'VersionLabel': <version label from state>, 'ApplicationName': <application name from state>}}, 'Parameter': 'AppSource'}}, got None)
  $.DescribeEnvironmentResourcesResponse.DescribeEnvironmentResourcesResult.EnvironmentResources.AutoScalingGroups.member.Name: value_mismatch (expected '<shaped-not-asserted>', got 'awseb-e-4f6f7d14402d4-stack-AWSEBAutoScalingGroup-E6ECA87DE2F84')

Model evidence: smithy_query operation=DescribeEnvironments/DescribeEnvironmentResources and shape=EnvironmentDescription/EnvironmentResourceDescription/EnvironmentId on services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/smithy-model/com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice.json: EndpointURL, CNAME (DNSCname, @length 1..255), EnvironmentLinks and EnvironmentId are modeled members of EnvironmentDescription; AutoScalingGroups is a modeled member of EnvironmentResourceDescription; MinCapacityEnabled, Alerts and RuntimeSources are not in the on-disk model (recorded behavior only).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: replay fixture elasticbeanstalk-python-environment-runs-uploaded-version step task#24 (and task#25) — create an application, upload/create an application version, create an environment from it, wait for it to become Ready, then call DescribeEnvironments for that environment. Inspect the Environments.member element: MinCapacityEnabled, Alerts, EndpointURL and EnvironmentLinks are absent, EnvironmentId is `e-` + 13 hex characters instead of AWS's short lowercase alphanumeric id, and CNAME is `<prefix>-<environment-id>.<region>.elasticbeanstalk.com` with no `eba-<token>` label — and no listener answers that host.
3. Cause 2: replay the same fixture's step task#26 — call DescribeEnvironmentResources for the same environment. EnvironmentResources contains no RuntimeSources (no AppSource entry naming the deployed ApplicationName/VersionLabel), and AutoScalingGroups.member.Name embeds the off-shape environment id.
4. Both steps report verdict=mismatch with expected_status 200 / actual_status 200.
