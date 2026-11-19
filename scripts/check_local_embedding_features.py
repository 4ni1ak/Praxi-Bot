#!/usr/bin/env python3
"""Fail the explicit-artifact build if model/runtime download features activate."""
from __future__ import annotations

import re
import subprocess
import sys
import unittest


def verify(graph: str) -> None:
    found: dict[str, set[str]] = {}
    for line in graph.splitlines():
        match = re.match(r"(fastembed|ort|ort-sys) v\S+ features=\[([^]]*)\]", line)
        if match:
            found.setdefault(match[1], set()).update(filter(None, match[2].split(",")))
    required = {"fastembed": "ort-load-dynamic", "ort": "load-dynamic", "ort-sys": "disable-linking"}
    for package, feature in required.items():
        if feature not in found.get(package, set()):
            raise ValueError(f"{package} is missing required explicit-artifact feature {feature}")
    for package, features in found.items():
        forbidden = {feature for feature in features if "download-binaries" in feature or "hf-hub" in feature}
        if forbidden:
            raise ValueError(f"{package} unexpectedly enables downloads: {sorted(forbidden)}")


class FeatureGateTests(unittest.TestCase):
    graph = "\n".join(("fastembed v5 features=[ort-load-dynamic]", "ort v2 features=[load-dynamic,std]", "ort-sys v2 features=[disable-linking,std]"))

    def test_accepts_explicit_artifact_only(self) -> None:
        verify(self.graph)

    def test_rejects_native_downloader(self) -> None:
        with self.assertRaisesRegex(ValueError, "unexpectedly enables downloads"):
            verify(self.graph.replace("disable-linking,std", "disable-linking,std,download-binaries"))

    def test_rejects_model_hub(self) -> None:
        with self.assertRaisesRegex(ValueError, "unexpectedly enables downloads"):
            verify(self.graph.replace("ort-load-dynamic]", "ort-load-dynamic,hf-hub-native-tls]"))

    def test_rejects_missing_dynamic_runtime(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing required"):
            verify(self.graph.replace("disable-linking", "api-17"))


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    else:
        result = subprocess.run(["cargo", "tree", "--locked", "-p", "memory-store", "--features", "local-embeddings", "--prefix", "none", "-e", "normal,build", "--format", "{p} features=[{f}]"], check=True, text=True, capture_output=True)
        verify(result.stdout)
        print("Explicit-artifact feature graph verified: no ONNX Runtime or model hub downloader enabled.")
