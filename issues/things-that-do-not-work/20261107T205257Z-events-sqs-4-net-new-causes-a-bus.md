Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.bus-to-bus-forwarding-happens-once-and-preserves-the-envelope events__sqs.eventbridge-input-transformer-missing-path-not-delivered events__sqs.events-pattern-language-array-field-matches-if-any-element-matches events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path
Issue-Key: events__sqs.bus-to-bus-forwarding-happens-once-and-preserves-the-envelope|run#47|AssertionError|assert leaked == [], (
Issue-Key: events__sqs.eventbridge-input-transformer-missing-path-not-delivered|_emit_until#14|AssertionError|raise AssertionError(
Issue-Key: events__sqs.events-pattern-language-array-field-matches-if-any-element-matches|_assert_matrix#2|AssertionError|assert cold == [], (
Issue-Key: events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path|_assert_matrix#2|AssertionError|assert cold == [], (
Issue-Commit: 959ef406fada22402fcbc8404625fdf48db790f8

# events__sqs: 4 net-new causes — a bus-to-bus forward is re-forwarded a second hop, an InputTransformer-rendered body never matches, and the pattern matcher misses nested arrays and flattened object paths

> _Note: this issue was identified against the code base at commit `959ef406fada22402fcbc8404625fdf48db790f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents / PutRule / PutTargets

## Description
Four causes under the events__sqs heading, all un-owned. Related open issues that do not cover them: 20261029T182228Z-events-sqs-eventbridge-puttargets-accepts-conflicting-invalid-input-forms-the.md, 20261029T014025Z-events-3-net-new-behavioral-root-causes-an.md, 20261029T182228Z-cloudwatch-events-sqs-eventbridge-bus-to-bus-forwarding-ignores-the-destination.md.

Cause 1 of 4: an event that arrived on a bus via a bus-to-bus forward is forwarded again.
AWS: EventBridge forwards between buses in the same Region at most once — an event delivered to bus B by a rule on bus A is not forwarded on by B's own bus target, so a chain A->B->C produces exactly one hop per event and no event is ever seen twice on the same bus.
Emulator: a second hop occurs — a bus receives an event that had already been forwarded once (A->B), observed inside a window of 3x the measured per-hop latency, with the leaked envelope still carrying the original id and the forwarding bus in `resources`.
Failing check: events__sqs.bus-to-bus-forwarding-happens-once-and-preserves-the-envelope.

Cause 2 of 4: an InputTransformer target delivers a body that does not equal the rendered template, even when every input path resolves.
AWS: Smithy InputTransformer.InputPathsMap (JSON dot-notation paths, up to 100) + InputTemplate — with InputPathsMap {"who": "$.detail.who", "deep": "$.detail.user.name"} and InputTemplate '{"who": <who>, "deep": <deep>, "seen": "transformed"}', an event carrying detail {"who":"alice","user":{"name":"bob"}} is delivered as {"who":"alice","deep":"bob","seen":"transformed"}, and an event where neither path resolves is delivered as the same template with the EMPTY STRING substituted for each unresolvable placeholder — never the raw untransformed event.
Emulator: the control event's delivered body never equals the rendered template, while other, non-matching messages do arrive — "control event never delivered to <queue-url> after 10 PutEvents attempts (non-matching messages seen meanwhile: 10)" — so the positive leg cannot be established and the unresolvable-path behavior is unprovable. Hypothesis: the same renderer divergence the owned sibling checks show, where an unresolvable/absent path is rendered as JSON null or the literal text "null" instead of the empty string (owned: events.input-transformer-array-paths-render-elements-slices-and-null-exactly expected 'beyond': '' and got None; events__sqs.events-delivery-and-failure-input-transformer-substitution-forms-render-exactly expected 'missq': 'xy' and got 'xnully').
Failing check: events__sqs.eventbridge-input-transformer-missing-path-not-delivered.

Cause 3 of 4: an array-valued field is not matched when its matching element is inside a nested array.
AWS: a pattern {"detail":{"xs":["b"]}} matches when any element of the array field matches, including through nesting and for a scalar field — so events {"xs":["a","b","c"]}, {"xs":"b"} and {"xs":[["b"]]} all match, while {"xs":["a","c"]} and {"xs":[]} match nothing. The same holds for {"numeric":[">",10]} and {"prefix":"ab"} element patterns.
Emulator: the value rule delivered only the flat-array and scalar cases; the nested-array case was never delivered within 16 rounds ('value': ['one-of-three', 'scalar'] of the expected {'one-of-three','scalar','nested-array'}).
Failing check: events__sqs.events-pattern-language-array-field-matches-if-any-element-matches.

Cause 4 of 4: a nested-object pattern does not match through an array nor by its flattened dotted key.
AWS: {"detail":{"o":{"p":{"q":["v"]}}}} matches {"o":{"p":{"q":"v"}}} AND {"o":[{"p":{"q":"v"}}]} (the path is traversed through arrays), and {"detail":{"o":{"p":["v"]}}} matches {"o":{"p":"v"}} AND the literal flattened key {"o.p":"v"}; a leaf at another depth ({"o":{"q":"v"}}, {"q":"v"}) never matches.
Emulator: only the plain nested cases were delivered — the array-traversal case ('deep-in-array') and the dotted-key case ('dotted-key') never arrived within 16 rounds ('deep': ['deep'], 'mid': ['mid']).
Failing check: events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path.

Why: multi-hop duplication makes a bus mesh re-deliver and potentially amplify events a client expects exactly once (cause 1); a transformer-fed consumer receives a body it cannot parse against its contract (cause 2); and patterns that AWS matches are silently not matched, so rules that work on AWS deliver nothing against the emulator (causes 3 and 4).

Fix: the bus-to-bus forwarding path (it must not re-forward an event that arrived by a forward); the target input-transformer renderer (substitution of resolvable and unresolvable paths, bare and quoted placeholders); and the event-pattern matcher's path resolution (array flattening/traversal and flattened dotted-key names).

Traces: the four failing check ids in trace_ids.

## Evidence
Cause 1 — Expected: no bus sees an event twice; exactly one hop per event. Actual: AssertionError: bus <name> received an event that had ALREADY been forwarded once (A->B): EventBridge forwards between same-Region buses at most once, so this is a second hop that must not exist.
Cause 2 — Expected delivered body {"who":"alice","deep":"bob","seen":"transformed"} (and {"who":"","deep":"","seen":"transformed"} for the unresolvable event). Actual: AssertionError: control event never delivered to <queue-url> after 10 PutEvents attempts (non-matching messages seen meanwhile: 10).
Cause 3 — Expected value-rule matches {'one-of-three','scalar','nested-array'}. Actual: per-rule cases seen {'value': ['one-of-three','scalar'], 'numeric': ['one-number-over'], 'prefix': ['one-prefixed']}; 'nested-array' ({"xs":[["b"]]}) never delivered.
Cause 4 — Expected 'deep' -> {'deep','deep-in-array'} and 'mid' -> {'mid','dotted-key'}. Actual: per-rule cases seen {'deep': ['deep'], 'mid': ['mid']}.

## Steps to Reproduce
Cause 1: create buses A, B, C; a rule on A with a bus target B and a rule on B with a bus target C (and a sink on each); PutEvents once on A; watch every sink for the same event id arriving twice / a second hop.
Cause 2: PutRule {"source":[<src>]} with an SQS target whose InputTransformer is InputPathsMap {"who":"$.detail.who","deep":"$.detail.user.name"} and InputTemplate '{"who": <who>, "deep": <deep>, "seen": "transformed"}'; PutEvents a control event with both paths present and an event with neither; compare the delivered bodies.
Cause 3: one rule per pattern {"detail":{"xs":["b"]}}, {"detail":{"xs":[{"numeric":[">",10]}]}}, {"detail":{"xs":[{"prefix":"ab"}]}}, each with its own queue; PutEvents details {"xs":["a","b","c"]}, {"xs":"b"}, {"xs":[["b"]]}, {"xs":["a","c"]}, {"xs":[]}, {"xs":[1,20]}, {"xs":[1,2]}, {"xs":["zz","abz"]} in each round.
Cause 4: rules {"detail":{"o":{"p":{"q":["v"]}}}} and {"detail":{"o":{"p":["v"]}}}; PutEvents details {"o":{"p":{"q":"v"}}}, {"o":[{"p":{"q":"v"}}]}, {"o":{"p":"v"}}, {"o.p":"v"}, {"o":{"q":"v"}}, {"q":"v"}.
