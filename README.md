# BlackFox DWG Converter

An open-source bidirectional DXF and DWG converter built in Rust using **AcadRust**.

## Overview

BlackFox DWG Converter provides a standalone conversion component for the **BlackFox CAD Exporter** extension for IngeTrazo.

The converter is distributed as platform-specific binaries and is downloaded by the extension only after the user grants permission.

## Features

- Bidirectional DXF and DWG conversion, with read-only mesh interchange inputs
- Standalone command-line converter
- Cross-platform binary distribution
- SHA-256 checksum verification by the BlackFox CAD Exporter
- No installation required when used through the extension

## Downloads

Precompiled binaries are available for:

- Windows â€” x86-64 and ARM64
- Linux â€” x86-64 and ARM64
- macOS â€” x86-64 and ARM64

Download the appropriate archive from the [Releases](https://github.com/davidsmavrov-debug/blackfox-dwg-converter/releases) page.

## Integration

The converter is used by [BlackFox CAD Exporter](https://github.com/ingelibre/ingetrazo), an extension for the IngeTrazo CAD application.

When DWG export is requested, the extension asks permission before downloading the converter, verifies the downloaded archive and caches the converter for subsequent use.

## Licensing

BlackFox DWG Converter uses third-party open-source components subject to their respective licences.

See the repository's licence and third-party notices for applicable terms.

## Project status

Version 1.0.0 is the published converter release.

This repository contains the standalone DWG conversion component used by BlackFox CAD Exporter.

## Bidirectional converter development

The development build keeps the existing package executable name
`BlackFoxDWG.exe` on Windows and `BlackFoxDWG` on Linux and macOS.
Cargo's internal target is `blackfox-dwg-test`.

- The published baseline is BlackFox DWG Converter v1.0.0, tag commit
  `665e9cfb35c34e2136a96a9ac5cdaafb2b44e401`. The local
  `baseline/cad-1.0.0` branch at `2e49045` keeps the same converter source
  with later documentation and licensing commits.
- AcadRust (MPL-2.0) is pinned to upstream commit
  `cc2010a415f3d2cfecf574f833febda0076c7572`. The newest stable tag is
  `v0.6.8` at `afffe4697b09f31f24080cc7a2a44928caeb3640`, which predates
  the existing BlackFox dependency at `17a3125153335eac19efa6409ace130a75c0bf54`.
  The new pin contains the existing dependency revision. For reproducible
  dependency resolution, build with the checked-in Cargo lockfile.

Build with `cargo build --release --locked`. The existing three-argument
invocation remains available, and the new commands are:

```text
BlackFoxDWG input.dxf output.dwg R2018
BlackFoxDWG convert input.dwg output.dxf
BlackFoxDWG convert --binary-dxf input.dwg output.dxf R2018
BlackFoxDWG inspect input.dxf
```

The standalone converter reads and writes DXF and DWG for AC1009 (R11/R12),
R13, R14, 2000, 2004, 2007, 2010, 2013, and 2018. A conversion without a
version argument keeps the input's CAD version. Output defaults to ASCII
DXF, with binary DXF available explicitly. OBJ, STL, COLLADA, FBX, glTF,
and GLB can be read through AcadRust's mesh importers and written to CAD
formats. These are geometry conversions with the limitations of the
upstream importers. They do not provide native mesh-interchange writers.

The converter preserves the source document model through the CAD reader
and writer. It reopens the staged output before replacing the destination,
verifies the format and version, and reports reader diagnostics, insertion
unit changes, missing layer and block names, and entity inventory changes.
These checks cannot establish lossless transfer of every property.
Down-saving to R12â€“R14 can omit the modern insertion-unit field, and
entities unsupported by older formats may be approximated or omitted.
The converter does not rescale coordinates on its own.

Run the independent DXF read-back benchmark with Python and ezdxf:

```text
python tests/roundtrip.py target/release/blackfox-dwg-test.exe --baseline path/to/BlackFoxDWG.exe
```

The optional baseline path should point to the verified v1.0.0 release
binary. DWG headers and the converter's ability to read its DWG output are
checked separately. Add `--dwg-reader path/to/dwgread.exe` to verify basic
entity inventories directly with GNU LibreDWG. LibreDWG 0.14 opened the
candidate's R13 through R2018 DWG fixtures. It rejected the R12 DWG fixture
from both this build and the published v1.0.0 binary.

## Platform builds and packaging

The build workflow compiles the checked-in lockfile and runs the round-trip
suite on native Windows, Linux and macOS runners for both x86-64 and ARM64.
The packaged archives contain the correctly named executable and AcadRust
notices. To package an already tested executable, use:

```text
python tests/package_release.py package <target> <binary> <pinned-AcadRust-LICENSE> <dist>
```

After collecting all six verified archives, generate an extension release
copy containing the pinned manifest and archive hashes:

```text
python tests/package_release.py embed <version> <dist> <extension-source.py> <extension-release.py>
```

For Linux cross-builds, cargo-zigbuild 0.24.1 with Zig 0.15.2 can build
against a glibc 2.17 runtime baseline:

```text
cargo zigbuild --release --locked --target x86_64-unknown-linux-gnu.2.17
cargo zigbuild --release --locked --target aarch64-unknown-linux-gnu.2.17
```

Inspect the resulting ELF version requirements. The Rust target's
minimum glibc version alone does not qualify a binary built against a
newer distribution. Native CI logs the actual version requirements.

The macOS builds request a deployment target of 11.0. Qualify that floor
and the dynamic libraries on both architectures. Sign and notarize the
final distributable where required and hash the resulting final binaries.
[Apple signing and notarization guidance](https://developer.apple.com/developer-id/)

## Direct IngeTrazo exchange

ABI 3 uses the same AcadRust executable for geometry exchange in both
directions. The Python adapter sends and receives schema 1 JSON. The
converter writes DXF or DWG directly.

```text
BlackFoxDWG decode input.dwg model.json
BlackFoxDWG encode model.json output.dxf R2018
```

The direct implementation covers lines and polylines, faces and meshes,
guides, simple text leaders, aligned and world-axis linear dimensions,
and nested block placements. Arcs, circles, ellipses and lightweight
polyline bulges import as segmented contours. Standard analytic and
architectural records outside the supported mapping are reported. The
ACIS adapter produces an editable mesh for a single planar body with
straight, single-loop face boundaries. Curved, transformed or holed ACIS
bodies require additional mapping. The six mesh interchange inputs are
read-only.

Run the Rust tests and the extension's `tests/direct_exchange.py` suite.
The integration suite blocks all imports of ezdxf and verifies native
objects and the normal IngeTrazo command entry points on both engines.

### Local R2010 surface reader fix

The Windows preview 1.2.6 applies `patches/acadrust-r2010-surface.patch`
to the pinned AcadRust revision. Normal decoding remains the first path.
For generic R2010 surfaces, the fallback checks the inline SAB body with
AcadRust and accepts the compact trailer only when it consumes exactly
the remaining main-stream bits. The fallback retains both isoline counts.

To rebuild the candidate, apply the patch to a separate checkout of the
pinned AcadRust revision. In a disposable copy of the converter source,
replace its AcadRust git dependency with a path to that checkout, keeping
the same feature flags, and run `cargo build`. The published dependency pin
and source assets are unchanged. The matching DXF was used only for QA.

The acceptance DWG yielded all eleven native editable objects. The
comparison with the DXF matched shape bounds, positions and topology,
including the closed torus and both hollow-cylinder end openings.
