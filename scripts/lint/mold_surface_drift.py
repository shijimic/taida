#!/usr/bin/env python3
"""Check documented builtin mold signatures against the shared registry.

The registry is the source of truth for builtin mold metadata. This gate
checks that documented public molds are present in the registry, that their
documented arities and option names are accepted by the registry, and that
documented molds still have a runtime lowering or interpreter arm.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path


DOC_PATHS = [
    Path("docs/reference/standard_methods.md"),
    Path("docs/reference/class_like_types.md"),
]
RUNTIME_PATHS = [
    Path("src/codegen/lower_molds.rs"),
    Path("src/interpreter/mold_eval.rs"),
]
REGISTRY_PATH = Path("src/types/mold_specs.rs")

SIG_RE = re.compile(r"`([A-Z][A-Za-z0-9_]*)\[([^`]*)\]\(\)`")
HEADING_SIG_RE = re.compile(r"(?<![A-Za-z0-9_])([A-Z][A-Za-z0-9_]*)\[([^\]]*)\]")
RUNTIME_ARM_NAME_RE = re.compile(r'"([A-Z][A-Za-z0-9_]*)"')

TYPE_ONLY_HEADINGS = {
    "RelaxedGorillax",
}


@dataclass
class RegistrySpec:
    name: str
    arity_min: int
    arity_max: int | None
    return_kind: str
    options: set[str] = field(default_factory=set)
    enforced: bool = False

    def accepts_arity(self, arity: int) -> bool:
        if arity < self.arity_min:
            return False
        return self.arity_max is None or arity <= self.arity_max

    def arity_label(self) -> str:
        if self.arity_max is None:
            return f"{self.arity_min}+"
        if self.arity_min == self.arity_max:
            return str(self.arity_min)
        return f"{self.arity_min}-{self.arity_max}"


@dataclass
class DocSpec:
    name: str
    arities: set[int] = field(default_factory=set)
    options: set[str] = field(default_factory=set)
    return_kinds: set[str] = field(default_factory=set)
    locations: list[str] = field(default_factory=list)


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def split_args(args: str) -> int:
    stripped = args.strip()
    if not stripped:
        return 0
    return len([part for part in (p.strip() for p in stripped.split(",")) if part])


def split_options(raw: str) -> set[str]:
    raw = raw.strip()
    if not raw or raw == "-":
        return set()
    names: set[str] = set()
    for part in raw.split(","):
        name = part.strip().strip("`")
        if not name:
            continue
        name = name.split("<=", 1)[0].strip()
        name = name.split(":", 1)[0].strip()
        if name:
            names.add(name)
    return names


def doc_return_kind(raw: str) -> str | None:
    raw = raw.strip()
    if not raw or raw == "-":
        return None
    if raw == "Bool":
        return "Bool"
    if raw == "Int":
        return "Int"
    if raw == "Float":
        return "Float"
    if raw == "Str":
        return "Str"
    if raw.startswith("@["):
        return "List"
    if any(token in raw for token in ("Lax", "Result", "Gorillax", "JSRilla", "Molten")):
        return "Pack"
    if "Num" in raw or "T" in raw or "A" == raw:
        return "Dynamic"
    return None


def add_doc_entry(
    docs: dict[str, DocSpec],
    path: Path,
    line_no: int,
    name: str,
    args: str,
    options: set[str] | None = None,
    return_kind: str | None = None,
) -> None:
    if name == "Name" or name in TYPE_ONLY_HEADINGS:
        return
    entry = docs.setdefault(name, DocSpec(name=name))
    entry.arities.add(split_args(args))
    if options:
        entry.options.update(options)
    if return_kind:
        entry.return_kinds.add(return_kind)
    entry.locations.append(f"{path}:{line_no}")


def parse_docs(root: Path) -> dict[str, DocSpec]:
    docs: dict[str, DocSpec] = {}
    for rel in DOC_PATHS:
        path = root / rel
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if line.startswith("|"):
                cells = [c.strip() for c in line.strip().strip("|").split("|")]
                if not cells or cells[0].startswith("---"):
                    continue
                sig_cell = None
                for idx, cell in enumerate(cells[:2]):
                    if SIG_RE.search(cell):
                        sig_cell = idx
                        break
                if sig_cell is None:
                    continue
                options: set[str] = set()
                ret_kind: str | None = None
                if sig_cell == 0 and len(cells) >= 4:
                    options = split_options(cells[2])
                    ret_kind = doc_return_kind(cells[3])
                for match in SIG_RE.finditer(cells[sig_cell]):
                    add_doc_entry(
                        docs,
                        rel,
                        line_no,
                        match.group(1),
                        match.group(2),
                        options,
                        ret_kind,
                    )
            elif line.startswith("#"):
                if "系統" in line:
                    continue
                for match in HEADING_SIG_RE.finditer(line):
                    add_doc_entry(docs, rel, line_no, match.group(1), match.group(2))
    return docs


def parse_option_sets(text: str) -> dict[str, set[str]]:
    options: dict[str, set[str]] = {}
    const_re = re.compile(
        r"const\s+([A-Z0-9_]+):\s*&\[MoldOptionSpec\]\s*=\s*&\[(.*?)\];",
        re.S,
    )
    for match in const_re.finditer(text):
        options[match.group(1)] = set(re.findall(r'name:\s*"([^"]+)"', match.group(2)))
    return options


def parse_registry(root: Path) -> dict[str, RegistrySpec]:
    text = (root / REGISTRY_PATH).read_text(encoding="utf-8")
    option_sets = parse_option_sets(text)
    table_start = text.index("pub static MOLD_SPECS")
    table_end = text.index("];", table_start)
    table = text[table_start:table_end]
    starts = [m.start() for m in re.finditer(r"MoldSpec::(?:exact|range)\(", table)]
    specs: dict[str, RegistrySpec] = {}
    for index, start in enumerate(starts):
        end = starts[index + 1] if index + 1 < len(starts) else len(table)
        block = table[start:end]
        header = re.search(r'MoldSpec::(exact|range)\(\s*"([^"]+)"', block)
        if not header:
            continue
        kind, name = header.group(1), header.group(2)
        if kind == "exact":
            arity_match = re.search(r'MoldSpec::exact\(\s*"[^"]+"\s*,\s*(\d+)', block)
            if not arity_match:
                raise ValueError(f"cannot parse exact arity for {name}")
            arity_min = arity_max = int(arity_match.group(1))
        else:
            arity_match = re.search(
                r'MoldSpec::range\(\s*"[^"]+"\s*,\s*(\d+)\s*,\s*(Some\((\d+)\)|None)',
                block,
            )
            if not arity_match:
                raise ValueError(f"cannot parse range arity for {name}")
            arity_min = int(arity_match.group(1))
            arity_max = int(arity_match.group(3)) if arity_match.group(3) else None
        ret_match = re.search(r"MoldReturnKind::([A-Za-z0-9_]+)", block)
        if not ret_match:
            raise ValueError(f"cannot parse return kind for {name}")
        opt_match = re.search(r"\.with_options\(([A-Z0-9_]+)\)", block)
        specs[name] = RegistrySpec(
            name=name,
            arity_min=arity_min,
            arity_max=arity_max,
            return_kind=ret_match.group(1),
            options=option_sets.get(opt_match.group(1), set()) if opt_match else set(),
            enforced=".enforce_checker()" in block,
        )
    return specs


def parse_runtime_names(root: Path) -> dict[str, set[str]]:
    names: dict[str, set[str]] = {}
    for rel in RUNTIME_PATHS:
        path = root / rel
        found: set[str] = set()
        pending: list[str] = []
        for raw in path.read_text(encoding="utf-8").splitlines():
            line = raw.split("//", 1)[0]
            stripped = line.lstrip()
            if not stripped:
                continue
            if not (stripped.startswith('"') or stripped.startswith("|")):
                pending.clear()
                continue
            pending.extend(RUNTIME_ARM_NAME_RE.findall(stripped))
            if "=>" in stripped:
                found.update(pending)
                pending.clear()
        for name in found:
            names.setdefault(name, set()).add(str(rel))
    return names


def check(strict: bool) -> int:
    root = repo_root()
    docs = parse_docs(root)
    registry = parse_registry(root)
    runtime = parse_runtime_names(root)

    failures: list[str] = []
    advisories: list[str] = []

    for name in sorted(docs):
        doc = docs[name]
        spec = registry.get(name)
        if spec is None:
            failures.append(
                f"documented mold `{name}` is missing from {REGISTRY_PATH} "
                f"({', '.join(doc.locations[:3])})"
            )
            continue
        bad_arities = sorted(a for a in doc.arities if not spec.accepts_arity(a))
        if bad_arities:
            failures.append(
                f"`{name}` documented arity {bad_arities} is not accepted by registry "
                f"arity {spec.arity_label()}"
            )
        missing_options = sorted(doc.options - spec.options)
        if missing_options:
            failures.append(
                f"`{name}` documented option(s) {missing_options} missing from registry "
                f"options {sorted(spec.options)}"
            )
        for ret in sorted(doc.return_kinds):
            if ret == "Dynamic" or spec.return_kind == "Dynamic":
                continue
            if ret != spec.return_kind:
                failures.append(
                    f"`{name}` documented return {ret} disagrees with registry "
                    f"return {spec.return_kind}"
                )
        if name not in runtime:
            failures.append(f"documented mold `{name}` has no runtime dispatch arm")

    documented = set(docs)
    registry_names = set(registry)
    runtime_names = set(runtime)
    registry_only = sorted(registry_names - documented)
    runtime_only = sorted(runtime_names - registry_names - documented)
    enforced_missing_runtime = sorted(
        name for name, spec in registry.items() if spec.enforced and name not in runtime
    )

    if registry_only:
        advisories.append(
            "registry entries not documented in the scoped reference files: "
            + ", ".join(registry_only)
        )
    if runtime_only:
        advisories.append(
            "runtime names not represented in the registry: " + ", ".join(runtime_only)
        )
    if enforced_missing_runtime:
        failures.append(
            "checker-enforced registry entries missing runtime arms: "
            + ", ".join(enforced_missing_runtime)
        )

    print("== mold surface drift check ==")
    print(f"  documented molds : {len(documented)}")
    print(f"  registry entries : {len(registry_names)}")
    print(f"  runtime names     : {len(runtime_names)}")

    for msg in advisories:
        print(f"  note: {msg}")

    if failures:
        result = "DRIFT" if strict else "DRIFT (non-strict)"
        print(f"  result            : {result}")
        for msg in failures:
            print(f"  - {msg}")
        return 1 if strict else 0

    print("  result            : OK")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--strict",
        action="store_true",
        help="Fail when documented molds drift from registry or runtime.",
    )
    args = parser.parse_args()
    return check(strict=args.strict)


if __name__ == "__main__":
    sys.exit(main())
