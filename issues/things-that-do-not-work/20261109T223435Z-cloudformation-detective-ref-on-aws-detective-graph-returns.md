Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__detective.deploy-detective-graph-round-trip-and-delete
Issue-Key: cloudformation__detective.deploy-detective-graph-round-trip-and-delete|run#13|AssertionError|assert graph_arn == graph_arn_att, (
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__detective: Ref on AWS::Detective::Graph returns a generated name instead of the graph ARN

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::Detective::Graph Ref does not equal the graph ARN.
AWS: Ref on AWS::Detective::Graph returns the graph's ARN, the same value Fn::GetAtt <Logical>.Arn yields; the two are identical for this type.
Emulator: Ref returns the generated physical name `<stack-name>-<logical-id>-<suffix>` while Fn::GetAtt .Arn returns `arn:aws:detective:<region>:<account>:graph/<that same generated name>` — so Ref is not an ARN and the two disagree.
Why: a template that passes the graph Ref to another resource, an output or an IAM policy hands downstream code a value Detective does not accept as a graph identifier.
Fix: the AWS::Detective::Graph resource handler's physical-resource-id/Ref resolution.
Traces: the failing check below.

## Evidence
AssertionError: Ref 'sim2real-behav-detective-graph-<id>-Graph-<suffix>' does not equal Fn::GetAtt Graph.Arn 'arn:aws:detective:us-west-2:000000000000:graph/sim2real-behav-detective-graph-<id>-Graph-<suffix>'. Expected Ref == the graph ARN.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Deploy a stack with an AWS::Detective::Graph resource and two outputs: {Ref Graph} and {Fn::GetAtt [Graph, Arn]}.
3. Compare the two output values.
