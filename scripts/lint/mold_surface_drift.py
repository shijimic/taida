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
    Path("docs/api/prelude.md"),
    Path("docs/api/os.md"),
    Path("docs/api/net.md"),
    Path("docs/api/js.md"),
    Path("docs/api/abi.md"),
    Path("docs/api/build_descriptors.md"),
    Path("docs/reference/addon_manifest.md"),
]
RUNTIME_PATHS = [
    Path("src/codegen/lower_molds.rs"),
    Path("src/interpreter/mold_eval.rs"),
]
REGISTRY_PATH = Path("src/types/mold_specs.rs")

BACKTICK_RE = re.compile(r"`([^`]+)`")
HEADING_SIG_RE = re.compile(r"(?<![A-Za-z0-9_])([A-Z][A-Za-z0-9_]*)\[([^\]]*)\]")
RUNTIME_ARM_NAME_RE = re.compile(r'"([A-Z][A-Za-z0-9_]*)"')

TYPE_ONLY_HEADINGS: set[str] = set()

RUNTIME_TYPE_LITERAL_NAMES = {
    "Error",
    "Num",
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
    parts: list[str] = []
    start = 0
    square_depth = 0
    paren_depth = 0
    for idx, char in enumerate(stripped):
        if char == "[":
            square_depth += 1
        elif char == "]" and square_depth:
            square_depth -= 1
        elif char == "(":
            paren_depth += 1
        elif char == ")" and paren_depth:
            paren_depth -= 1
        elif char == "," and square_depth == 0 and paren_depth == 0:
            parts.append(stripped[start:idx].strip())
            start = idx + 1
    parts.append(stripped[start:].strip())
    return len([part for part in parts if part])


def split_options(raw: str) -> set[str]:
    raw = raw.strip()
    if not raw or raw in {"-", "—"}:
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


def extract_mold_sigs(raw: str) -> list[tuple[str, str]]:
    sigs: list[tuple[str, str]] = []
    index = 0
    while index < len(raw):
        match = re.search(r"(?<![A-Za-z0-9_])([A-Z][A-Za-z0-9_]*)\[", raw[index:])
        if not match:
            break
        name = match.group(1)
        bracket_start = index + match.end() - 1
        cursor = bracket_start + 1
        depth = 1
        while cursor < len(raw) and depth:
            if raw[cursor] == "[":
                depth += 1
            elif raw[cursor] == "]":
                depth -= 1
            cursor += 1
        if depth:
            index = bracket_start + 1
            continue
        args = raw[bracket_start + 1 : cursor - 1]
        after = cursor
        while after < len(raw) and raw[after].isspace():
            after += 1
        if after >= len(raw) or raw[after] != "(":
            index = cursor
            continue
        paren = after + 1
        paren_depth = 1
        while paren < len(raw) and paren_depth:
            if raw[paren] == "(":
                paren_depth += 1
            elif raw[paren] == ")":
                paren_depth -= 1
            paren += 1
        if paren_depth:
            index = after + 1
            continue
        sigs.append((name, args))
        index = paren
    return sigs


def doc_return_kind(raw: str) -> str | None:
    raw = raw.strip().strip("`")
    if not raw or raw == "-":
        return None
    if "->" in raw:
        raw = raw.rsplit("->", 1)[1].strip()
    raw = raw.strip().strip("`")
    if raw == "Bool":
        return "Bool"
    if raw == "Int":
        return "Int"
    if raw == "Float":
        return "Float"
    if raw == "Str":
        return "Str"
    if raw == "Pack":
        return "Pack"
    if raw == "Dynamic":
        return "Dynamic"
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
        if not path.exists():
            continue
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if line.startswith("|"):
                cells = [c.strip() for c in line.strip().strip("|").split("|")]
                if not cells or cells[0].startswith("---"):
                    continue
                sig_cell = None
                sigs: list[tuple[str, str]] = []
                for idx, cell in enumerate(cells[:2]):
                    sigs = [
                        sig
                        for fragment in BACKTICK_RE.findall(cell)
                        for sig in extract_mold_sigs(fragment)
                    ]
                    if sigs:
                        sig_cell = idx
                        break
                if sig_cell is None:
                    continue
                options: set[str] = set()
                ret_kind: str | None = None
                if sig_cell == 0:
                    if len(cells) >= 5:
                        options = split_options(cells[2])
                        ret_kind = doc_return_kind(cells[3])
                    elif len(cells) >= 4:
                        ret_kind = doc_return_kind(cells[2])
                    elif len(cells) >= 2:
                        ret_kind = doc_return_kind(cells[1])
                for name, args in sigs:
                    add_doc_entry(
                        docs,
                        rel,
                        line_no,
                        name,
                        args,
                        options,
                        ret_kind,
                    )
            elif line.startswith("#"):
                if "系統" in line:
                    continue
                for match in HEADING_SIG_RE.finditer(line):
                    add_doc_entry(docs, rel, line_no, match.group(1), match.group(2))
            else:
                if "=> :" in line:
                    for name, args in extract_mold_sigs(line):
                        add_doc_entry(
                            docs,
                            rel,
                            line_no,
                            name,
                            args,
                            return_kind=doc_return_kind(line),
                        )
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
        if name not in runtime and spec.enforced:
            failures.append(f"documented mold `{name}` has no runtime dispatch arm")

    documented = set(docs)
    registry_names = set(registry)
    runtime_names = set(runtime)
    registry_only = sorted(registry_names - documented)
    runtime_only = sorted((runtime_names - registry_names - documented) - RUNTIME_TYPE_LITERAL_NAMES)
    enforced_missing_runtime = sorted(
        name for name, spec in registry.items() if spec.enforced and name not in runtime
    )

    if registry_only:
        failures.append(
            "registry entries not documented in the scoped reference files: "
            + ", ".join(registry_only)
        )
    if runtime_only:
        failures.append(
            "runtime mold names not represented in the registry: " + ", ".join(runtime_only)
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
