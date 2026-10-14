#!/usr/bin/env python3
"""Usage: python3 scripts/module-graph.py"""
import re
from collections import defaultdict
from pathlib import Path

SOURCE = Path(__file__).resolve().parent.parent / "src"
MODULES = set(re.findall(r"^(?:pub )?mod (\w+);", (SOURCE / "lib.rs").read_text(), re.MULTILINE))


def imported(text):
    for group, single in re.findall(r"crate::\{(.*?)\};|crate::(\w+)", text, re.DOTALL):
        if single:
            yield single
            continue
        depth, name = 0, ""
        for character in group + ",":
            if character == "," and depth == 0:
                found = re.match(r"\s*(\w+)", name)
                if found:
                    yield found.group(1)
                name = ""
                continue
            depth += {"{": 1, "}": -1}.get(character, 0)
            name += character


def edges():
    found = defaultdict(set)
    for file in sorted(SOURCE.rglob("*.rs")):
        module = file.relative_to(SOURCE).parts[0].removesuffix(".rs")
        if module in MODULES:
            found[module].update(name for name in imported(file.read_text()) if name in MODULES and name != module)
    return found


def main():
    found = edges()
    print("flowchart TD")
    for module in sorted(found):
        for target in sorted(found[module]):
            both = module in found.get(target, ())
            if both and module > target:
                continue
            print(f"  {module} {'<-->' if both else '-->'} {target}")


if __name__ == "__main__":
    main()
