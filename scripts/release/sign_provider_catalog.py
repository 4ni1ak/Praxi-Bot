#!/usr/bin/env python3
"""Build and sign `provider-catalog.json` (maintainers only).

The signing key never lives in the repo. It is passed as a base64 32-byte
ed25519 seed in the environment variable PRAXI_CATALOG_SIGNING_KEY.

  # one-time: make a key, keep the seed file private, pin the printed public key
  python scripts/release/sign_provider_catalog.py --generate-key ~/catalog-key.b64

  # every catalog refresh: bump `last_reviewed` in model_metadata.json, then
  PRAXI_CATALOG_SIGNING_KEY="$(cat ~/catalog-key.b64)" \
    python scripts/release/sign_provider_catalog.py --out dist/

It writes `provider-catalog.json` and `provider-catalog.json.minisig` (a
minisign-compatible signature, prehashed "ED"). Upload both as release assets.
The public key printed by --public-key must equal PINNED_PUBLIC_KEY in
crates/provider-catalog/src/catalog_update.rs.
"""
import argparse
import base64
import hashlib
import json
import os
import sys
from pathlib import Path

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

SOURCE = Path(__file__).resolve().parents[2] / "crates/provider-catalog/src/model_metadata.json"
ENV = "PRAXI_CATALOG_SIGNING_KEY"


def raw_public(key: Ed25519PrivateKey) -> bytes:
    return key.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )


def key_id(pub: bytes) -> bytes:
    return hashlib.sha256(pub).digest()[:8]


def public_line(key: Ed25519PrivateKey) -> str:
    pub = raw_public(key)
    return base64.b64encode(b"Ed" + key_id(pub) + pub).decode()


def sign(key: Ed25519PrivateKey, data: bytes, trusted_comment: str) -> str:
    pub = raw_public(key)
    sig = key.sign(hashlib.blake2b(data, digest_size=64).digest())
    global_sig = key.sign(sig + trusted_comment.encode())
    return (
        "untrusted comment: Praxi-Bot provider catalog signature\n"
        + base64.b64encode(b"ED" + key_id(pub) + sig).decode()
        + f"\ntrusted comment: {trusted_comment}\n"
        + base64.b64encode(global_sig).decode()
        + "\n"
    )


def load_key() -> Ed25519PrivateKey:
    seed = os.environ.get(ENV, "").strip()
    if not seed:
        sys.exit(f"error: set {ENV} to the base64 32-byte signing seed (never commit it)")
    raw = base64.b64decode(seed)
    if len(raw) != 32:
        sys.exit("error: the signing seed must decode to exactly 32 bytes")
    return Ed25519PrivateKey.from_private_bytes(raw)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--generate-key", metavar="FILE", help="write a new base64 seed to FILE, print the public key")
    ap.add_argument("--public-key", action="store_true", help="print the public key of the env seed")
    ap.add_argument("--source", type=Path, default=SOURCE)
    ap.add_argument("--out", type=Path, default=Path("dist"))
    args = ap.parse_args()

    if args.generate_key:
        path = Path(args.generate_key)
        if path.exists():
            sys.exit(f"error: {path} already exists; refusing to overwrite a key")
        key = Ed25519PrivateKey.generate()
        seed = key.private_bytes(
            serialization.Encoding.Raw,
            serialization.PrivateFormat.Raw,
            serialization.NoEncryption(),
        )
        path.write_text(base64.b64encode(seed).decode() + "\n")
        print(public_line(key))
        return
    key = load_key()
    if args.public_key:
        print(public_line(key))
        return

    table = json.loads(args.source.read_text(encoding="utf-8"))
    data = (json.dumps(table, indent=2, ensure_ascii=False) + "\n").encode()
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "provider-catalog.json").write_bytes(data)
    comment = f"praxi provider-catalog last_reviewed={table['last_reviewed']}"
    (args.out / "provider-catalog.json.minisig").write_text(sign(key, data, comment))
    print(f"wrote {args.out}/provider-catalog.json (+ .minisig), last_reviewed={table['last_reviewed']}")


if __name__ == "__main__":
    main()
