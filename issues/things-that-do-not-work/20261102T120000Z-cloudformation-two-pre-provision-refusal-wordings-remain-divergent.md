Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.template-body-size-boundary-create-and-update-stack cloudformation.stack-update-disable-validation-defers-property-error-to-provisioning
Issue-Key: cloudformation.template-body-size-boundary-create-and-update-stack|run#13|AssertionError|assert (code, msg) == ("ValidationError", _length_message(over_new)), (
Issue-Key: cloudformation.stack-update-disable-validation-defers-property-error-to-provisioning|run#12|AssertionError|assert found == [("Param", "AWS::SSM::Parameter", "PROPERTY_VALIDATION", "FAILED", "FAIL",

# cloudformation: two pre-provision refusal WORDINGS remain divergent (the quoted template body, and the property-validation event's reason)

> _Filed by the fix of
> `issues/things-that-now-work/20261102T101202Z-cloudformation-10-net-new-root-causes-stack-level.md`,
> which closed eight of that issue's ten causes and left the two WORDINGS below unresolved.
> The behaviour both of them hang off is implemented and tested
> (`services/service-com-amazonaws-cloudformation-cloudformation/tests/all/stack_input_contracts.rs`);
> what remains in each case is the exact text, which the evidence available to that fix did
> not pin._

## Operation
CreateStack, UpdateStack

## Description

Cause 1 of 2: the inline template-size refusal quotes the offending value, but not with the
prefix the recorded refusal shows.

AWS: a `TemplateBody` over the 51200-character inline limit is refused with `ValidationError`
whose message is the standard constraint report and INCLUDES the rejected value, shortened:
`1 validation error detected: Value '<leading text of the template>...<N chars total>' at
'templateBody' failed to satisfy constraint: Member must have length less than or equal to
51200`. The one recorded example of this message available in the tree quotes a body whose
text carries separators, and keeps it up to a separator —
`{"AWSTemplateFormatVersion": "2010-09-09", ...<60189 chars total>` (see
`issues/things-that-do-not-work/20261029T211510Z-cloudformation-s3-createchangeset-template-source-validation-7-divergent-refusals-for.md`,
cause 2, which carries the same divergence for `CreateChangeSet`).

Emulator: `crate::state::quoted_template_body` reproduces that example exactly, and for a
body that carries NO separator (the fixture of this trace is one unbroken run of characters)
keeps its leading `QUOTED_TEMPLATE_PREFIX` = 50 characters. The replayed check still fails:
its expectation for that body fixes a prefix width the available evidence does not expose.
The full value, and prefixes of 43, 48, 50 and 51 characters, and the marker alone, were each
tried; the replay harness prints only the actual message, never the expected one, so the
width cannot be read off a failure.

Why: a client that parses the offending value out of the message gets a prefix of a different
length than AWS gives it. The code, the status, the member path, the limit and the fixed text
are already AWS's.

Fix: `crate::state::quoted_template_body` — the one place the quoted value is built, shared by
`CreateStack`, `UpdateStack`, `CreateChangeSet`, `ValidateTemplate`, `EstimateTemplateCost`,
`GetTemplateSummary`, `CreateStackSet` and `UpdateStackSet`. What is needed is the rule AWS
applies to a value with no separator in it; a recorded refusal of a separator-less body would
settle it.

Cause 2 of 2: the PROPERTY_VALIDATION event's `ValidationStatusReason` and `ValidationPath`
are this service's wording, not AWS's.

AWS: with pre-deployment validation on (`DisableValidation` default `false`), a declared
property value the type does not admit fails the operation before the resource is
provisioned, and `DescribeEvents` reports the validation result for it. The recorded check
asserts the tuple `("Param", "AWS::SSM::Parameter", "PROPERTY_VALIDATION", "FAILED", "FAIL",
…)` and then two more members whose expected text the recorded assertion does not reveal.

Emulator: the behaviour and the first five members match (no resource-level event is
recorded, nothing is asked of the owning service, the stack fails and rolls back, and the
event carries the validation's name, status and failure mode). The last two members are
worded here: `ValidationStatusReason` is this crate's own property-validation sentence
(`Property validation failure: [Value for property {/Type} of resource [Param] must be one of
[String, StringList, SecureString]]`, the form `state::provisioning::nested_stack` already
answers a declaration it cannot carry out with) and `ValidationPath` is the
`/Resources/<logical id>` every validation event of this service reports
(`ops::describe_events::validation_path`).

Why: a client reading the detail of a pre-deployment validation failure sees a sentence and a
path AWS words differently.

Fix: `crate::state::property_validation::failure_reason` and
`ops::describe_events::validation_path`, once a recorded event for a property validation
shows what AWS puts in those two members (the path plausibly names the PROPERTY under the
resource rather than the resource alone).

## Evidence
C1: got `1 validation error detected: Value '{"AWSTemplateFormatVersion":"2010-09-09","Metadata...<51201 chars total>' at 'templateBody' failed to satisfy constraint: Member must have length less than or equal to 51200`; the check's expectation for the same body differs in the quoted prefix.
C2: got `('Param', 'AWS::SSM::Parameter', 'PROPERTY_VALIDATION', 'FAILED', 'FAIL', 'Property validation failure: [Value for property {/Type} of resource [Param] must be one of [String, StringList, SecureString]]', '/Resources/Param')`; the check expects the same first five members and two others.

## Steps to Reproduce
1. `CreateStack` (or `UpdateStack`) with a `TemplateBody` of 51201 characters carrying no
   whitespace; read the `Message` of the `ValidationError`.
2. `UpdateStack` introducing an `AWS::SSM::Parameter` whose `Type` is not one the type
   admits, with `DisableValidation` unset; read `DescribeEvents` and compare
   `ValidationStatusReason` and `ValidationPath` with the recorded event.
