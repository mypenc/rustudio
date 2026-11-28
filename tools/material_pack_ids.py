#!/usr/bin/env python3
"""Texture pack ids rustudio draws materials with, one per line.

  material_pack_ids.py parts    the part table Studio draws by default
                                (MaterialService.Use2022Materials, 4K):
                                crates/rustudio-render/src/matpack.rs Y2022_4K
  material_pack_ids.py terrain  the 2022 terrain table: assets/terrain-materials.json

The workflow (.github/workflows/studio-content.yml) feeds them to
crates/rustudio-core/examples/prefetch_packs.rs.
"""
import json
import re
import sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent


def parts() -> set[int]:
    src = (root / "crates/rustudio-render/src/matpack.rs").read_text(encoding="utf-8")
    start = src.index("pub static Y2022_4K")
    block = src[start : src.index("];", start)]
    # e(material, pack, detail, colour, word)
    return {int(n) for pair in re.findall(r"\be\(\d+, (\d+), (\d+),", block) for n in pair if int(n) > 0}


def terrain() -> set[int]:
    table = json.loads((root / "assets/terrain-materials.json").read_text(encoding="utf-8"))["2022"]
    found: set[int] = set()

    def walk(o):
        if isinstance(o, dict):
            for k, v in o.items():
                if k == "texturePackId" and isinstance(v, str):
                    found.add(int(v.removeprefix("rbxassetid://")))
                else:
                    walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)

    walk(table)
    return {i for i in found if i > 0}


if __name__ == "__main__":
    which = sys.argv[1] if len(sys.argv) > 1 else ""
    if which not in ("parts", "terrain"):
        sys.exit(__doc__)
    ids = sorted(parts() if which == "parts" else terrain())
    if not ids:
        sys.exit(f"no {which} pack ids found")
    print("\n".join(map(str, ids)))
