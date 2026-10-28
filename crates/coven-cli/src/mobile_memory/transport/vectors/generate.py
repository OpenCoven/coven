#!/usr/bin/env python3
"""Review fixtures only: independent CBOR/HKDF bytes, never runtime crypto.

Uses Python standard-library HMAC/SHA256 and OpenSSL for synthetic public keys.
RFC 8949 sections 3/4.2 and RFC 5869 sections 2.2/2.3 define the encodings.
Run with --check to verify the checked-in fixture, or --write to regenerate it.
No production input, private key file, network access, or package install is used.
"""
import argparse
import hashlib
import hmac
import json
from pathlib import Path
import subprocess

PROFILE = "Noise_NK_25519_ChaChaPoly_SHA256"


def head(major, length):
    if length < 24:
        return bytes([(major << 5) | length])
    for size, marker in [(1, 24), (2, 25), (4, 26), (8, 27)]:
        if length < 1 << (8 * size):
            return bytes([(major << 5) | marker]) + length.to_bytes(size, "big")
    raise ValueError("outside unsigned 64-bit fixture domain")


def cbor(value):
    if isinstance(value, int) and value >= 0:
        return head(0, value)
    if isinstance(value, bytes):
        return head(2, len(value)) + value
    if isinstance(value, str):
        data = value.encode("utf-8")
        return head(3, len(data)) + data
    if isinstance(value, list):
        return head(4, len(value)) + b"".join(map(cbor, value))
    raise TypeError("fixture supports only unsigned integers, bytes, text and arrays")


def public_point(scalar):
    # SEC1 ECPrivateKey with scalar 1 or 2, exclusively synthetic test inputs.
    der = bytes.fromhex("30310201010420") + scalar.to_bytes(32, "big")
    der += bytes.fromhex("a00a06082a8648ce3d030107")
    output = subprocess.run(
        ["openssl", "pkey", "-inform", "DER", "-pubout", "-outform", "DER"],
        input=der, capture_output=True, check=True,
    ).stdout
    prefix = bytes.fromhex("3059301306072a8648ce3d020106082a8648ce3d030107034200")
    assert output.startswith(prefix) and len(output) == len(prefix) + 65
    return output[len(prefix):]


def hkdf32(salt, material, info):
    # Exactly one SHA-256 expansion block, sufficient for these 32-byte outputs.
    extracted = hmac.digest(salt, material, hashlib.sha256)
    return hmac.digest(extracted, info + b"\x01", hashlib.sha256)


def fixture():
    # Standard examples independently anchor the small fixture encoders.
    assert cbor([1, 24, b"ab", "cd"]).hex() == "84011818426162626364"
    assert hkdf32(bytes(range(13)), b"\x0b" * 22, bytes(range(240, 250))).hex() == (
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf"
    )
    host, device = public_point(1), public_point(2)
    noise = bytes(range(32))
    context = bytes(range(32, 64))
    digest, host_nonce, device_nonce = bytes([3]) * 32, bytes([4]) * 32, bytes([5]) * 32
    cases = []
    for epoch in [0, 23, 24, 255, 256, 65535, 65536, 2**32, 2**64 - 1]:
        fields = [(1).to_bytes(16, "big"), (2).to_bytes(16, "big"), epoch, host, device, noise]
        cases.append({
            "epoch": epoch,
            "binding": cbor(["coven.mobile.noise-binding", 1, PROFILE] + fields).hex(),
            "hostProof": cbor(["coven.mobile.possession", 1, PROFILE, "host", digest, host_nonce, device_nonce] + fields).hex(),
            "deviceProof": cbor(["coven.mobile.possession", 1, PROFILE, "device", digest, host_nonce, device_nonce] + fields).hex(),
        })
    routing = []
    for transport in ["direct", "relay"]:
        for slot in [0, 1, 2**32, 2**64 - 1]:
            values = {purpose: hkdf32(b"coven.mobile.rendezvous.v1", noise, transport.encode() + purpose.encode() + slot.to_bytes(8, "big")).hex() for purpose in ["room", "credential"]}
            routing.append({"transport": transport, "slot": slot, **values})
    return {
        "notice": "Synthetic draft-profile review fixtures; not Noise interoperability or security approval.",
        "hostPublic": host.hex(), "devicePublic": device.hex(),
        "bindingAndProofs": cases,
        "prologues": {transport: cbor(["coven.mobile.connection", 1, PROFILE, transport, context]).hex() for transport in ["direct", "relay"]},
        "routing": routing,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--write", action="store_true")
    args = parser.parse_args()
    path = Path(__file__).with_name("wire.json")
    rendered = json.dumps(fixture(), indent=2) + "\n"
    if args.write:
        path.write_text(rendered)
    elif path.read_text() != rendered:
        raise SystemExit("wire fixture differs from independent generator")
    print("Wire fixture generation check passed.")
