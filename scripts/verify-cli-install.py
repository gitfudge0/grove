#!/usr/bin/env python3
"""Exercise the real installer with fake build tools and disposable destinations."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def executable(path, body):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o755)


with tempfile.TemporaryDirectory(prefix="grove-cli-install-") as directory:
    root = Path(directory)
    checkout = root / "checkout"
    checkout.mkdir()
    (checkout / "Cargo.toml").touch()
    applications = root / "Applications"
    applications.mkdir()
    home = root / "home"
    cargo_home = home / "custom cargo"
    tools = root / "tools"
    tools.mkdir()
    # Only redirect installation destinations; run the complete real script.
    for name in ("install.sh", "uninstall.sh"):
        text = (ROOT / name).read_text().replace('"/Applications"', f'"{applications}"')
        text = text.replace('/Applications/Grove.app/', f'{applications}/Grove.app/')
        (checkout / name).write_text(text)
    executable(tools / "uname", '#!/bin/sh\necho Darwin\n')
    executable(tools / "sysctl", '#!/bin/sh\necho 1\n')
    executable(tools / "rustup", '#!/bin/sh\nexit 0\n')
    executable(tools / "xattr", '#!/bin/sh\nexit 0\n')
    executable(tools / "security", '#!/bin/sh\nexit 1\n')
    executable(tools / "cargo", '''#!/bin/sh
if [ "$1" = bundle ] && [ "$2" != --help ]; then
  mkdir -p target/aarch64-apple-darwin/release/bundle/osx/Grove.app/Contents/MacOS
  cp "$FIXTURE_BINARY" target/aarch64-apple-darwin/release/bundle/osx/Grove.app/Contents/MacOS/grove
fi
''')
    fixture = root / "current-grove"
    executable(fixture, '#!/bin/sh\nprintf "current:%s\\n" "$@"\n')
    cli = cargo_home / "bin/grove"
    executable(cli, '#!/bin/sh\necho stale-gui\n')
    env = dict(os.environ, HOME=str(home), CARGO_HOME=str(cargo_home),
               CARGO_TARGET_DIR="target", PATH=f"{tools}:{cargo_home / 'bin'}:/usr/bin:/bin", FIXTURE_BINARY=str(fixture))

    def run(script):
        result = subprocess.run(["/bin/bash", str(checkout / script)], env=env,
                                capture_output=True, text=True)
        print(result.stdout, end="")
        print(result.stderr, end="")
        assert result.returncode == 0, result.returncode

    run("install.sh")
    assert cli.is_symlink(), "installer retained stale executable"
    assert cli.resolve() == (applications / "Grove.app/Contents/MacOS/grove").resolve()
    arguments = ["projects", "list", "--json", "space preserved"]
    result = subprocess.run([str(cli), *arguments], env=env, capture_output=True, text=True, check=True)
    assert result.stdout.splitlines() == [f"current:{arg}" for arg in arguments]
    executable(fixture, '#!/bin/sh\necho updated\n')
    run("install.sh")
    assert subprocess.check_output([str(cli), "--help"], env=env, text=True) == "updated\n"
    run("uninstall.sh")
    assert not cli.exists() and not cli.is_symlink(), "uninstall left dangling managed CLI"
    foreign = root / "foreign"
    executable(foreign, '#!/bin/sh\nexit 0\n')
    cli.symlink_to(foreign)
    run("uninstall.sh")
    assert cli.resolve() == foreign.resolve(), "uninstall removed foreign symlink"
    # Run the exact helper on aliased paths and conflicting directories too.
    installer = (ROOT / "install.sh").read_text()
    helper = installer[installer.index("install_cli_link() {"):installer.index('OS="$(uname -s)"')]
    same = cargo_home / "bin/grove"
    same.unlink()
    executable(same, '#!/bin/sh\nexit 0\n')
    subprocess.run(["/bin/bash", "-c", helper + '\ninstall_cli_link "$1"', "test", str(same)],
                   env=env, check=True)
    assert not same.is_symlink(), "same-path install created a self-link"
    same.unlink()
    same.mkdir()
    result = subprocess.run(["/bin/bash", "-c", helper + '\ninstall_cli_link "$1"', "test", str(foreign)],
                            env=env, capture_output=True, text=True)
    assert result.returncode != 0 and not list(same.iterdir()), "directory conflict was not rejected"
    same.rmdir()
    executable(tools / "uname", '#!/bin/sh\necho Linux\n')
    executable(tools / "cargo", '''#!/bin/sh
if [ "$1" = bundle ] && [ "$2" != --help ]; then
  mkdir -p target/release/bundle/deb
  cp "$FIXTURE_BINARY" target/release/grove
  if [ "$FIXTURE_DEB" = 1 ]; then touch target/release/bundle/deb/grove.deb; fi
fi
''')
    executable(tools / "dpkg", '#!/bin/sh\nexit 0\n')
    executable(tools / "sudo", '#!/bin/sh\n"$@"\n')
    executable(tools / "install", '''#!/bin/sh
shift
source="$1"
destination="$2"
mkdir -p "$(dirname "$destination")"
cp "$source" "$destination"
chmod 755 "$destination"
''')
    (checkout / "assets/icon").mkdir(parents=True)
    (checkout / "assets/icon/512x512.png").touch()
    # Fallback runs with no generated .deb.
    env["FIXTURE_DEB"] = "0"
    run("install.sh")
    assert cli.resolve() == (home / ".local/bin/grove").resolve()
    run("uninstall.sh")
    # Mock dpkg installation destination without writing system paths.
    packaged = root / "usr/bin/grove"
    executable(packaged, '#!/bin/sh\necho packaged\n')
    text = (checkout / "install.sh").read_text().replace('"/usr/bin/grove"', f'"{packaged}"')
    (checkout / "install.sh").write_text(text)
    env["FIXTURE_DEB"] = "1"
    run("install.sh")
    assert cli.resolve() == packaged.resolve()
    print("PASS: stale command replacement, argument preservation, updates, managed cleanup, foreign link preservation, same-path safety, Linux fallback and deb")
