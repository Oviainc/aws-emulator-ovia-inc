Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__servicediscovery.deploy-http-namespace-service-instance-lifecycle cloudformation__servicediscovery.deploy-public-dns-namespace-lifecycle cloudformation__servicediscovery.deploy-service-dnsconfig-in-http-namespace-rejected
Issue-Key: cloudformation__servicediscovery.deploy-http-namespace-service-instance-lifecycle|run#13|AssertionError|assert listed == [(namespace_id, outputs["NamespaceArn"], "HTTP")], (
Issue-Key: cloudformation__servicediscovery.deploy-public-dns-namespace-lifecycle|stack_outputs#8|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__servicediscovery.deploy-service-dnsconfig-in-http-namespace-rejected|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__servicediscovery: 3 net-new causes — an HTTP namespace a stack creates is not listed, Outputs omit OutputValue, and a DnsConfig in an HTTP namespace is accepted

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three distinct root causes. Each is numbered with its own failing check id; all three must be closed before this issue is done.

Cause 1 of 3: the HTTP namespace a stack reports CREATE_COMPLETE is not listed by Cloud Map.
AWS: after CREATE_COMPLETE, ListNamespaces returns the namespace the stack created, findable by the name/id the stack's Ref reports, and its services and instances are then registrable.
Emulator: ListNamespaces returns an empty list for the stack's Ref.
Why: a client that creates a Cloud Map namespace with CloudFormation and then registers services or instances into it finds no namespace to register against.
Fix: the AWS::ServiceDiscovery::HttpNamespace resource handler must create the namespace in Cloud Map state.
Traces: cloudformation__servicediscovery.deploy-http-namespace-service-instance-lifecycle.

Cause 2 of 3: DescribeStacks returns Outputs entries with no OutputValue.
AWS: Smithy: cloudformation#Output carries OutputKey, OutputValue, Description and ExportName; a resolved output always carries OutputValue.
Emulator: the Output entries carry OutputKey only, so reading output["OutputValue"] raises KeyError: 'OutputValue'.
Why: the public-DNS namespace id the stack publishes cannot be read back.
Fix: the DescribeStacks serializer / stack-output resolution in the CloudFormation state.
Traces: cloudformation__servicediscovery.deploy-public-dns-namespace-lifecycle.

Cause 3 of 3: an AWS::ServiceDiscovery::Service that declares a DnsConfig inside an HTTP namespace is accepted and the stack reaches CREATE_COMPLETE.
AWS: Cloud Map refuses a service with a DnsConfig when its namespace is an HTTP namespace (InvalidInput), so CloudFormation marks the resource CREATE_FAILED and the stack ROLLBACK_COMPLETE.
Emulator: the stack ends `CREATE_COMPLETE`.
Why: a client believes it created a DNS-discoverable service in a namespace that has no DNS, so discovery silently never resolves.
Fix: the AWS::ServiceDiscovery::Service handler's namespace-type validation / propagation of the Cloud Map refusal.
Traces: cloudformation__servicediscovery.deploy-service-dnsconfig-in-http-namespace-rejected.

## Evidence
Cause 1: AssertionError: ListNamespaces [] for Ref 'sim2real-behav-cfnhttp-<id>'.
Cause 2: KeyError: 'OutputValue'.
Cause 3: AssertionError: rejected stack ended 'CREATE_COMPLETE' — expected ROLLBACK_COMPLETE.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: deploy AWS::ServiceDiscovery::HttpNamespace and call ListNamespaces.
3. Cause 2: deploy AWS::ServiceDiscovery::PublicDnsNamespace with an output and read Outputs[0]['OutputValue'].
4. Cause 3: deploy an AWS::ServiceDiscovery::Service with a DnsConfig whose NamespaceId is the HTTP namespace, and watch the final stack status.
