# Plugins

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-14 · **Change:** minor (protocol 1.1: host requests, macros and the Developer tab; previously minor: Office add-ins load as plugins, officejs.md; previously minor: one plugin for several apps: `apps`, `uiFor`, `host.id`; previously minor: install plugins from a GitHub repository) · **Target:** Microsoft Excel (Microsoft 365)

A plugin adds **functions** that formulas call and **commands** the user (or an agent) runs,
and can put its commands on the ribbon (a group on a built-in tab, or a tab of its own) and in
the right-click menus.
It's a separate program, written in any language, that GridCraft starts and talks to over its
standard input and output. This is GridCraft's equivalent of Excel's add-ins (custom functions,
XLL and COM add-ins). VBA stays out of scope.

Because a plugin runs in its own process, a crash, a hang or a bad answer costs a cell an error
value, never the workbook. The host's code is `crates/plugin`; the reference plugin is
`crates/plugin/src/bin/example_plugin.rs`.

**One plugin, several apps.** The manifest and the protocol are shared with WordCraft, so one
plugin can serve both: `apps` names the apps it's for, `uiFor` gives each app its own ribbon and
menus, and `initialize` says which app started it (`host.id`). WordCraft plugins have commands
only; functions are GridCraft's. A test plugin written in Python that works in both is
[sirmmo/gridcraft-python-plugin](https://github.com/sirmmo/gridcraft-python-plugin).

**Office add-ins** (Office.js web add-ins with an XML manifest) load as plugins too: their
commands open their task pane in a web view; see [officejs.md](officejs.md).

Live data from sources and streams (protocol 2) and co-editing come later; see
[gaps.md](gaps.md) #16 and #21.

## Installing a plugin

**From GitHub.** In **File › Options › Add-ins** (or **Automate › Add-ins**), paste the
repository's address in **Install from GitHub** and confirm. GridCraft downloads it in the
background, unpacks it into the plugins folder of the settings (as `<plugin id>/`, replacing an
earlier install of the same plugin) and loads it. Agents and the CLI use
`plugins.install {"url": …, "dir": …}`. Addresses it understands:

| Address | Installs |
|---|---|
| `https://github.com/owner/repo` (or `github.com/owner/repo`, `owner/repo`) | The latest release; the default branch's source when there is no release |
| `…/releases/tag/v1.2.0` | That release |
| `…/tree/<branch, tag or commit>` | That source |

From a release, GridCraft takes the `.zip` asset built for this computer when there is one
(see [Publishing a plugin](#publishing-a-plugin-on-github)), else the release's source code. The
plugin is the folder in the zip that holds `plugin.json` (the shallowest one). Downloads are
limited to 256 MiB and 512 MiB unpacked; paths that would leave the plugin's folder and symbolic
links are skipped. GitHub allows 60 anonymous requests an hour per address; private repositories
can't be installed. `.craft-source.json` in the plugin's folder records where it came from,
and Options shows the repository.

**From a folder.** A plugin is a folder holding a `plugin.json` manifest and whatever the plugin
needs to run. GridCraft loads plugins from:

1. each folder in `GRIDCRAFT_PLUGIN_PATH` (separated like `PATH`);
2. `--plugin DIR` on the command line (`gridcraft` and every `gridcraft-cli` subcommand; repeatable);
3. (desktop app only) the `plugins` folder in the settings folder: `~/.config/gridcraft/plugins`
   on Linux and BSD, `~/Library/Application Support/GridCraft/plugins` on macOS,
   `%APPDATA%\GridCraft\plugins` on Windows.

A folder holding `plugin.json` is one plugin. Otherwise, each of its subfolders that holds one is
a plugin. Loading only reads the manifest. The plugin's process starts the first time one of its
functions or commands is used.

**File › Options › Add-ins** lists the installed plugins with their version, author and status.
**Documentation** opens a plugin's web page (its `homepage`), and **Deactivate** turns a plugin
off: it stays listed, but its functions (cells show `#NAME?`), commands, ribbon buttons and menu
items are gone and its process stops. **Activate** brings it back. The app remembers which
plugins are off (`disabledPlugins` in `prefs.json`), so they stay off at the next start and when
loaded again.

**Automate › Add-ins** (also **Manage Add-ins…** in Options) lists the loaded plugins with their
functions, runs their commands, turns them off and on, and loads or unloads a plugin. Commands do
the same:

| Command | Params | Does |
|---|---|---|
| `plugins.list` (also `home.addIns`) | `{}` | The loaded plugins with their status, functions and commands |
| `plugins.load` | `{path}` | Loads the plugin in a folder (or a manifest file). A plugin with the same id is replaced |
| `plugins.install` | `{url, dir?}` | Installs the plugin in a GitHub repository into `dir` (default: the app's plugins folder) and loads it |
| `plugins.unload` | `{id}` | Unloads a plugin and stops its process (until the next start) |
| `plugins.setEnabled` | `{id, enabled}` | Turns a plugin off or back on, and remembers it in the preferences |
| `plugins.openHomepage` | `{id}` | Opens the plugin's web page in the browser |
| `plugins.run` | `{plugin, command, params?}` | Runs a plugin command (one undoable step) |

Open workbooks that call a plugin's functions are recalculated when it loads or unloads.

```sh
gridcraft-example-plugin --manifest > ~/plugins/example/plugin.json
gridcraft-cli --plugin ~/plugins/example eval '=EXAMPLE.HELLO("world")'     # Hello, world!
gridcraft-cli --plugin ~/plugins/example run --sample sales --quiet \
  --cmd 'selection.set={"range":"K1"}' --cmd 'plugins.run={"plugin":"org.gridcraft.example","command":"countUp","params":{"count":5}}' --print K1:K5
```

**Trust.** A plugin is a program running with your permissions, like any app you install.
Install plugins only from people you trust. GridCraft doesn't sandbox them.

## The manifest

```json
{
  "id": "org.example.finance",
  "name": "Finance tools",
  "version": "1.2.0",
  "description": "Prices and rates",
  "author": "Example Corp",
  "homepage": "https://example.org/finance-plugin",
  "apps": ["gridcraft"],
  "run": {"command": "python3", "args": ["main.py"]},
  "functions": [
    {
      "name": "FIN.PRICE",
      "signature": "FIN.PRICE(ticker, [date])",
      "description": "Closing price",
      "minArgs": 1,
      "maxArgs": 2,
      "volatile": false,
      "timeoutMs": 5000
    }
  ],
  "commands": [
    {"id": "refreshAll", "label": "Refresh Prices", "description": "…", "params": "{sheet?}"}
  ],
  "ui": {
    "ribbon": [
      {"tab": "Finance", "group": "Prices", "buttons": [
        {"command": "refreshAll", "icon": "refresh"},
        {"command": "refreshAll", "label": "This Sheet", "size": "small", "params": {"sheet": "current"}}
      ]},
      {"tab": "Data", "group": "Finance", "buttons": [{"command": "refreshAll", "icon": "currency"}]}
    ],
    "menus": [
      {"menu": "cell", "command": "refreshAll", "label": "Refresh Prices Here"},
      {"menu": "sheetTab", "command": "refreshAll"}
    ]
  }
}
```

| Field | Meaning |
|---|---|
| `id` | Unique id: 1–128 letters, digits, `.`, `-`, `_`. Reverse-domain style is recommended |
| `apps` | The apps the plugin is for (`gridcraft`, `wordcraft`; any case). Left out: any app. A plugin made only for other apps isn't loaded |
| `author`, `homepage` | Shown in Options › Add-ins. `homepage` is an `http://` or `https://` link to the plugin's documentation |
| `run.command`, `run.args` | The program to start. A command containing `/` or `\` is relative to the plugin folder; a bare name is looked up on `PATH`. It runs in the plugin folder, with `GRIDCRAFT_PLUGIN_DIR`, `CRAFT_PLUGIN_DIR` (the folder), `CRAFT_HOST` (`gridcraft`) and `GRIDCRAFT_VERSION` set |
| `functions[].name` | The name formulas use (case-insensitive). It starts with a letter and holds letters, digits, `.` and `_`. It can't be a built-in function or another plugin's function. Prefix it with your own namespace (`FIN.`) |
| `minArgs`, `maxArgs` | Argument count (default 0 and 255). Other counts give `#VALUE!` without calling the plugin |
| `volatile` | Recalculate on every recalculation, like `RAND`. Otherwise a result is reused for the same arguments while the plugin is loaded (up to 10,000 results), and the cell is recalculated only when its precedents change |
| `timeoutMs` | How long a call may take: default 2,000, at most 30,000 |
| `commands[]` | `id` (unique in the plugin), `label`, `description`, `params` (documentation) |
| `uiFor.<app>` | Ribbon groups and menu items for one app: `uiFor.gridcraft` is used instead of `ui` when it's there |
| `ui.ribbon[]` | A group of buttons: `tab`, `group`, `buttons`. A built-in tab's English name (`Home`, `Insert`, `Draw`, `Page Layout`, `Formulas`, `Data`, `Review`, `View`, `Automate`) adds the group at the end of that tab; any other name makes a new tab after Automate, shared by the plugins that name it |
| `buttons[]` | `command` (one of the plugin's `commands[].id`), `label` and `tooltip` (default: the command's label and description), `icon`, `size` (`large`, the default, or `small`: stacked three high), `params` (passed to the command) |
| `ui.menus[]` | A right-click menu item: `menu` (`cell`, the default; `row` or `column` for the headers; `sheetTab`), `command`, `label`, `params`. Items go at the end of the menu, after a separator. A `sheetTab` item also gets `sheet` (the tab's index) in its params |

Icons are named from GridCraft's own icon set: `script` (the default), `refresh`, `calc`, `sum`,
`function`, `fx`, `table`, `pivotTable`, `chart`, `chartBar`, `chartLine`, `chartPie`,
`chartScatter`, `sparkline`, `picture`, `link`, `comment`, `note`, `filter`, `sort`, `sortDesc`,
`find`, `search`, `lock`, `settings`, `share`, `sheet`, `workbook`, `folder`, `save`, `print`,
`calendar`, `clock`, `info`, `check`, `plus`, `minus`, `pen`, `currency`, `percent`,
`validation`, `name`, `zoom`, `more`, `copy`, `cut`, `paste`, `insert`, `delete`, `format`.
An unknown name gets the default. Plugins can't ship their own icon images yet. Labels are at
most 64 characters. Buttons and menu items run the command exactly as `plugins.run` does, as one
undoable step.

## Protocol (version 1)

JSON-RPC 2.0, **one JSON message per line**, UTF-8. GridCraft writes requests to the plugin's
standard input; the plugin writes one response per request to standard output. Lines on
standard output that aren't JSON are logged and ignored. Standard error goes to GridCraft's log.
The plugin exits when its standard input closes, or when it receives the `shutdown`
notification.

### `initialize`

Sent once, after the process starts. Answer within 10 s with any result, for example
`{"protocolVersion": 1}`.

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"host":{"id":"gridcraft","name":"GridCraft","version":"0.6.1"},"plugin":{"id":"org.example.finance","dir":"/home/me/plugins/finance"}}}
```

### `functions/call`

```json
{"jsonrpc":"2.0","id":2,"method":"functions/call","params":{"name":"FIN.PRICE","args":["ACME",[[1,2],[3,null]]]}}
{"jsonrpc":"2.0","id":2,"result":{"value":42.5}}
```

Arguments arrive evaluated. A reference arrives as the array of its values. Values on the wire:

| Cell value | JSON |
|---|---|
| empty | `null` |
| number | number |
| text | string (up to 32,767 characters) |
| logical | `true` / `false` |
| error | `{"error": "#N/A"}` (any Excel error text) |
| array | rows of values: `[[1, 2], [3, 4]]`. A result may also be a flat list (one row). Ragged rows are padded with `#N/A`. An array result spills |

A call carries at most 1,000,000 cells, and a result may hold as many. Anything larger gives
`#VALUE!` (arguments) or `#NUM!` (result).

### `commands/run`

```json
{"jsonrpc":"2.0","id":3,"method":"commands/run","params":{"command":"refreshAll","params":{},
  "context":{"workbook":"Book1","sheet":"Sheet1","active":"B2","selection":"B2:C3","values":[[1,2],[3,4]]}}}
{"jsonrpc":"2.0","id":3,"result":{"commands":[{"command":"range.setValues","params":{"range":"B2","values":[[5],[6]]}}],"message":"Updated 2 prices","result":{"any":"json"}}}
```

`context.values` is included for selections up to 100,000 cells. The plugin answers with engine
commands to run. They are the same commands the CLI, MCP and control channel use (see
`gridcraft-cli commands`), except `plugins.*`. GridCraft runs them in order, as **one undoable
step**: if any fails, the workbook goes back to how it was and the error is shown. `message` is
shown to the user, and `result` is passed back to the caller of `plugins.run`. A command may
take up to 60 s.

### Errors and failures

- An answer `{"error": {"code": …, "message": "…"}}` gives the cell `#VALUE!`, or fails the
  command with that message. The process keeps running.
- A call that times out, a process that exits, or broken output gives `#VALUE!`. The process is
  stopped and started again on the next call.
- After 3 such failures the plugin is marked `failed` and its functions give `#VALUE!` until
  it's reloaded (`plugins.load`).

### Writing a plugin

Any language that reads and writes lines works. A complete Python plugin with one function:

```python
import json, sys

for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:          # notifications: shutdown
        break
    method, params = msg["method"], msg.get("params", {})
    if method == "initialize":
        result = {"protocolVersion": 1}
    elif method == "functions/call" and params["name"] == "PY.TWICE":
        x = params["args"][0]
        result = {"value": x * 2 if isinstance(x, (int, float)) else {"error": "#VALUE!"}}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32601, "message": "unknown"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
```

```json
{"id": "org.example.py", "name": "Python example", "run": {"command": "python3", "args": ["plugin.py"]},
 "functions": [{"name": "PY.TWICE", "minArgs": 1, "maxArgs": 1}]}
```

## Protocol 1.1: asking the host, and macros

**The plugin asks the host while it works.** While GridCraft waits for its answer to a request
(`commands/run`, `macros/run`…), a plugin may send requests of its own on standard output — JSON-RPC
requests with its own ids — and read GridCraft's answers on standard input before it goes on. Each
answer starts the request's timeout again. Everything these requests change is part of the same
undoable step, and is taken back if the plugin's answer is an error.

| Request | Params | Answer |
|---|---|---|
| `host/sync` | `{actions}` | `{results}`, or an error whose `data` is `{code, message, action}`. The actions are the Office bridge's ([officejs.md](officejs.md#the-bridge-officesync)): paths from `workbook` to worksheets, ranges, formats; `load`, `set`, `call` |
| `host/vbaProject/get` | `{}` | `{base64}`: the workbook's `vbaProject.bin`, or `null` |
| `host/vbaProject/set` | `{base64}` | `{}`: replaces it (the workbook then saves as `.xlsm`); at most 64 MiB |
| `host/message` | `{text, title?}` | `{result: 1}`: shows a message after the step (it doesn't wait for the user yet) |
| `host/log` | `{text}` | `{}`: writes to GridCraft's log |

A plugin calling the host can make GridCraft recalculate a formula that calls one of the plugin's
own functions; that call fails (`#VALUE!`) instead of waiting for the busy plugin.

**Macros.** A plugin that declares `"macros": {"language": "VBA"}` runs macros, and brings the
**Developer** tab: Visual Basic (the workbook's code modules in an editor), Macros (run, edit,
create), Record Macro, Macro Security and Add-ins. GridCraft draws all of it; the plugin owns the
language. It answers:

| Request | Params | Answer |
|---|---|---|
| `macros/list` | `{}` | `{macros: [{name: "Module1.Hello", module, procedure}]}` |
| `macros/run` | `{name}` | `{}`, or an error (`Run-time error '13': Type mismatch`, with `data: {module, line}`) |
| `macros/modules` | `{}` | `{modules: [{name, kind: standard\|class\|document, source}]}` |
| `macros/saveModule` | `{name, source}` | `{}` |
| `macros/deleteModule` | `{name}` | `{}` |
| `macros/record` | `{steps: [{command, params}]}` | `{source}`: the recorded steps as a macro |

Commands: `developer.macros`, `developer.runMacro {name}`, `developer.modules`,
`developer.saveModule {name, source}`, `developer.deleteModule {name}`, `developer.recordMacro {stop?}`,
`developer.recordToCode {steps}`, `developer.enableMacros`, `developer.macroSecurity {level}`.

**Macro security** (Developer › Macro Security, `macroSecurity` in the preferences): `prompt`, the
default, runs a workbook's macros once the user enabled them for it (the message bar's **Enable
Content** on a workbook with macros, `developer.enableMacros`); `disable` runs none; `enable` runs
all. A macro is a program: agents and the CLI are bound by the same setting.

## Publishing a plugin on GitHub

Put `plugin.json` at the root of the repository (or in one folder of it). A plugin written in
a scripting language (`"run": {"command": "python3", "args": ["main.py"]}`) needs nothing more:
GridCraft installs the source.

A compiled plugin attaches one `.zip` per platform to each GitHub release, holding `plugin.json`
and the program (`"run": {"command": "./my-plugin"}`; on Windows `./my-plugin.exe`). Name each zip
after its platform and architecture, separated by `-`, `.` or `+`:

| Platform | Words GridCraft recognises |
|---|---|
| macOS | `macos`, `darwin`, `mac`, `osx`, `apple`; `universal` for both architectures |
| Windows | `windows`, `win`, `win64`, `win32` |
| Linux | `linux` |
| FreeBSD | `freebsd` |
| Apple silicon, ARM | `aarch64`, `arm64` |
| Intel, AMD | `x86_64`, `x64`, `amd64` |

For example `finance-1.2.0-macos-aarch64.zip`, `finance-1.2.0-windows-x64.zip`. A zip naming no
platform is used on every platform that has no zip of its own. Executable permissions in the zip
are kept.

## Limits and next steps

- Calls are synchronous. A slow function holds up recalculation for up to its timeout.
  Asynchronous functions (`#GETTING_DATA` until the answer arrives) come with protocol 2.
- Protocol 2 (planned): data sources and streams pushed by the plugin (`STREAM()`, `RTD`), with
  targeted recalculation of the cells that use them.
- Plugin functions don't appear in Insert Function or formula autocomplete yet.
- Installed plugins aren't updated automatically; install again to update. There's no
  Uninstall button yet: unload the plugin and delete its folder.
- Ribbon groups go at the end of a tab; plugins can't place them between built-in groups, add
  drop-downs or toggles, or ship their own icons yet. Their labels aren't translated.
- The web app can't start processes, so plugins need the desktop app or the CLI.
- Workbooks don't record which plugin a formula needs. Without the plugin, its cells show
  `#NAME?` after a recalculation. In XLSX, the formula text is kept as written.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-14 | minor | Protocol 1.1: `host/*` requests while a plugin works (one undo step), macro plugins (`macros/*`), the Developer tab and Macro Security |
| 2026-10-14 | minor | Office add-ins (`manifest.xml`) load as plugins whose commands open task panes ([officejs.md](officejs.md)) |
| 2026-10-14 | minor | One plugin for several apps: `apps`, `uiFor`, `host.id` in `initialize`, `CRAFT_HOST`/`CRAFT_PLUGIN_DIR`; the shared protocol WordCraft speaks too; `.craft-source.json` |
| 2026-10-13 | minor | Install from a GitHub repository (`plugins.install`, Options and Automate › Add-ins); wider add-in list |
| 2026-10-13 | minor | Options › Add-ins: turn plugins off and on (remembered), `homepage` and `author`, `plugins.setEnabled`, `plugins.openHomepage` |
| 2026-10-13 | minor | `ui` in the manifest: ribbon groups on built-in or new tabs, items in the cell, row, column and sheet-tab menus |
| 2026-10-13 | major | Created: out-of-process plugin host, manifest, protocol 1 (functions, commands), Add-ins dialog |
