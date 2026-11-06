#!/usr/bin/env python3
"""Independent v2 review fixtures using RFC 5869 HMAC and RFC 8949 CBOR.

Synthetic keys only. No production secrets or network. V1 fixtures are immutable.
The KDF includes full u64 boundaries as encodings, not consumable counters.
"""
import argparse
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location("v1_vectors", Path(__file__).with_name("generate.py"))
v1 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(v1)


def fixture():
    seed = bytes(range(32))
    cases = []
    for transport in ["direct", "relay"]:
        for generation in [0, 1, 2**32, 2**64 - 1]:
            for counter in [0, 1, 2**32, 2**64 - 1]:
                values = {}
                for purpose in ["room", "credential"]:
                    info = bytes([len(transport)]) + transport.encode()
                    info += bytes([len(purpose)]) + purpose.encode()
                    info += generation.to_bytes(8, "big") + counter.to_bytes(8, "big")
                    values[purpose] = v1.hkdf32(b"coven.mobile.rendezvous.v2", seed, info).hex()
                cases.append({"transport": transport, "generation": generation, "counter": counter, **values})
    host, device = v1.public_point(1), v1.public_point(2)
    proofs = []
    for generation, floor, counter in [(7, 8, 9), (0, 0, 0), (2**32, 2**32, 2**32), (2**64-1, 2**64-1, 2**64-1)]:
        fields = [(1).to_bytes(16, "big"), (2).to_bytes(16, "big"), 3, host, device, bytes(range(32))]
        case = {"generation": generation, "hostFloor": floor, "counter": counter}
        for role in ["host", "device"]:
            case[role + "Proof"] = v1.cbor(["coven.mobile.possession", 2, v1.PROFILE, role, bytes([3])*32, bytes([4])*32, bytes([5])*32] + fields + [generation, floor, counter]).hex()
        proofs.append(case)
    return {"fixtureFormatVersion": 1, "routingProtocolVersion": 2, "possessionProofVersion": 2,
            "notice": "Synthetic independent HKDF and CBOR vectors. Signature and Noise transcript evidence is separate and remains required.",
            "routingSeed": seed.hex(), "routing": cases, "hostPublic": host.hex(), "devicePublic": device.hex(), "possessionProofs": proofs}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--write", action="store_true")
    args = parser.parse_args()
    path = Path(__file__).with_name("wire-v2.json")
    rendered = json.dumps(fixture(), indent=2) + "\n"
    if args.write:
        path.write_text(rendered)
    elif path.read_text() != rendered:
        raise SystemExit("v2 fixture differs from independent generator")
    print("V2 routing fixture generation check passed.")
