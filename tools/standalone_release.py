"""Build and check a standalone release archive from the tested npm engine and wheel."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import shutil
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path

from check_versions import parse
from package_release import TARGETS, clean_environment, digest


def package(engine: Path, wheel: Path, target: str, version: str, out: Path) -> Path:
    parse(version)
    package_name = TARGETS[target].removeprefix("@").replace("/", "-")
    if engine.name != f"{package_name}-{version}.tgz":
        raise ValueError("npm engine filename does not match the target and version")
    out.mkdir(parents=True, exist_ok=True)
    destination = out / f"grida-fx-{version}-{target}.tar.gz"
    if destination.exists():
        raise FileExistsError(destination)
    with tempfile.TemporaryDirectory(prefix=".fx-standalone-", dir=out) as directory:
        work = Path(directory).resolve()
        binary = work / "grida-fx"
        license_file = work / "LICENSE"
        with tarfile.open(engine, "r:gz") as npm:
            for member, path in (
                ("package/bin/grida-fx", binary),
                ("package/LICENSE", license_file),
            ):
                entry = npm.getmember(member)
                if not entry.isfile() or (path == binary and entry.mode & 0o777 != 0o755):
                    raise ValueError(f"npm archive has an unexpected member: {member}")
                stream = npm.extractfile(entry)
                assert stream is not None
                with stream, path.open("wb") as output:
                    shutil.copyfileobj(stream, output)
                path.chmod(entry.mode & 0o777)
        binary_digest = digest(binary)
        with zipfile.ZipFile(wheel) as python, python.open("grida/fx/_bin/grida-fx") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != binary_digest:
                raise ValueError("npm and PyPI packages carry different executables")
        archive_path = work / destination.name
        with (
            archive_path.open("wb") as output,
            gzip.GzipFile(filename="", fileobj=output, mode="wb", mtime=0) as compressed,
            tarfile.open(mode="w", fileobj=compressed) as archive,
        ):
            for path in (binary, license_file):
                entry = archive.gettarinfo(path, arcname=path.name)
                entry.uid = entry.gid = entry.mtime = 0
                entry.uname = entry.gname = ""
                with path.open("rb") as stream:
                    archive.addfile(entry, stream)
        binary.unlink()
        with tarfile.open(archive_path, "r:gz") as archive:
            archive.extract("grida-fx", work, filter="data")
        if digest(binary) != binary_digest or binary.stat().st_mode & 0o777 != 0o755:
            raise ValueError("standalone archive changed the executable or its permissions")
        printed = subprocess.check_output(
            [binary, "--version"], cwd=work, env=clean_environment(), text=True
        ).strip()
        if printed != f"grida-fx {version}":
            raise ValueError(f"standalone executable reported {printed!r}")
        archive_path.replace(destination)
    return destination


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--wheel", type=Path, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        print(package(args.engine, args.wheel, args.target, args.version, args.out))
    except (
        OSError,
        ValueError,
        KeyError,
        tarfile.TarError,
        zipfile.BadZipFile,
        subprocess.SubprocessError,
    ) as error:
        parser.exit(1, f"standalone_release: {error}\n")


if __name__ == "__main__":
    main()
