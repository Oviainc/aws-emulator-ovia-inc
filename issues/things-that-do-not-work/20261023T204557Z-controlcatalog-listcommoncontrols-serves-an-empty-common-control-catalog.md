Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-common-controls
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-common-controls
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListCommonControls|2xx->2xx|$.CommonControls[*]
Issue-Commit: 9b28682be3d06b0e1ca8f56d8f0bc182f6004def

# controlcatalog ListCommonControls serves an empty common-control catalog for every domain and objective filter

> _Note: this issue was identified against the code base at commit `9b28682be3d06b0e1ca8f56d8f0bc182f6004def`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCommonControls

## Description
ListCommonControls answers 200 but never returns any CommonControls entry, for any filter the corpus exercises. The divergence replays as nine distinct response clusters (one per objective/domain group), each missing a different recorded slice of the catalog; all nine must be closed. The resolved issue 20261021T234107Z-controlcatalog-listcommoncontrols-returns-an-empty-commoncontrols-list-for-every covers the same surface but is CLOSED and therefore does not cover these replays.

AWS (applies to every cause below): ListCommonControls (Smithy: restJson1, POST /common-controls; output ListCommonControlsResponse; optional CommonControlFilter.Objectives, which per the model 'can specify one objective ARN at a time') returns a paginated CommonControls list of CommonControlSummary records. CommonControlSummary requires Arn (pattern ^arn:(aws(?:[-a-z]*)?):controlcatalog:::common-control/[0-9a-z]+$, @length min 41), Name, Description, Domain {Arn, Name}, Objective {Arn, Name}, CreateTime and LastUpdateTime. The exact catalog membership is recorded behavior only.
Emulator (applies to every cause): HTTP 200 with the CommonControls array holding none of the expected records ($.CommonControls[0..n] missing).
Why (applies to every cause): the common-control layer of the ontology is empty, so control-mapping and compliance tooling that resolves a control to its common controls gets a successful-but-empty answer and silently reports no coverage.
Fix (applies to every cause): the ListCommonControls handler and the seeded common-control catalog/ontology state it reads.

Cause 1 of 9: the 'Change management' objective group — expects common-control/1nxqoydhsjdyboc4frsrrmev 'Change request and approval', /8xy67kb7kojr38ozqpmrm60d7 'Change deployment and automation', /2o50zyyv219ir9r7brjq0y37f and one more, all under domain eyrstdrz8kmo28x5j8vgal6au. Trace: controlcatalog-catalog-list-common-controls#task#1 (8 steps).
Cause 2 of 9: the 'Code reviews and testing' objective group — expects /blplx8cx7l3qy5jd6eryax5e7 'Development testing', /euni3089ej2htlwl6tyjpoy79 'Security testing', /7n0u4ven3pyh78qbkeitstex3. Trace: controlcatalog-catalog-list-common-controls#task#2 (18 steps).
Cause 3 of 9: the 'Asset maintenance' objective group — expects /ez1ccu4xkxrmlekcvjqo3jk59 'Asset performance management' and /ez4hcdf2fpirhuyojurkval9w 'Asset maintenance scheduling' under domain d4msesd9vvmzmmuvlv06m92uq. Trace: controlcatalog-catalog-list-common-controls#task#4 (9 steps).
Cause 4 of 9: the 'Crisis and emergency management' objective group — expects /7s3t061xlrf2pg0ku4uc9ies2, /v4vb6zir7567vnkcna9ioze, /djyqf4shcydocdojrlplpu7jj, /f23qb01nmegwilcco67p4dv7k and one more under domain 33mjpzadrlwo1by3c1012ai5i. Trace: controlcatalog-catalog-list-common-controls#task#5 (11 steps).
Cause 5 of 9: the 'Secure configuration management' objective group — expects /7m2i6nvt88hcor0f11ywp3rws 'Secure configuration standards', /4aih1y0rudjzgspgm9gt0u0h6 and four more (six entries). Trace: controlcatalog-catalog-list-common-controls#task#13.
Cause 6 of 9: the 'Network architecture and secure configuration' objective group — expects /7n06xcbdggxt86y266cwpngh8, /9b2ix5ogwa1b85c81k028juod, /9vdb29o7zzu3aiwpqh5m35gvq and five more (eight entries) under domain 2oc9yao2szxsquya737481hw9. Trace: controlcatalog-catalog-list-common-controls#task#14 (2 steps).
Cause 7 of 9: the 'Asset inventory management' objective group — expects /d4s7ik8fgv8082v3x31hifzcc, /1ukpmkewk4i92tjmhsvewi4y7, /c0qrxhefhmxkbq22tiejp3enn and four more (seven entries). Trace: controlcatalog-catalog-list-common-controls#task#35 (3 steps).
Cause 8 of 9: the 'Physical security management' objective group — expects /eurwhj15bbp9b4g80686ivehk, /3hdplf9gfvuzqgwqzaewwdjov, /8j3a8rlo557qf1gk08hbhcpjx and six more (nine entries) under domain bn9ck1b74fp4b4wdysq0fanpc. Trace: controlcatalog-catalog-list-common-controls#task#42.
Cause 9 of 9: the 'Authentication and access control' objective group — expects /9706hq18za0bapf4n1bdyhxrs, /8vtkbmh266wkjbvuxcy20tfa2, /1rkm6gzvv1izjv6bl49xfezey and eight more (eleven entries) under domain avh1ncrx6oyndrbsv0rfywvat. Trace: controlcatalog-catalog-list-common-controls#task#49.

## Evidence
Every cluster is verdict=mismatch with expected_status 200 and actual_status 200; the divergence is purely missing payload, e.g.
- controlcatalog-catalog-list-common-controls#task#1: "$.CommonControls[0]: missing (expected {'Arn': 'arn:aws:controlcatalog:::common-control/1nxqoydhsjdyboc4frsrrmev', ..., 'Name': 'Change request and approval', 'Objective': {'Arn': ...objective/14qj3s9axkdma059lhktc14dx', 'Name': 'Change management'}}, got None)" and likewise $.CommonControls[1..3].
- #task#49: $.CommonControls[0]..[10] all missing (expected the eleven 'Authentication and access control' common controls).
No cluster carries a stub_path or owned_by, so none is already filed. Replay summary: steps_total 1408, steps_passed 3, score 0.2%.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. POST /common-controls (fixture controlcatalog-catalog-list-common-controls), with and without CommonControlFilter.Objectives, at steps task#1, task#2, task#4, task#5, task#13, task#14, task#35, task#42 and task#49.
3. Each answers 200 with a CommonControls array that omits every recorded CommonControlSummary for that objective group.
