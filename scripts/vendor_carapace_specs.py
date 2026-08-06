#!/usr/bin/env python3
"""Vendor carapace-bin's man/cmd YAML spec collection into a JSON map keyed by
tool name.

Usage:
    python3 scripts/vendor_carapace_specs.py <path-to-carapace-bin-clone>

Writes mandible-extract/src/known_specs/specs.json (normalized carapace-spec),
which the CarapaceSpec extraction tier (priority 1) serves at runtime.

Flag keys use the carapace-spec grammar (see carapace-spec pkg/command/flag.go):
    [-s, ][--long][modifier...]
with modifiers:  =  takes a value
                 ?  takes an optional value
                 *  repeatable
                 !  required
                 &  hidden
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parent.parent
SOURCE = Path(sys.argv[1]) if len(sys.argv) > 1 else REPO_ROOT / ".." / "carapace-bin"
OUT = REPO_ROOT / "mandible-extract" / "src" / "known_specs" / "specs.json"
MAN = SOURCE / "man" / "cmd"

FLAG_RE = re.compile(
    r"^(?P<shorthand>-[^-][^ =*?&!]*)?(?:, )?(?P<longhand>-[-]?[^ =*?&!]*)?(?P<modifier>[=*?&!]*)$"
)


def parse_flag(key: str, value) -> dict | None:
    m = FLAG_RE.match(key.strip())
    if not m:
        return None
    longhand = m.group("longhand") or ""
    shorthand = m.group("shorthand") or ""
    mod = m.group("modifier") or ""
    name_as_shorthand = bool(longhand) and not longhand.startswith("--")
    flag = {
        "long": longhand[2:] if longhand and not name_as_shorthand else (longhand[1:] if longhand else None),
        "short": shorthand[1:] if shorthand else None,
        "takes_value": "=" in mod or "?" in mod,
        "optional_value": "?" in mod,
        "repeatable": "*" in mod,
        "required": "!" in mod,
        "hidden": "&" in mod,
    }
    if isinstance(value, dict):
        if value.get("description"):
            flag["description"] = str(value["description"])
        if "nargs" in value:
            flag["nargs"] = value["nargs"]
        if "default" in value:
            flag["default"] = str(value["default"])
    elif value is not None:
        flag["description"] = str(value)
    if flag.get("description"):
        flag["description"] = flag["description"].strip()
    return flag


def normalize_name(name: str) -> tuple[str, list[str]]:
    # man-derived specs smuggle usage into `name`, e.g. "docker [OPTIONS] COMMAND [ARG...]".
    parts = name.split(None, 1)
    if not parts:
        return "", []
    name = parts[0]
    usage = []
    if len(parts) > 1:
        usage.append(parts[1])
    return name, usage


def spec_from_yaml(data: dict) -> dict:
    name, usage = normalize_name(str(data.get("name", "")))
    out: dict = {"name": name}
    if usage:
        out["usage"] = usage
    if data.get("description"):
        out["description"] = str(data["description"]).strip()
    if data.get("aliases"):
        out["aliases"] = [str(a) for a in data["aliases"]]
    if data.get("hidden"):
        out["hidden"] = True
    if data.get("group"):
        out["group"] = str(data["group"])
    out["flags"] = _flags(data.get("flags"))
    out["persistentflags"] = _flags(data.get("persistentflags"))
    doc = data.get("documentation")
    if isinstance(doc, dict) and doc.get("command"):
        out["documentation"] = str(doc["command"]).strip()
    out["commands"] = [spec_from_yaml(c) for c in (data.get("commands") or [])]
    return out


def _flags(raw) -> list[dict]:
    if not isinstance(raw, dict):
        return []
    out = []
    for key, value in raw.items():
        f = parse_flag(key, value)
        if f:
            out.append(f)
    return out


def merge_flags(a: list[dict], b: list[dict]) -> list[dict]:
    merged = {f.get("long") or f.get("short"): f for f in a}
    for f in b:
        merged.setdefault(f.get("long") or f.get("short"), f)
    return list(merged.values())


def build_tree(tool_dir: Path, tool: str) -> dict:
    """Nest the flat `tool.<sub>*.yaml` files into a trie by dotted name."""
    trie: dict = {}
    for f in sorted(tool_dir.glob(f"{tool}*.yaml")):
        parts = f.stem.split(".")
        if parts[0] != tool:
            continue
        cur = trie
        for part in parts[1:]:
            cur = cur.setdefault(part, {})
        cur["__file__"] = f
    return trie


def build_spec(trie: dict, fallback_name: str) -> dict:
    spec = {}
    if "__file__" in trie:
        data = yaml.safe_load(trie["__file__"].read_text(encoding="utf-8")) or {}
        spec = spec_from_yaml(data)
    if not spec.get("name"):
        spec["name"] = fallback_name

    children = []
    for key, sub in trie.items():
        if key == "__file__":
            continue
        children.append(build_spec(sub, key))
    if children:
        inline = {c["name"]: c for c in spec.get("commands", [])}
        for child in children:
            if child["name"] in inline:
                existing = inline[child["name"]]
                existing["flags"] = merge_flags(existing.get("flags", []), child.get("flags", []))
                if child.get("persistentflags") and not existing.get("persistentflags"):
                    existing["persistentflags"] = child["persistentflags"]
                if not existing.get("description") and child.get("description"):
                    existing["description"] = child["description"]
            else:
                inline[child["name"]] = child
        spec["commands"] = list(inline.values())
    return spec


def main() -> None:
    if not MAN.is_dir():
        sys.exit(f"source spec dir not found: {MAN}")
    try:
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=SOURCE, text=True).strip()
    except Exception:
        commit = "unknown"

    db: dict = {"_meta": {
        "provider": "carapace-spec",
        "source": "https://github.com/carapace-sh/carapace-bin",
        "source_dir": "man/cmd",
        "commit": commit,
        "generated": datetime.now(timezone.utc).isoformat(),
    }}
    skipped = []
    for tool_dir in sorted(MAN.iterdir()):
        if not tool_dir.is_dir():
            continue
        tool = tool_dir.name
        trie = build_tree(tool_dir, tool)
        if not trie:
            skipped.append(tool)
            continue
        db[tool] = build_spec(trie, tool)

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(db, separators=(",", ":"), ensure_ascii=False), encoding="utf-8")
    print(f"vendored {len(db) - 1} tools -> {OUT} ({OUT.stat().st_size / 1e6:.1f} MB)")
    if skipped:
        print("skipped (no yaml):", ", ".join(skipped))


if __name__ == "__main__":
    main()