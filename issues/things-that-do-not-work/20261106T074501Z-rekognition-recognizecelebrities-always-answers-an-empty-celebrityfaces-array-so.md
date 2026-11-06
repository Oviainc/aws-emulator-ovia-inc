Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-celebrity-recognition-and-lookup
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-celebrity-recognition-and-lookup
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|RecognizeCelebrities|2xx->2xx|$.CelebrityFaces[*]
Issue-Commit: 1a3a8e83afcbc5d4f0c42bb829e14c7ed367d3ec

# RecognizeCelebrities always answers an empty CelebrityFaces array, so no face is ever reported as a celebrity

> _Note: this issue was identified against the code base at commit `1a3a8e83afcbc5d4f0c42bb829e14c7ed367d3ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RecognizeCelebrities

## Description
Cause 1 of 1: RecognizeCelebrities never populates CelebrityFaces (fixture rekognition-celebrity-recognition-and-lookup, step task#2).

AWS: RecognizeCelebrities partitions the faces of the submitted image between the two output lists — Smithy: RecognizeCelebrities -> RecognizeCelebritiesResponse{CelebrityFaces: CelebrityList, UnrecognizedFaces: ComparedFaceList, OrientationCorrection}, and the operation's own documentation: "It lists the recognized celebrities in the CelebrityFaces array and any unrecognized faces in the UnrecognizedFaces array. For each celebrity recognized, RecognizeCelebrities returns a Celebrity object. The Celebrity object contains the celebrity name, ID, URL links to additional information, match confidence, and a ComparedFace object". Smithy shape Celebrity carries Id (RekognitionUniqueId, @pattern ^[0-9A-Za-z]*$), Name (String), Urls (Urls, @length 0..255), MatchConfidence (Percent, @range 0..100), Face (ComparedFace) and KnownGender (KnownGender{Type: KnownGenderType}). The same documentation makes that Id the key for the follow-on lookup: "use the Celebrity ID property as a unique identifier for the celebrity ... you will need the ID to identify the celebrity in a call to the GetCelebrityInfo operation". The recording shows exactly that: HTTP 200 with CelebrityFaces[0] populated as a full Celebrity (an Id, a Name, a KnownGender.Type and a Urls entry), which the fixture then looks up. The recorded Id, name, gender and URL are that one run's values filling the template — the requirement is that a recognized face is reported on the CelebrityFaces side with those members populated and with an Id that the service's own GetCelebrityInfo resolves, not those particular strings.

Emulator: the handler answers HTTP 200 with CelebrityFaces unconditionally EMPTY for every image, and puts every detected face on the UnrecognizedFaces side instead. Replay reports $.CelebrityFaces[0]: missing (expected a Celebrity object, got None) with expected_status 200 and actual_status 200 — the status and envelope match, only the recognized-celebrity half of the partition is never produced. The handler's own doc comment states the choice ("no face is reported as a celebrity"), so the behavior is image-independent: nothing a caller submits can ever yield a CelebrityFaces entry.

Why: a client that relies on AWS's behavior gets a silent, 200-status wrong answer rather than an error it could branch on. Celebrity-recognition flows see every celebrity as an anonymous face, any "did we recognize anyone?" test is always false, and the documented chain RecognizeCelebrities -> Celebrity.Id -> GetCelebrityInfo/GetCelebrityRecognition is unreachable because no Id is ever handed out. Downstream fixtures that index CelebrityFaces[0] fault on a missing element.

Fix: the RecognizeCelebrities handler and the service state it reads. The recognized/unrecognized decision needs a deterministic celebrity identification backed by service state (a seeded celebrity catalogue keyed off the submitted image, the same way the geometric detector is keyed off its pixels) so that a face matching a catalogue entry is emitted as a Celebrity with Id, Name, Urls, MatchConfidence, KnownGender and Face, and is NOT also repeated in UnrecognizedFaces; the Id it mints must be storable/resolvable by GetCelebrityInfo so the documented lookup chain closes. An image with no catalogue match must still answer both lists as measured.

Traces: rekognition-celebrity-recognition-and-lookup#task#2 (RecognizeCelebrities, 200 vs 200, $.CelebrityFaces[0] missing).

Not covered by any open issue: the open rekognition issues cover collection/face fidelity (20261029T194740Z-rekognition-collection-face-fidelity-...), unimplemented scaffold stubs, PutProjectPolicy, and the DetectLabels MinConfidence message formatting (20261102T103806Z-...); none of them touches RecognizeCelebrities or the CelebrityFaces partition.

## Evidence
Fixture rekognition-celebrity-recognition-and-lookup, step task#2, operation RecognizeCelebrities.
Expected (recorded AWS): HTTP 200 with $.CelebrityFaces[0] present as a Celebrity object — the recording's instance was {"Id": "4rr8sP5H", "Name": "Rob Nabors", "KnownGender": {"Type": "Male"}, "Urls": ["www.wikidata.org/wiki/Q7340410"]} (those values are that run's data, the shape and the populated members are the requirement).
Actual (emulator): HTTP 200 with CelebrityFaces = [] — replay detail: "$.CelebrityFaces[0]: missing (expected {'Id': '4rr8sP5H', 'Name': 'Rob Nabors', 'KnownGender': {'Type': 'Male'}, 'Urls': ['www.wikidata.org/wiki/Q7340410']}, got None)".
Source: services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/recognize_celebrities.rs line 232 sets `celebrity_faces: Some(Vec::new())` unconditionally, with every detected face routed to `unrecognized_faces` at line 235.
Model: Smithy RecognizeCelebritiesResponse.CelebrityFaces -> CelebrityList of Celebrity{Id,Name,Urls,Face,MatchConfidence,KnownGender}; operation doc requires a Celebrity object per recognized celebrity and names Celebrity.Id as the GetCelebrityInfo key.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture rekognition-celebrity-recognition-and-lookup, step task#2 issues RecognizeCelebrities with the fixture's image (inline Bytes or the S3Object the earlier step uploaded).
3. Observe the response: HTTP 200, UnrecognizedFaces populated, CelebrityFaces = [].
4. Compare with the recorded response, which carries CelebrityFaces[0] as a full Celebrity (Id/Name/Urls/KnownGender) — the array element the emulator never emits; the fixture's follow-on celebrity lookup has no Id to use.
