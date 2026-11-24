"""Package a tested converter or embed a manifest into a release copy of the extension."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "davidsmavrov-debug/blackfox-dwg-converter"
TARGETS = {
    f"{arch}-{platform}"
    for arch in ("x86_64", "aarch64")
    for platform in ("pc-windows-msvc", "unknown-linux-gnu", "apple-darwin")
}


def executable_name(target):
    return "BlackFoxDWG.exe" if target.endswith("windows-msvc") else "BlackFoxDWG"


def validate_binary(data, target):
    if len(data) < 64:
        raise ValueError(f"Truncated executable for {target}")
    arm = target.startswith("aarch64-")
    if target.endswith("windows-msvc"):
        offset = int.from_bytes(data[60:64], "little")
        magic, cpu = data[offset:offset + 4], data[offset + 4:offset + 6]
        matches = data[:2] == b"MZ" and magic == b"PE\0\0" and (
            int.from_bytes(cpu, "little") == (0xAA64 if arm else 0x8664))
    elif target.endswith("linux-gnu"):
        matches = data[:6] == b"\x7fELF\x02\x01" and (
            int.from_bytes(data[18:20], "little") == (183 if arm else 62))
    else:
        matches = data[:4] == b"\xcf\xfa\xed\xfe" and (
            int.from_bytes(data[4:8], "little") == (0x0100000C if arm else 0x01000007))
    if not matches:
        raise ValueError(f"Executable does not match {target}")


def dependency_notices(target):
    report = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked",
         "--filter-platform", target, "--manifest-path", str(ROOT / "Cargo.toml")],
        encoding="utf-8",
    )
    text = [(ROOT / "THIRD_PARTY_NOTICES.md").read_text(encoding="utf-8")]
    for package in sorted(json.loads(report)["packages"], key=lambda item: item["name"]):
        if package["name"] in {"blackfox-dwg-test", "acadrust"}:
            continue
        folder = Path(package["manifest_path"]).parent
        text.append(f"## {package['name']} {package['version']}\n\n"
                    f"Declared license: {package['license'] or package['license_file']}\n")
        files = {path for path in folder.iterdir() if path.is_file()
                 and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "UNLICENSE"))}
        if package["license_file"]:
            files.add(folder / package["license_file"])
        for path in sorted(files):
            text.append(f"### {path.name}\n\n" + path.read_text(encoding="utf-8", errors="replace"))
    return "\n\n".join(text)


def package(args):
    filename = executable_name(args.target)
    binary = args.binary.read_bytes()
    validate_binary(binary, args.target)
    args.output.mkdir(parents=True, exist_ok=True)
    destination = args.output / f"blackfox-dwg-{args.target}.zip"
    notices = dependency_notices(args.target)
    with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data, mode in (
            (filename, binary, 0o100755),
            ("LICENSE_AcadRust.txt", args.license.read_bytes(), 0o100644),
            ("THIRD_PARTY_NOTICES.md", notices.encode(), 0o100644),
        ):
            info = zipfile.ZipInfo(name)
            info.create_system = 3
            info.external_attr = mode << 16
            archive.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED)


def embed(args):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", args.version) or args.version == "1.0.0":
        raise ValueError("Provide a new numeric bidirectional converter release version")
    assets = {}
    for target in sorted(TARGETS):
        path = args.assets / f"blackfox-dwg-{target}.zip"
        name = executable_name(target)
        with zipfile.ZipFile(path) as archive:
            if sorted(archive.namelist()) != sorted((
                name, "LICENSE_AcadRust.txt", "THIRD_PARTY_NOTICES.md",
            )):
                raise ValueError(f"Unexpected archive members for {target}")
            validate_binary(archive.read(name), target)
        contents = path.read_bytes()
        assets[target] = {"size": len(contents), "sha256": hashlib.sha256(contents).hexdigest()}
    manifest = json.dumps(
        {"schema": 1, "repository": REPOSITORY, "abi": 2, "version": args.version,
         "assets": assets},
        sort_keys=True, separators=(",", ":"),
    ).encode()
    source = args.source.read_text(encoding="utf-8")
    for marker, value in (
        ('RELEASE_MANIFEST = b""', f"RELEASE_MANIFEST = {manifest!r}"),
        ('RELEASE_MANIFEST_SHA256 = ""',
         f"RELEASE_MANIFEST_SHA256 = {hashlib.sha256(manifest).hexdigest()!r}"),
    ):
        if source.count(marker) != 1:
            raise ValueError(f"Expected one release marker {marker}")
        source = source.replace(marker, value, 1)
    args.output.write_text(source, encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(required=True)
    build = commands.add_parser("package")
    build.add_argument("target", choices=sorted(TARGETS))
    build.add_argument("binary", type=Path)
    build.add_argument("license", type=Path)
    build.add_argument("output", type=Path)
    build.set_defaults(run=package)
    release = commands.add_parser("embed")
    release.add_argument("version")
    release.add_argument("assets", type=Path)
    release.add_argument("source", type=Path)
    release.add_argument("output", type=Path)
    release.set_defaults(run=embed)
    options = parser.parse_args()
    options.run(options)
