"""Checks dist/integration/ferry-send-nautilus.py without Nautilus: the `gi`
modules are replaced by stand-ins, so this runs anywhere with Python 3.

    python3 -I tests/integration/test_nautilus_extension.py
"""

import importlib.util
import pathlib
import sys
import types

EXTENSION = pathlib.Path(__file__).resolve().parents[2] / "dist" / "integration" / "ferry-send-nautilus.py"


class MenuItem:
    def __init__(self, name, label, icon=None):
        self.name, self.label, self.icon, self.handlers = name, label, icon, []

    def connect(self, signal, handler, *data):
        self.handlers.append((signal, handler, data))


class Location:
    def __init__(self, path):
        self.path = path

    def get_path(self):
        return self.path


class FileInfo:
    def __init__(self, uri):
        self.uri = uri

    def get_uri_scheme(self):
        return self.uri.split(":", 1)[0]

    def get_location(self):
        return Location(self.uri[len("file://"):] if self.uri.startswith("file://") else None)


def load():
    gi = types.ModuleType("gi")
    repository = types.ModuleType("gi.repository")
    repository.GObject = types.SimpleNamespace(GObject=type("GObject", (), {}))
    repository.Nautilus = types.SimpleNamespace(
        MenuProvider=type("MenuProvider", (), {}), MenuItem=MenuItem
    )
    gi.repository = repository
    sys.modules.update({"gi": gi, "gi.repository": repository})
    spec = importlib.util.spec_from_file_location("ferry_send", EXTENSION)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    module = load()
    ext = module.FerrySendExtension()
    files = [FileInfo("file:///home/me/a b.jpg"), FileInfo("sftp://host/x"), FileInfo("file:///home/me/Album")]

    # Nautilus 43+ passes (files), older versions (window, files).
    for args in ((files,), (object(), files)):
        items = ext.get_file_items(*args)
        assert len(items) == 1, items
        item = items[0]
        assert item.label in ("Send with Ferry", "Wyślij przez Ferry"), item.label
        signal, handler, data = item.handlers[0]
        assert signal == "activate"
        assert data == (["/home/me/a b.jpg", "/home/me/Album"],), data

    assert ext.get_file_items([FileInfo("sftp://host/x")]) == [], "remote files only: no entry"

    launched = []
    module.subprocess.Popen = lambda command, **kw: launched.append((command, kw))
    module.shutil.which = lambda name: None
    handler(item, ["/home/me/a b.jpg"])
    command, options = launched[0]
    assert command == ["ferry-gui", "--", "/home/me/a b.jpg"], command
    assert options.get("start_new_session") is True
    print("ok  nautilus extension")


if __name__ == "__main__":
    main()
