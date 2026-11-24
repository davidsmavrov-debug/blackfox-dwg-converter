"""Run with Python and ezdxf against the compiled BlackFox converter."""

import argparse
import base64
import json
import struct
from collections import Counter
from pathlib import Path
import subprocess
import tempfile
import traceback

import ezdxf

VERSIONS = {
    "R12": b"AC1009", "R13": b"AC1012", "R14": b"AC1014",
    "R2000": b"AC1015", "R2004": b"AC1018", "R2007": b"AC1021",
    "R2010": b"AC1024", "R2013": b"AC1027", "R2018": b"AC1032",
}


def fixture(path):
    drawing = ezdxf.new("R2018")
    drawing.units = 4
    drawing.layers.new("MODEL", dxfattribs={"color": 3})
    attrs = {"layer": "MODEL"}
    model = drawing.modelspace()
    model.add_point((1.125, -3.5, 9.25), dxfattribs=attrs)
    model.add_line((1.125, -3.5, 9.25), (-3.25, 2.125, 1.5), dxfattribs=attrs)
    model.add_circle((9.5, 1.25, 7.5), 3.125, dxfattribs=attrs)
    model.add_arc((20, 5, 3), 2.5, 20, 145, dxfattribs=attrs)
    model.add_3dface([(0, 0, 1), (2, 0, 1), (1, 1, 2)], dxfattribs=attrs)
    leaf = drawing.blocks.new("LEAF")
    leaf.add_line((1, 2, 3), (4, 5, 6), dxfattribs=attrs)
    parent = drawing.blocks.new("PARENT")
    parent.add_blockref("LEAF", (1, 2, 3))
    model.add_blockref("PARENT", (10, 20, 30), dxfattribs={
        "rotation": 25, "xscale": -2, "yscale": 3, "zscale": 0.5,
    })
    drawing.saveas(path)


def check_dxf(path, expected_version, expected_units=None):
    assert expected_version in path.read_bytes()[:1024]
    drawing = ezdxf.readfile(path)
    model = drawing.modelspace()
    assert Counter(e.dxftype() for e in model) == {
        "POINT": 1, "LINE": 1, "CIRCLE": 1, "ARC": 1, "3DFACE": 1, "INSERT": 1,
    }
    assert drawing.layers.get("MODEL").color == 3
    if expected_units is not None:
        assert drawing.units == expected_units
    line = model.query("LINE").first
    assert line.dxf.layer == "MODEL"
    assert line.dxf.start.isclose((1.125, -3.5, 9.25), abs_tol=1e-9)
    assert line.dxf.end.isclose((-3.25, 2.125, 1.5), abs_tol=1e-9)
    assert abs(model.query("ARC").first.dxf.end_angle - 145) < 1e-7
    assert abs(model.query("CIRCLE").first.dxf.radius - 3.125) < 1e-9
    face = model.query("3DFACE").first
    assert face.dxf.vtx2.isclose((1, 1, 2), abs_tol=1e-9)
    insert = model.query("INSERT").first
    assert insert.dxf.name == "PARENT"
    assert insert.dxf.insert.isclose((10, 20, 30), abs_tol=1e-9)
    assert tuple(round(insert.dxf.get(k), 7) for k in (
        "rotation", "xscale", "yscale", "zscale")) == (25, -2, 3, 0.5)
    assert drawing.blocks["PARENT"].query("INSERT").first.dxf.name == "LEAF"
    assert drawing.blocks["LEAF"].query("LINE").first.dxf.end.isclose((4, 5, 6))


