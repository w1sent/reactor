---
title: Stage H — Network Traffic Analysis
toolset: network
---
**Goal:** Characterise malicious network activity: C2, exfiltration, spread and payload delivery.

**Entry criteria:** Packet captures or network telemetry exist.

**Activities:**
- Profile protocols, endpoints and volumes; identify suspicious or algorithmically generated name resolution.
- Follow application-layer streams for C2 patterns (request structure, headers, encoding, beacon timing and jitter); extract transferred files and artifacts.
- Fingerprint encrypted sessions to link infrastructure; identify exfiltration and lateral or device-to-device spread; extract network IOCs.
- Include non-IP and short-range channels where relevant to the platform (Bluetooth, cellular, serial, industrial protocols).

**Decision points:** Encrypted C2 -> pivot to metadata analysis and to memory or reverse engineering for key and protocol recovery. Evidence of exfiltration -> escalate scope and impact assessment.

**Pitfalls:** Analysing live malicious traffic on a production network; ignoring encrypted flows entirely; missing low-and-slow beacons; overlooking non-TCP/IP transports.

**Deliverables:** Network analysis report, C2 and exfiltration IOCs, extracted files, infrastructure fingerprints. Feeds Stages I, J, K.

Call `reactor_phase_complete` once C2 and exfiltration behaviour is characterised.
