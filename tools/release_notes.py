"""Print the workspace version's GitHub Release notes from CHANGELOG.md."""

import re
import sys

from check_versions import ROOT, pep440, workspace_version


def release_notes(changelog: str, version: str) -> str:
    # The comparison links follow the oldest entry in Keep a Changelog format.
    entries = changelog.partition("\n[Unreleased]:")[0]
    heading = rf"^## \[{re.escape(version)}\] - \d{{4}}-\d{{2}}-\d{{2}}$"
    sections = re.split(heading, entries, flags=re.MULTILINE)
    if len(sections) != 2:
        raise ValueError(f"expected one dated CHANGELOG.md section for {version}")
    notes = sections[1].partition("\n## ")[0].strip("\n")
    if not notes.strip():
        raise ValueError(f"empty CHANGELOG.md section for {version}")
    return (
        f"{notes}\n\n"
        f"[npm](https://www.npmjs.com/package/@grida/fx/v/{version}) · "
        f"[PyPI](https://pypi.org/project/grida/{pep440(version)}/)"
    )


if __name__ == "__main__":
    try:
        print(release_notes((ROOT / "CHANGELOG.md").read_text(), workspace_version(ROOT)))
    except ValueError as error:
        sys.exit(str(error))
