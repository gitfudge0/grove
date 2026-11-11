#!/usr/bin/env python3
"""Test real GSettings notifications in a private bus and disposable dconf DB."""
import os
from pathlib import Path
import shlex
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="grove-text-preferences-") as directory:
    temporary = Path(directory)
    binary = temporary / "text-preferences"
    flags = shlex.split(subprocess.check_output(
        ["pkg-config", "--cflags", "--libs", "gio-2.0"], text=True))
    subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                    str(ROOT / "tests/native/linux_text_preferences.c"),
                    str(ROOT / "src/platform/text_preferences_linux.c"),
                    "-o", str(binary), *flags, "-lm"], check=True)
    schemas = temporary / "schemas"
    schemas.mkdir()
    (schemas / "org.gnome.desktop.interface.gschema.xml").write_text('''
<schemalist><schema id="org.gnome.desktop.interface" path="/org/gnome/desktop/interface/">
<key name="text-scaling-factor" type="d"><default>1.0</default></key>
</schema></schemalist>
''')
    subprocess.run(["glib-compile-schemas", str(schemas)], check=True)
    profile = temporary / "dconf-profile"
    profile.write_text("user-db:grovetexttest\n")
    environment = dict(os.environ, XDG_CONFIG_HOME=str(temporary / "config"),
                       XDG_DATA_DIRS=str(temporary / "data"),
                       GSETTINGS_SCHEMA_DIR=str(schemas), GSETTINGS_BACKEND="dconf",
                       DCONF_PROFILE=str(profile), GIO_USE_VFS="local")
    subprocess.run(["dbus-run-session", "--", str(binary)], env=environment, check=True)
    # No compiled schemas and no system schema directories: init must be safe.
    empty = temporary / "empty"
    empty.mkdir()
    environment["GSETTINGS_SCHEMA_DIR"] = str(empty)
    subprocess.run([str(binary), "missing-schema"], env=environment, check=True)
