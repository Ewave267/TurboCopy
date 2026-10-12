"""Publish tested Linux packages, then remove every superseded GitHub release."""
import hashlib
import json
import os
from pathlib import Path
import subprocess


def gh(*arguments):
    return subprocess.run(["gh", *map(str, arguments)], check=True,
                          stdout=subprocess.PIPE, text=True).stdout


def publish(folder, repository, branch, sha, run_id, attempt):
    head = json.loads(gh("api", f"repos/{repository}/commits/{branch}"))["sha"]
    if head != sha:
        print("Skipping superseded build; the default branch has a newer commit.")
        return
    assets = [folder / f"tcopy-linux-{arch}.tar.gz" for arch in ("x86_64", "aarch64")]
    if any(not asset.is_file() or asset.stat().st_size == 0 for asset in assets):
        raise ValueError("Both Linux packages must be present and nonempty before publishing")
    checksums = folder / "SHA256SUMS"
    checksums.write_text("".join(
        f"{hashlib.sha256(asset.read_bytes()).hexdigest()}  {asset.name}\n" for asset in assets))
    tag = f"preview-{run_id}-{attempt}"
    notes = folder / "notes.md"
    notes.write_text(
        "Development prerelease of TurboCopy for Linux x86-64 and AArch64.\n\n"
        "Extract the matching archive and run `./tcopy --help`. Binaries use static musl linking.\n\n"
        f"Commit: [{sha[:12]}](https://github.com/{repository}/commit/{sha}).\n"
        f"[Build and tests](https://github.com/{repository}/actions/runs/{run_id}).\n\n"
        "Only the most recently published release is retained.\n")
    gh("release", "create", tag, "--target", sha, "--draft", "--prerelease",
       "--latest=false", "--title", f"TurboCopy development preview — {sha[:12]}",
       "--notes-file", notes)
    gh("release", "upload", tag, *assets, checksums)
    gh("release", "edit", tag, "--draft=false", "--prerelease", "--latest=false")
    # Publication must succeed before any previous release is removed.
    pages = json.loads(gh("api", "--paginate", "--slurp",
                         f"repos/{repository}/releases?per_page=100"))
    for page in pages:
        for release in page:
            if release["tag_name"] != tag:
                gh("release", "delete", release["tag_name"], "--yes")
    print(f"Published https://github.com/{repository}/releases/tag/{tag}")


if __name__ == "__main__":
    publish(Path("dist"), os.environ["GH_REPO"], os.environ["RELEASE_BRANCH"],
            os.environ["GITHUB_SHA"], os.environ["GITHUB_RUN_ID"],
            os.environ["GITHUB_RUN_ATTEMPT"])
