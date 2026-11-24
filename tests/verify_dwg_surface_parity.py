"""Compare the standalone DWG acceptance drawing against its DXF reference."""
import collections
import json
import math
import sys
from pathlib import Path


def signature(entity):
    vertices = entity["vertices"]
    bounds = tuple(value for axis in zip(*vertices) for value in (min(axis), max(axis)))
    holes = sum(len(rings) for rings in entity.get("holes", ()))
    euler = None
    if not holes:
        ids = {}
        mapped = [ids.setdefault(tuple(round(v, 6) for v in point), len(ids)) for point in vertices]
        edges = set()
        for face in entity["faces"]:
            for a, b in zip(face, face[1:] + face[:1]):
                edges.add(tuple(sorted((mapped[a], mapped[b]))))
        euler = len(ids) - len(edges) + len(entity["faces"])
    return bounds, len(entity.get("faces", ())), holes, euler


def main(dxf_file, dwg_file):
    dxf = json.loads(Path(dxf_file).read_text(encoding="utf-8"))
    dwg = json.loads(Path(dwg_file).read_text(encoding="utf-8"))
    for name, drawing in (("DXF", dxf), ("DWG", dwg)):
        assert len(drawing["entities"]) == 11, (name, len(drawing["entities"]))
        assert not drawing["omitted"], drawing["omitted"]
    remaining = [signature(record) for record in dwg["entities"]]
    for reference in map(signature, dxf["entities"]):
        candidates = [other for other in remaining if
                      other[1:] == reference[1:] and all(math.isclose(a,b,abs_tol=1e-5)
                      for a,b in zip(reference[0],other[0]))]
        assert len(candidates) == 1, reference
        remaining.remove(candidates[0])
    assert not remaining
    print("All 11 DWG shapes match the DXF in placement, bounds, face counts, holes and Euler characteristic.")
    print("Closed torus:", [entry for entry in map(signature, dwg["entities"]) if entry[-1] == 0])
    print("Cylinder openings:", sum(signature(entity)[2] for entity in dwg["entities"]))


if __name__ == "__main__":
    main(*sys.argv[1:])
