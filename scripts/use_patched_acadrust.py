"""Switch Cargo to the checked-out and patched AcadRust for release builds."""

from pathlib import Path

revision = "cc2010a415f3d2cfecf574f833febda0076c7572"
url = "https://github.com/hakanaktt/acadrust.git"
changes = [
    (Path("Cargo.toml"), f'git = "{url}", rev = "{revision}"', 'path = "acadrust-source"'),
    (Path("Cargo.lock"), f'source = "git+{url}?rev={revision}#{revision}"\n', ""),
]
updated = []
for path, original, replacement in changes:
    contents = path.read_text(encoding="utf-8")
    if contents.count(original) != 1:
        raise SystemExit(f"Expected exactly one pinned AcadRust reference in {path}")
    updated.append((path, contents.replace(original, replacement, 1)))

for path, contents in updated:
    path.write_text(contents, encoding="utf-8", newline="\n")
