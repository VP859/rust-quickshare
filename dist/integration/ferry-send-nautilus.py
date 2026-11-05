"""Nautilus (GNOME Files) extension: "Send with Ferry" in the context menu of
files and folders. Needs nautilus-python (Fedora: nautilus-python, Debian and
Ubuntu: python3-nautilus); without it, `ferry integrate` installs a script in
the Scripts submenu instead."""

import locale
import os
import shutil
import subprocess

from gi.repository import GObject, Nautilus

COMMAND = "ferry-gui"


def _label():
    lang = (locale.getlocale()[0] or os.environ.get("LANG", "")).lower()
    return "Wyślij przez Ferry" if lang.startswith("pl") else "Send with Ferry"


def _local_paths(files):
    """Paths of the local files and folders; files on remote locations
    (sftp://, smb://) are left out, Ferry only sends local files."""
    paths = []
    for item in files:
        if item.get_uri_scheme() != "file":
            continue
        path = item.get_location().get_path()
        if path:
            paths.append(path)
    return paths


class FerrySendExtension(GObject.GObject, Nautilus.MenuProvider):
    def get_file_items(self, *args):
        # Nautilus 43+ passes (files), older versions (window, files).
        paths = _local_paths(args[-1])
        if not paths:
            return []
        item = Nautilus.MenuItem(name="Ferry::send", label=_label(), icon="ferry-gui")
        item.connect("activate", self._send, paths)
        return [item]

    def _send(self, _item, paths):
        command = shutil.which(COMMAND) or COMMAND
        subprocess.Popen([command, "--", *paths], start_new_session=True)
