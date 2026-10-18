#!/usr/bin/env python3
"""Starts DRH Launcher on a made-up install and mod index, showing every case
of the Mods page: updates, withdrawn versions, missing, disabled or unfitting
dependencies, mods built for another DRH, mods not from the index, and a
catalog to install from. Linux only: the launcher's folders follow XDG there.

    python3 scripts/mods-demo.py          a fresh demo
    python3 scripts/mods-demo.py --keep   the last one, as the launcher left it

Everything lives in target/mods-demo/; your own install is not touched. The
index is served on a free local port; the zips install for real. DRH is a
stand-in V13: Play starts nothing.
"""

import hashlib
import http.server
import io
import json
import os
import shutil
import subprocess
import sys
import threading
import zipfile
from functools import partial

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ROOT = os.path.join(REPO, "target", "mods-demo")
INSTALL = os.path.join(ROOT, "data", "drhlauncher")
MODS = os.path.join(INSTALL, "mods")
INDEX = os.path.join(ROOT, "index")
LAUNCHER = os.path.join(REPO, "target", "debug", "DRH-Launcher")
API_SOURCE = "https://github.com/Tutez64/DRH-Mod-API"


def readme(name, body):
    return f"# {name}\n\n<!-- drh:show -->\n{body}\n<!-- /drh:show -->\n"


LONG = """A longer description, to see the page scroll and Markdown render.

## Features

- First thing it does
- Second thing, with **bold** and `code`
- A [link](https://github.com/Tutez64/Dungeon-Rampage-Haxe)

## Usage

1. Install it
2. Enable it
3. Play

```haxe
class Main extends mods.api.Mod {
\toverride public function init(context:Context):Void {
\t\tcontext.log("hello");
\t}
}
```

> A quote, for good measure.
"""

# Every mod the index lists: (id, name, version, author, owner, description,
# api, drh, uses, dependencies, yanked, long description).
INDEXED = [
    ("api", "API", "0.1.0", "Tutez", "Tutez64",
     "The official API every mod builds on: a stable layer over the game, so mods keep working across its updates.",
     None, "13-14", ["extends"], {}, None, "The official API."),
    ("api", "API", "0.1.1", "Tutez", "Tutez64",
     "The official API every mod builds on: a stable layer over the game, so mods keep working across its updates.",
     None, "13-14", ["extends"], {}, None, "The official API, 0.1.1."),
    ("party_board", "Party Board", "0.1.0", "Tutez", "Tutez64",
     "See players' info (level, weapons, ...) and act on them (friend, block, report) in a dungeon.",
     "0.1", None, [], {}, None, "In a dungeon, press P to see all the players inside it."),
    ("party_board", "Party Board", "0.2.0", "Tutez", "Tutez64",
     "See players' info (level, weapons, ...) and act on them (friend, block, report) in a dungeon.",
     "0.1", None, [], {}, None, "In a dungeon, press P to see all the players inside it.\n\nNew in 0.2: sorting."),
    ("hud_tweaks", "HUD Tweaks", "1.2.0", "Mira", "mira-mods",
     "Moves and resizes parts of the HUD: health, cooldowns, minimap.",
     "0.1", "13", ["extends"], {}, None, LONG),
    ("boss_rework", "Boss Rework", "0.3.0", "Kael", "kael-dev",
     "Rewrites the bosses' attack patterns for a harder fight.",
     "0.1", "14", ["replace"], {}, None, "Built for DRH 14 only."),
    ("text_lib", "Text Lib", "0.2.3", "Nox", "nox",
     "Text helpers other mods use: formatting, colors, wrapping.",
     "0.1", None, [], {}, None, "A library: install a mod that needs it."),
    ("chat_macros", "Chat Macros", "1.0.0", "Nox", "nox",
     "Bind chat messages to keys.",
     "0.1", None, [], {"text_lib": "0.2"}, None, "Needs Text Lib, installed with it."),
    ("old_mod", "Old Mod", "1.0.0", "Someone", "someone",
     "A mod whose installed version was withdrawn from the index.",
     "0.1", None, [], {}, "It corrupted saves on logout.", "Withdrawn version."),
    ("old_mod", "Old Mod", "1.1.0", "Someone", "someone",
     "A mod whose installed version was withdrawn from the index.",
     "0.1", None, [], {}, None, "The fixed version."),
    ("very_long_named_mod", "A Mod With A Very Long Name That Does Not Fit On A Card", "10.20.30",
     "An Author With A Long Name", "another-owner",
     "A description as long as the index allows, to see how a card and a row show it: one hundred and twenty characters, tops.",
     "0.1", None, [], {}, None, LONG),
    ("loot_filter", "Loot Filter", "0.4.0", "Mira", "mira-mods",
     "Hides the loot you never pick up.",
     "0.1", None, [], {}, None, "Filters loot."),
    ("damage_meter", "Damage Meter", "2.0.1", "Kael", "kael-dev",
     "Shows the party's damage per second.",
     "0.1", "13-14", ["extends"], {}, None, "DPS meter."),
    ("damage_meter", "Damage Meter", "2.1.0", "Kael", "kael-dev",
     "Shows the party's damage per second.",
     "0.1", "14", ["extends"], {}, None, "Only for DRH 14: the Catalog offers 2.0.1 on DRH 13."),
]


