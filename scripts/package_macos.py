#!/usr/bin/env python3
"""Build a native App and optional DMG; never install or launch it.

Only a newly created dist subdirectory is populated. No cleanup, overwrite of
previous packages, persistent settings changes, or embedded signing credentials.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import sys
import tempfile


def run(*command):
    subprocess.run(command, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sign-identity", default=os.environ.get("KEYFLUX_SIGN_IDENTITY", "-"),
                        help="Developer ID Application identity; default '-' is local ad-hoc signing")
    parser.add_argument("--notary-profile", default=os.environ.get("KEYFLUX_NOTARY_PROFILE"),
                        help="Existing notarytool Keychain profile; enables notarization and stapling")
    parser.add_argument("--bundle-id", default="org.keyflux.KeyFlux")
    parser.add_argument("--app-only", action="store_true", help="Create a development App under target/dev-apps; skip DMG and notarization")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("Packaging requires macOS and the Xcode command line tools")
    if args.notary_profile and args.sign_identity == "-":
        parser.error("Notarization requires a Developer ID Application identity")
    if args.app_only and args.notary_profile:
        parser.error("Development App mode does not submit notarization requests")
    for tool in ("cargo", "codesign", "hdiutil", "xcrun"):
        if not shutil.which(tool):
            parser.error(f"Required tool not found: {tool}")

    root = Path(__file__).resolve().parents[1]
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--no-deps", "--offline", "--locked", "--format-version", "1",
        "--manifest-path", str(root / "Cargo.toml")], text=True))
    package = next(package for package in metadata["packages"] if Path(package["manifest_path"]) == root / "Cargo.toml")
    version = package["version"]
    target = {"arm64": "aarch64-apple-darwin", "x86_64": "x86_64-apple-darwin"}.get(platform.machine())
    if target is None:
        parser.error(f"Unsupported build architecture: {platform.machine()}")
    # Explicit target directory matches the binary copied below; environment
    # CARGO_TARGET_DIR must not accidentally select an older build elsewhere.
    run("cargo", "build", "--release", "--locked", "--manifest-path", str(root / "Cargo.toml"),
        "--target-dir", str(root / "target"), "--target", target)
    binary = root / "target" / target / "release/KeyFlux"
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    artifact_root = root / "target/dev-apps" if args.app_only else root / "dist"
    artifact_root.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix=f"KeyFlux-{version}-{platform.machine()}-{stamp}-", dir=artifact_root))
    staging = output / "image"
    app = staging / "KeyFlux.app"
    macos = app / "Contents/MacOS"
    resources = app / "Contents/Resources"
    macos.mkdir(parents=True)
    resources.mkdir()
    shutil.copy2(binary, macos / "KeyFlux")
    with (root / "packaging/macos/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    info["CFBundleIdentifier"] = args.bundle_id
    info["CFBundleShortVersionString"] = version
    info["CFBundleVersion"] = version
    # Rust's target may require a newer macOS than the template. Never claim
    # compatibility below the actual Mach-O deployment target.
    load_commands = subprocess.check_output(["xcrun", "vtool", "-show-build", str(binary)], text=True)
    for line in load_commands.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0] == "minos":
            info["LSMinimumSystemVersion"] = parts[1]
            break
    with (app / "Contents/Info.plist").open("wb") as target:
        plistlib.dump(info, target)
    for source in (root / "LICENSE", root / "docs/THIRD_PARTY_NOTICES.md"):
        shutil.copy2(source, resources / source.name)
    shutil.copy2(root / "packaging/macos/安装说明.txt", staging / "安装说明.txt")
    (staging / "Applications").symlink_to("/Applications", target_is_directory=True)

    sign_options = [] if args.sign_identity == "-" else ["--options", "runtime", "--timestamp"]
    # --force only replaces the linker signature on our new staging copy.
    # The build binary, previously produced packages and installed apps are untouched.
    run("codesign", "--force", "--sign", args.sign_identity, "--identifier", args.bundle_id,
        *sign_options, str(app))
    run("codesign", "--verify", "--strict", "--verbose=2", str(app))
    if args.app_only:
        print(f"\nDevelopment App: {app}\nNo DMG created. No App launched or installed.")
        return
    if args.notary_profile:
        archive = output / "KeyFlux-notarization.zip"
        run("ditto", "-c", "-k", "--keepParent", str(app), str(archive))
        run("xcrun", "notarytool", "submit", str(archive), "--keychain-profile", args.notary_profile, "--wait")
        run("xcrun", "stapler", "staple", str(app))
        run("xcrun", "stapler", "validate", str(app))

    image = output / f"KeyFlux-{version}-{platform.machine()}.dmg"
    run("hdiutil", "create", "-volname", "KeyFlux", "-srcfolder", str(staging), "-format", "UDZO", str(image))
    if args.sign_identity != "-":
        run("codesign", "--sign", args.sign_identity, "--timestamp", str(image))
    if args.notary_profile:
        run("xcrun", "notarytool", "submit", str(image), "--keychain-profile", args.notary_profile, "--wait")
        run("xcrun", "stapler", "staple", str(image))
        run("xcrun", "stapler", "validate", str(image))
    run("hdiutil", "verify", str(image))
    digest = hashlib.sha256(image.read_bytes()).hexdigest()
    (output / f"{image.name}.sha256").write_text(f"{digest}  {image.name}\n")
    print(f"\nApp: {app}\nDMG: {image}\nSHA256: {digest}")
    print("Signing: " + ("local ad-hoc; not notarized" if args.sign_identity == "-" else args.sign_identity))
    if not args.notary_profile:
        print("Not notarized: this artifact is for testing, not a notarized public release.")


if __name__ == "__main__":
    main()
