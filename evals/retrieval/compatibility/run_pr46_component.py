#!/usr/bin/env python3
"""Reproduce the RAG component check against PR46's unchanged real egress gate."""

import argparse
import os
from pathlib import Path
import subprocess


EGRESS_REF = "5d5773281bc38b1c08882bdbf3f8b6bb841efa87"
KNOWLEDGE_REF = "4542f8266c5f232e4dd38fa2e7d8bdf489af5a01"
SOURCE_PATHS = [
    f"crates/memory-store/src/knowledge/{name}.rs"
    for name in ("mod", "schema", "store", "search", "chunk", "types")
] + [
    "crates/builtin-tools/src/knowledge.rs",
    "crates/builtin-tools/src/knowledge/tests.rs",
    "crates/tool-registry/src/provenance.rs",
]
UNCHANGED_PATHS = [
    "crates/permission-engine",
    "crates/agent-core/src/taint.rs",
    "crates/agent-core/src/approval.rs",
    "crates/agent-core/src/exec.rs",
    "crates/agent-router",
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", type=Path, default=Path.cwd())
    parser.add_argument("--worktree", required=True, type=Path)
    parser.add_argument("--target-dir", type=Path)
    args = parser.parse_args()
    repository = args.repository.resolve()
    worktree = args.worktree.resolve()
    if worktree.exists():
        parser.error("--worktree must name a new directory; existing work is never reset")

    def git(*parts, cwd=repository):
        return subprocess.check_output(["git", "-C", str(cwd), *parts])

    # Fetching is deliberately separate. This script never changes a remote ref.
    for ref in (EGRESS_REF, KNOWLEDGE_REF):
        git("cat-file", "-e", ref + "^{commit}")
    git("worktree", "add", "--detach", str(worktree), EGRESS_REF)
    for path in SOURCE_PATHS:
        destination = worktree / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(git("show", KNOWLEDGE_REF + ":" + path))

    def insert(path, anchor, replacement):
        destination = worktree / path
        source = destination.read_text(encoding="utf-8")
        if anchor not in source:
            raise ValueError(f"expected pinned-head registration anchor missing: {path}")
        destination.write_text(source.replace(anchor, replacement, 1), encoding="utf-8")

    insert("crates/memory-store/src/lib.rs", "pub mod embed;", "pub mod embed;\npub mod knowledge;")
    insert("crates/memory-store/Cargo.toml", "[dependencies]", '[dependencies]\nsha2 = "0.10"')
    insert("crates/builtin-tools/src/lib.rs", "pub mod files;", "pub mod files;\npub mod knowledge;")
    insert(
        "crates/builtin-tools/Cargo.toml",
        "[dependencies]",
        '[dependencies]\nmemory-store = { path = "../memory-store", version = "0.1.0" }',
    )
    insert(
        "crates/tool-registry/src/lib.rs",
        "pub mod registry;",
        "pub mod provenance;\npub mod registry;\npub use provenance::OutputProvenance;",
    )
    insert(
        "crates/tool-registry/src/tool.rs",
        "    fn spec(&self) -> &ToolSpec;",
        "    fn spec(&self) -> &ToolSpec;\n\n"
        "    fn output_provenance(&self) -> Option<crate::OutputProvenance> {\n"
        "        None\n    }",
    )
    insert(
        "crates/agent-core/src/lib.rs",
        "mod egress_tests;",
        "mod egress_tests;\n#[cfg(test)]\nmod rag_46_compat;",
    )
    harness = Path(__file__).with_name("rag_46_compat.rs")
    (worktree / "crates/agent-core/src/rag_46_compat.rs").write_bytes(harness.read_bytes())

    for path in SOURCE_PATHS:
        if (worktree / path).read_bytes() != git("show", KNOWLEDGE_REF + ":" + path):
            raise ValueError("knowledge source changed during overlay: " + path)
    git("diff", "--exit-code", EGRESS_REF, "--", *UNCHANGED_PATHS, cwd=worktree)
    print("Verified exact knowledge sources and unchanged PR46 native gate paths.", flush=True)

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(
        args.target_dir.resolve() if args.target_dir else worktree / "target-component-check"
    )
    subprocess.run(
        ["cargo", "test", "-p", "agent-core", "--lib", "rag_46_compat", "--", "--nocapture"],
        cwd=worktree,
        env=env,
        check=True,
    )
    git("diff", "--exit-code", EGRESS_REF, "--", *UNCHANGED_PATHS, cwd=worktree)
    print("Component checks passed; the detached worktree remains available for review.")


if __name__ == "__main__":
    main()