def rich_fixture(path):
    drawing = ezdxf.new("R2018")
    model = drawing.modelspace()
    drawing.layers.new("DETAIL", dxfattribs={"color": 4, "lineweight": 35})
    attrs = {"layer": "DETAIL"}
    drawing.appids.new("BLACKFOX_TEST")
    text = model.add_text("Cad exchange", height=0.75, dxfattribs=attrs)
    text.set_xdata("BLACKFOX_TEST", [(1000, "annotation"), (1070, 42)])
    model.add_mtext("Multiline\\Pnote", dxfattribs={**attrs, "char_height": 1.2})
    model.add_lwpolyline([(0, 0, 0.25), (4, 0, 0), (4, 2, 0)],
                         format="xyb", close=True, dxfattribs=attrs)
    model.add_polyline3d([(0, 0, 0), (2, 5, 1), (3, 0, -2)], dxfattribs=attrs)
    model.add_ellipse((2, 1, 5), (4, 1, 0), ratio=0.4, dxfattribs=attrs)
    model.add_open_spline([(0, 0, 0), (2, 2, 1), (4, 1, -1), (6, 0, 0)],
                          dxfattribs=attrs)
    hatch = model.add_hatch(color=3, dxfattribs=attrs)
    hatch.paths.add_polyline_path([(0, 0), (4, 0), (4, 4), (0, 4)], is_closed=True)
    mesh = model.add_mesh(dxfattribs=attrs)
    with mesh.edit_data() as data:
        data.vertices = [(0, 0, 0), (1, 0, 1), (1, 1, 0), (0, 1, 1)]
        data.faces = [(0, 1, 2), (0, 2, 3)]
    drawing.layout("Layout1").add_line((3, 4), (5, 6), dxfattribs=attrs)
    model.add_line((1e9 + 0.125, 0.00000031, -10), (1e9 + 1.25, 0.0025, -9),
                   dxfattribs=attrs)
    drawing.saveas(path)


def check_rich(path):
    doc = ezdxf.readfile(path)
    model = doc.modelspace()
    assert Counter(e.dxftype() for e in model) == {
        "TEXT": 1, "MTEXT": 1, "LWPOLYLINE": 1, "POLYLINE": 1,
        "ELLIPSE": 1, "SPLINE": 1, "HATCH": 1, "MESH": 1, "LINE": 1,
    }
    assert doc.layers.get("DETAIL").dxf.lineweight == 35
    assert all(e.dxf.layer == "DETAIL" for e in model)
    text = model.query("TEXT").first
    assert text.dxf.text == "Cad exchange" and text.dxf.height == 0.75
    assert list(text.get_xdata("BLACKFOX_TEST")) == [(1000, "annotation"), (1070, 42)]
    assert model.query("MTEXT").first.plain_text() == "Multiline\nnote"
    poly = model.query("LWPOLYLINE").first
    assert poly.closed and poly.get_points("xyb")[0] == (0, 0, 0.25)
    assert model.query("POLYLINE").first.is_3d_polyline
    ellipse = model.query("ELLIPSE").first
    assert abs(ellipse.dxf.ratio - 0.4) < 1e-9
    assert ellipse.dxf.major_axis.isclose((4, 1, 0))
    assert len(model.query("SPLINE").first.control_points) == 4
    assert len(model.query("HATCH").first.paths) == 1
    assert [tuple(f) for f in model.query("MESH").first.faces] == [(0, 1, 2), (0, 2, 3)]
    assert model.query("LINE").first.dxf.start.x == 1e9 + 0.125
    assert abs(model.query("LINE").first.dxf.start.y - 0.00000031) < 1e-12
    assert len(doc.layout("Layout1").query("LINE")) == 1