def mod_json(id, name, version, author, description, api, drh, uses, deps):
    fields = {"id": id, "name": name, "description": description, "version": version,
              "author": author}
    if api is not None:
        fields["api"] = api
    if drh is not None:
        fields["drh"] = drh
    fields["entry"] = "Main"
    fields["uses"] = uses
    if deps:
        fields["dependencies"] = deps
    return json.dumps(fields, indent=2) + "\n"


def mod_files(id, name, version, author, description, api, drh, uses, deps, long):
    return {
        "mod.json": mod_json(id, name, version, author, description, api, drh, uses, deps),
        "README.md": readme(name, long),
        "LICENSE": "Demo.\n",
        "src/Main.hx": f"package mods.{id};\n\nclass Main extends mods.api.Mod {{}}\n",
    }


def write_folder(folder, files):
    for path, text in files.items():
        full = os.path.join(folder, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w") as file:
            file.write(text)


def zip_bytes(files):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for path in sorted(files):
            archive.writestr(path, files[path])
    return buffer.getvalue()


def source_of(id, name, owner):
    if id == "api":
        return API_SOURCE
    return f"https://github.com/{owner}/DRH-Mod-{name.replace(' ', '-')}"


def write_index(base):
    """The index and its zips, whose URLs name the server's port."""
    shutil.rmtree(INDEX, ignore_errors=True)
    os.makedirs(INDEX)
    entries = []
    for (id, name, version, author, owner, description, api, drh, uses, deps, yanked,
         long) in INDEXED:
        files = mod_files(id, name, version, author, description, api, drh, uses, deps, long)
        data = zip_bytes(files)
        zip_name = f"{id}-{version}.zip"
        with open(os.path.join(INDEX, zip_name), "wb") as file:
            file.write(data)
        entry = {"id": id, "name": name, "version": version, "author": author,
                 "description": description, "longDescription": long}
        if api is not None:
            entry["api"] = api
        if drh is not None:
            entry["drh"] = drh
        entry["uses"] = uses
        if deps:
            entry["dependencies"] = deps
        entry.update({"url": f"{base}/{zip_name}", "size": len(data),
                      "sha256": hashlib.sha256(data).hexdigest(),
                      "source": source_of(id, name, owner)})
        if yanked:
            entry["yanked"] = yanked
        entries.append(entry)
    with open(os.path.join(INDEX, "index.json"), "w") as file:
        json.dump({"format": 1, "generated": "2026-10-18T00:00:00Z", "entries": entries},
                  file, indent=2)


def write_install():
    """The launcher's config and install: DRH V13 and the mods folder."""
    shutil.rmtree(os.path.join(ROOT, "config"), ignore_errors=True)
    shutil.rmtree(os.path.join(ROOT, "data"), ignore_errors=True)
    os.makedirs(os.path.join(ROOT, "config"))
    os.makedirs(MODS)

    def indexed(id, version):
        for row in INDEXED:
            if row[0] == id and row[2] == version:
                (id, name, version, author, _, description, api, drh, uses, deps, _,
                 long) = row
                write_folder(os.path.join(MODS, id), mod_files(
                    id, name, version, author, description, api, drh, uses, deps, long))
                return
        raise KeyError((id, version))

    def local(id, name, description, api="0.1", deps=None, raw=None):
        files = mod_files(id, name, "1.0.0", "Me", description, api, None, [], deps or {},
                          f"{name}, copied by hand.")
        if raw is not None:
            files["mod.json"] = raw
        write_folder(os.path.join(MODS, id), files)

    # From the index.
    indexed("api", "0.1.0")
    indexed("party_board", "0.1.0")
    indexed("old_mod", "1.0.0")
    indexed("boss_rework", "0.3.0")
    indexed("text_lib", "0.2.3")
    indexed("hud_tweaks", "1.2.0")
    # Was a dependency of a mod since uninstalled, and is no longer listed.
    write_folder(os.path.join(MODS, "orphan_lib"), mod_files(
        "orphan_lib", "Orphan Lib", "0.1.0", "Nox", "A library nothing needs any more.",
        "0.1", None, [], {}, "Orphan."))

    # Not from the index.
    local("needs_ghost", "Needs Ghost", "Needs a mod that is not installed.",
          deps={"ghost_lib": "1.0"})
    local("needs_text", "Needs Text", "Needs Text Lib, which is disabled.",
          deps={"text_lib": "0.2"})
    local("wants_new_api", "Wants New API", "Needs api 1.0; 0.1.1 is the newest.",
          api="1.0")
    local("from_zip", "From A Zip",
          "Installed from a zip, which no one checked: its description runs past the 120"
          " characters the index allows, so the launcher cuts it.")
    local("from_repo", "From A Repository", "Installed from a GitHub repository link.")
    local("broken_json", "Broken Json", "", raw=json.dumps(
        {"id": "broken_json", "name": "Broken Json", "version": "1.0.0", "api": 1}))
    local("plain_disabled", "Plain Disabled", "A mod that is installed but disabled.")

    enabled = ["api", "party_board", "old_mod", "boss_rework", "needs_ghost", "needs_text",
               "wants_new_api", "from_zip", "from_repo", "broken_json", "orphan_lib",
               "hud_tweaks"]
    with open(os.path.join(MODS, "enabled.json"), "w") as file:
        json.dump({"mods": [{"id": id} for id in enabled]}, file, indent=2)

    sources = {row[0]: source_of(row[0], row[1], row[4]) for row in INDEXED}
    state = {
        "mods": {
            "api": {"from": "index", "dependency": True},
            "party_board": {"from": "index"},
            "old_mod": {"from": "index"},
            "boss_rework": {"from": "index"},
            "hud_tweaks": {"from": "index"},
            "text_lib": {"from": "index", "dependency": True},
            "orphan_lib": {"from": "index", "dependency": True},
            "from_zip": {"from": "zip"},
            "from_repo": {"from": "repository",
                          "url": "https://github.com/me/DRH-Mod-From-A-Repository"},
        },
        # Resolved for the installed tag: no automatic refresh on open.
        "tag": 13,
    }
    for id, record in state["mods"].items():
        if record["from"] == "index":
            record["source"] = sources.get(id, f"https://github.com/nox/DRH-Mod-{id}")
    os.makedirs(os.path.join(INSTALL, "data"))
    with open(os.path.join(INSTALL, "data", "mods.json"), "w") as file:
        json.dump(state, file, indent=2)

    # DRH V13: what the launcher records, and a stand-in executable.
    installed = {"active": {
        "version": "V13", "platform": "linux-x64", "source": "Tutez64/Dungeon-Rampage-Haxe",
        "release_url": "https://github.com/Tutez64/Dungeon-Rampage-Haxe/releases/tag/V13",
        "archive": "demo.tar.gz", "archive_sha256": "0" * 64, "archive_size": 0,
        "installed_at": "2026-10-18 00:00:00 UTC"}}
    with open(os.path.join(INSTALL, "data", "installed.json"), "w") as file:
        json.dump(installed, file, indent=2)
    game = os.path.join(INSTALL, "Dungeon Rampage Haxe", "current")
    os.makedirs(game)
    executable = os.path.join(game, "Dungeon Rampage Haxe")
    with open(executable, "w") as file:
        file.write("#!/bin/sh\necho 'Mods demo: no game here.'\n")
    os.chmod(executable, 0o755)


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def main():
    if not sys.platform.startswith("linux"):
        sys.exit("The demo runs on Linux only: elsewhere the launcher would use your own install.")
    keep = "--keep" in sys.argv[1:]
    if not keep or not os.path.isdir(MODS):
        write_install()

    server = http.server.ThreadingHTTPServer(
        ("127.0.0.1", 0), partial(QuietHandler, directory=INDEX))
    base = f"http://127.0.0.1:{server.server_address[1]}"
    write_index(base)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    subprocess.run(["cargo", "build", "--quiet", "--bin", "DRH-Launcher"], cwd=REPO,
                   check=True)
    env = dict(os.environ, XDG_CONFIG_HOME=os.path.join(ROOT, "config"),
               XDG_DATA_HOME=os.path.join(ROOT, "data"),
               DRHL_MOD_INDEX_URL=f"{base}/index.json")
    try:
        return subprocess.run([LAUNCHER], env=env).returncode
    finally:
        server.shutdown()


if __name__ == "__main__":
    sys.exit(main())
