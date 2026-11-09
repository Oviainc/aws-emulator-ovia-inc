Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group
Issue-Key: elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group|_routed_request#20|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# An Elastic Beanstalk environment's CNAME is published as <name>.<region>.elasticbeanstalk.com, an address the emulator does not serve

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEnvironments

## Description
Cause 1 of 1: the environment announces an AWS production hostname instead of the address it is reached on.
AWS: EnvironmentDescription.CNAME is the URL a client requests to reach the running application, and the recorded run drives requests through it so the application's stdout reaches the derived log group (Smithy elasticbeanstalk EnvironmentDescription.CNAME).
Emulator: the CNAME is published as 'http://<env>-<id>.us-west-2.elasticbeanstalk.com/', whose host is not one of the emulator's own hosts — 'a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>'.
Why: the environment is unreachable for any client honouring the published CNAME, so streamed application logs cannot be produced or verified.
Fix: the CNAME/EndpointURL serializer in the elasticbeanstalk crate (publish the emulator's own host and port for the environment's listener).
Traces: elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group.

## Evidence
'simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-eb-log-env-<id>.us-west-2.elasticbeanstalk.com/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts'.

## Steps to Reproduce
Replay elasticbeanstalk__logs.eb-stream-logs-delivers-the-application-stdout-to-the-derived-log-group: create the environment, read its CNAME and request it.