def exchange_fixtures(root):
    obj = root / "triangle.obj"
    obj.write_text("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")
    stl = root / "triangle.stl"
    stl.write_text("""solid triangle
facet normal 0 0 1
outer loop
vertex 0 0 0
vertex 1 0 0
vertex 0 1 0
endloop
endfacet
endsolid triangle
""")
    dae = root / "triangle.dae"
    dae.write_text("""<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
<library_geometries><geometry id="tri"><mesh>
<source id="pos"><float_array id="coords" count="9">0 0 0 1 0 0 0 1 0</float_array>
<technique_common><accessor source="#coords" count="3" stride="3">
<param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/>
</accessor></technique_common></source>
<vertices id="verts"><input semantic="POSITION" source="#pos"/></vertices>
<triangles count="1"><input semantic="VERTEX" source="#verts" offset="0"/><p>0 1 2</p></triangles>
</mesh></geometry></library_geometries></COLLADA>
""")
    fbx = root / "triangle.fbx"
    fbx.write_text("""; FBX 7.4.0 project file
Objects: {
    Geometry: 100, "Geometry::Triangle", "Mesh" {
        Vertices: *9 {
            a: 0.0,0.0,0.0,1.0,0.0,0.0,0.0,1.0,0.0
        }
        PolygonVertexIndex: *3 {
            a: 0,1,-3
        }
    }
    Model: 200, "Model::Triangle", "Mesh" {
    }
}
Connections: {
    C: "OO", 100, 200
}
""")
    gltf = root / "triangle.gltf"
    buffer = struct.pack("<9f", 0, 0, 0, 1, 0, 0, 0, 1, 0)
    obj_data = {
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": len(buffer)}],
        "bufferViews": [{"buffer": 0, "byteLength": len(buffer), "target": 34962}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3,
                       "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
        "nodes": [{"mesh": 0}], "scenes": [{"nodes": [0]}], "scene": 0,
    }
    packed = json.dumps(obj_data).encode()
    packed += b" " * (-len(packed) % 4)
    chunks = struct.pack("<I4s", len(packed), b"JSON") + packed
    chunks += struct.pack("<I4s", len(buffer), b"BIN\0") + buffer
    glb = root / "triangle.glb"
    glb.write_bytes(struct.pack("<4sII", b"glTF", 2, 12 + len(chunks)) + chunks)
    obj_data["buffers"][0]["uri"] = "data:application/octet-stream;base64," + base64.b64encode(buffer).decode()
    gltf.write_text(json.dumps(obj_data))
    return obj, stl, dae, fbx, gltf, glb


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("converter", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--dwg-reader", type=Path, help="Optional GNU LibreDWG dwgread executable")
    args = parser.parse_args()
    exe = str(args.converter.resolve())

    def run(*args, fail=False):
        p = subprocess.run([exe, *map(str, args)], capture_output=True, text=True)
        if fail:
            assert p.returncode != 0, args
        elif p.returncode:
            raise AssertionError(f"{args}: {p.stderr or p.stdout}")
        return p

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        source = root / "model ÃƒÂ¸.dxf"
        fixture(source)
        errors = []
        for version, magic in VERSIONS.items():
            dxf = root / f"{version}.dxf"
            dwg = root / f"{version}.dwg"
            dwg_copy = root / f"{version}-copy.dwg"
            roundtrip = root / f"{version}-roundtrip.dxf"
            try:
                run("convert", source, dxf, version)
                legacy_units = version in ("R12", "R13", "R14")
                check_dxf(dxf, magic, None if legacy_units else 4)
                versioned_input = source if version == "R12" else dxf
                run(versioned_input, dwg, version)
                assert dwg.read_bytes().startswith(magic)
                run("convert", dwg, dwg_copy)
                assert dwg_copy.read_bytes().startswith(magic)
                run("convert", dwg_copy, roundtrip, "R2018")
                check_dxf(roundtrip, b"AC1032", 0 if legacy_units else 4)
                print(version, "PASS")
            except (AssertionError, ValueError, ezdxf.DXFError) as error:
                errors.append(version)
                print(version, "FAIL", repr(error))
                traceback.print_exc()

        binary = root / "binary.dxf"
        run("convert", "--binary-dxf", source, binary)
        assert binary.read_bytes().startswith(b"AutoCAD Binary DXF")
        # The binary header has its own framing, so validate content and
        # version through the independent reader instead of the ASCII helper.
        binary_model = ezdxf.readfile(binary)
        assert binary_model.dxfversion == "AC1032"
        assert len(binary_model.modelspace()) == 6
        run("convert", binary, root / "from-binary.dwg")

        for mesh in exchange_fixtures(root):
            target = root / f"{mesh.suffix[1:]}.dxf"
            try:
                result = run("convert", mesh, target, "R2018")
                assert "imported as mesh" in result.stderr
                mesh_entities = list(ezdxf.readfile(target).modelspace().query("MESH"))
                assert len(mesh_entities) == 1
                assert len(mesh_entities[0].vertices) == 3
                assert len(mesh_entities[0].faces) == 1
                print(mesh.suffix, "PASS")
            except (AssertionError, ValueError, ezdxf.DXFError) as error:
                errors.append(mesh.suffix)
                print(mesh.suffix, "FAIL", repr(error))
                traceback.print_exc()

        if args.baseline:
            for version, magic in VERSIONS.items():
                if version in ("R13", "R14"):
                    continue
                old = root / f"baseline-{version}.dwg"
                p = subprocess.run([str(args.baseline.resolve()), str(source),
                                    str(old), version], capture_output=True, text=True)
                if p.returncode:
                    errors.append(f"baseline-{version}")
                    print("Baseline failure", version, p.stderr or p.stdout)
                    continue
                assert old.read_bytes().startswith(magic)
                old_dxf = root / f"baseline-{version}.dxf"
                run("convert", old, old_dxf, "R2018")
                check_dxf(old_dxf, b"AC1032", 0 if version == "R12" else 4)
                print("Released CLI baseline", version, "PASS")

        if args.dwg_reader:
            for version in VERSIONS:
                drawing = root / f"independent-{version}.dwg"
                run("convert", source, drawing, version)
                result = subprocess.run(
                    [str(args.dwg_reader.resolve()), "-O", "JSON", str(drawing)],
                    capture_output=True, check=False,
                )
                if version == "R12" and result.returncode:
                    print("LibreDWG could not open R12 DWG (also observed on the published baseline)")
                    continue
                result.check_returncode()
                objects = json.loads(result.stdout)["OBJECTS"]
                kinds = Counter(entity.get("entity") for entity in objects)
                assert all(kinds[kind] == count for kind, count in {
                    "POINT": 1, "LINE": 2, "ARC": 1, "CIRCLE": 1,
                    "3DFACE": 1, "INSERT": 2,
                }.items())
                print("Independent LibreDWG entity inventory", version, "PASS")

        rich = root / "rich.dxf"
        rich_fixture(rich)
        check_rich(rich)
        direct = root / "rich-direct.dxf"
        run("convert", rich, direct)
        check_rich(direct)
        rich_dwg = root / "rich.dwg"
        run("convert", rich, rich_dwg)
        rich_back = root / "rich-back.dxf"
        run("convert", rich_dwg, rich_back)
        check_rich(rich_back)
        print("Modern entity, topology, metadata, paper space and precision PASS")

        large_source = root / "large.dxf"
        large_drawing = ezdxf.new("R2018")
        for i in range(20_000):
            large_drawing.modelspace().add_line((i, 0, i % 3), (i, 1, i % 3))
        large_drawing.saveas(large_source)
        large_dwg, large_dxf = root / "large.dwg", root / "large-back.dxf"
        run("convert", large_source, large_dwg)
        run("convert", large_dwg, large_dxf)
        recovered = ezdxf.readfile(large_dxf).modelspace().query("LINE")
        assert len(recovered) == 20_000
        assert recovered[-1].dxf.end.isclose((19999, 1, 1))
        print("20,000-entity drawing PASS")

        original = source.read_bytes()
        run("convert", source, source, fail=True)
        assert source.read_bytes() == original
        invalid = root / "broken.dwg"
        invalid.write_bytes(b"AC1032")
        protected = root / "protected.dxf"
        protected.write_bytes(b"previous output")
        run("convert", invalid, protected, fail=True)
        assert protected.read_bytes() == b"previous output"
        run("convert", source, root / "unsupported.dxf", "AC9999", fail=True)
        empty = root / "empty.dxf"
        ezdxf.new("R2018").saveas(empty)
        run("convert", empty, root / "empty.dwg")
        if errors:
            raise SystemExit(f"Version failures: {', '.join(errors)}")


if __name__ == "__main__":
    main()
