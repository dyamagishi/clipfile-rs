"""Optional cross-implementation oracle; requires a sibling clip-file checkout.

uv run --project ../clip-file python tests/compare_python_builder.py \
    --python-repo ../clip-file

Build examples first with cargo build --all-features --examples.
No real artwork is copied into this repository; outputs live in a temp directory.
"""
from pathlib import Path
import argparse
import json
import sqlite3
import subprocess
import tempfile

from PIL import Image, ImageChops
from clip_parser.builder import ClipBuilder
from clip_parser.clip_file import ClipFile
from clip_parser.reconstruct import ClipReconstructor
from clip_parser.sqlite_utils import extract_sqlite_bytes, temporary_sqlite


def layer_rows(path):
    with temporary_sqlite(extract_sqlite_bytes(ClipFile(path))) as database:
        with sqlite3.connect(database) as connection:
            return connection.execute(
                "SELECT LayerName,LayerType,LayerFolder,LayerOffsetX,LayerOffsetY,"
                "LayerOpacity,LayerNextIndex,LayerFirstChildIndex FROM Layer ORDER BY _PW_ID"
            ).fetchall()


def compare(binary, name, source, metadata, root, extra=(), python_options=None):
    py_clip, rs_clip = root / f"{name}-py.clip", root / f"{name}-rs.clip"
    ClipBuilder().build(source, py_clip, metadata_path=metadata, **(python_options or {}))
    command = [str(binary), str(source), str(rs_clip), *extra]
    if metadata is not None:
        command += ["--metadata", str(metadata)]
    subprocess.run(command, check=True)
    assert layer_rows(py_clip) == layer_rows(rs_clip), f"{name}: metadata mismatch"
    py_png, rs_png = root / f"{name}-py.png", root / f"{name}-rs.png"
    ClipReconstructor(py_clip).reconstruct(py_png)
    ClipReconstructor(rs_clip).reconstruct(rs_png)
    with Image.open(py_png) as a, Image.open(rs_png) as b:
        a, b = a.convert("RGBA"), b.convert("RGBA")
        assert a.size == b.size
        assert a.tobytes() == b.tobytes(), (name, ImageChops.difference(a, b).getextrema())
    print(f"{name}: metadata and reconstructed RGBA EXACT MATCH", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python-repo", type=Path, required=True)
    parser.add_argument("--rust-repo", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    binary = args.rust_repo.resolve() / "target/debug/examples/build_raster"
    cases = [
        ("single", "example/no_layer/test_image_for_clip.png", None),
        ("flat", "example/layers_01", None),
        ("positioned", "example/layers_02", "example/layers_02/metadata.json"),
        ("opacity", "example/layers_04", "example/layers_04/metadata.json"),
        ("nested", "example/layers_06/layers", "example/layers_06/layers/metadata.json"),
    ]
    with tempfile.TemporaryDirectory(prefix="raster-builder-interop-") as temporary:
        root = Path(temporary)
        for name, source, metadata in cases:
            compare(binary, name, args.python_repo / source,
                    args.python_repo / metadata if metadata else None, root)
        mixed = root / "mixed"
        mixed.mkdir()
        Image.new("RGBA", (80, 40), (255, 0, 0, 180)).save(mixed / "c1.png")
        Image.new("RGBA", (60, 64), (0, 255, 0, 180)).save(mixed / "c2.png")
        Image.new("RGBA", (20, 20), (0, 0, 255, 100)).save(mixed / "c10.png")
        compare(binary, "natural-mixed", mixed, None, root,
                ["--order", "natural"], {"order": "natural"})
        info = mixed / "positions.json"
        info.write_text(json.dumps({
            "CANVAS_SIZE": [120, 90],
            "c1.png": {"offset": [-10, -5], "opacity": 77},
            "c2.png": {"offset": [10, 20], "opacity": 154},
        }))
        compare(binary, "negative-alpha", mixed, info, root)
        compare(binary, "explicit-canvas", mixed, info, root,
                ["--canvas-size", "70x80"], {"canvas_size": "70x80"})
        preview = root / "override.png"
        Image.new("RGBA", (12, 7), (10, 20, 30, 255)).save(preview)
        compare(binary, "preview-override", mixed, None, root,
                ["--preview", str(preview)], {"preview_path": preview})
        compare(binary, "pattern", mixed, None, root,
                ["--pattern", "c1*.png"], {"pattern": "c1*.png"})
    print("10 builder interoperability cases passed")


if __name__ == "__main__":
    main()
