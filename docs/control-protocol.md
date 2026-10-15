# GridCraft control protocol

Start the desktop app with a control port:

```sh
gridcraft --control 7979            # or GRIDCRAFT_CONTROL_PORT=7979
```

Then send newline-delimited JSON to `127.0.0.1:7979`. Every request line gets one reply line:

```json
{"id": 1, "method": "engine.execute", "params": {"command": "cell.set", "params": {"cell": "B2", "input": "=SUM(A1:A9)"}}}
{"id": 1, "ok": true, "result": {"cell": "B2", "value": "45"}}
```

Errors come back as `{"id": …, "ok": false, "error": "…"}`.

## Methods

| Method | Params | What it does |
|---|---|---|
| `engine.execute` | `{command, params}` | Runs any engine command (see `engine.commands` or `gridcraft-cli commands`) |
| `engine.commands` | `{search?}` | Every command: id, label, ribbon path, shortcut, params doc, enabled; `search` keeps those whose id, label or ribbon path contains it (case-insensitive) |
| `engine.journal` | | Commands run so far (replayable). Passwords (`password`, `…Password` params) are left out, so a replayed Protect Sheet has no password |
| `document.inspect` | | Workbook summary: sheets, used ranges, tables, charts, names, selection, undo labels |
| `sheet.read` | `{range?, sheet?, formulas?, formatted?}` | Values (or formulas / displayed text) of a range |
| `cell.get` | `{cell?}` | One cell: value, formula, displayed text, style, merge, comment, link |
| `ui.inspect` | | UI state: ribbon tab, editor, open dialog, Backstage page, message, grid rect, scroll, perf |
| `ui.click` / `ui.drag` / `ui.move` | `{x, y, toX?, toY?, button?, count?, shift?, cmd?, alt?}` | Real pointer input in window points |
| `ui.cell.click` / `ui.cell.drag` | `{cell}` / `{from, to}` | Pointer input at a cell's centre (works in point mode while editing) |
| `ui.key` / `ui.text` | `{key, shift?, cmd?, alt?, ctrl?}` / `{text}` | Keyboard input (typing in the grid starts editing) |
| `ui.edit.begin` / `ui.edit.text` / `ui.edit.commit` / `ui.edit.cancel` | `{text?}` / `{move?: down\|right\|up\|left\|none}` | Drive the cell editor |
| `ui.ribbon` | `{tab}` | Switch ribbon tab |
| `ui.dialog` / `ui.dialog.set` / `ui.dialog.confirm` / `ui.dialog.cancel` | `{name}` / `{field, value}` | Open, fill in and confirm dialogs |
| `ui.backstage` (via `engine.execute`) | `{page?: home\|new\|open\|info\|saveAs\|print\|export\|options\|close}` | Open the File tab's Backstage view on a page, or close it (`ui.inspect` reports the open page as `backstage`) |
| `ui.screenshot` | `{path?}` | PNG of the window (base64 when no path). Animations are settled first: the picture shows the final state |
| `ui.set` | `{dark?, formulaBar?, ribbonCollapsed?, animations?}` | UI preferences (`formulaBar` runs `view.formulaBar`) |
| `ui.resize` / `ui.focus` | `{width, height}` | Window control |
| `app.open` / `app.save` / `app.quit` | `{path, password?}` / `{path?, password?}` | Files and lifetime. `password` opens a password-protected workbook; without it, such a workbook returns an error (never a dialog). On save, `password` encrypts the `.xlsx` (`""` saves it without one) |

Any other method name that is a command id (e.g. `home.bold`) runs that command.

Use `engine.execute` with `{command: "view.theme", params: {mode: "system"}}`
to choose the display theme; `mode` accepts `"system"`, `"light"`, or `"dark"`.
System follows live OS appearance changes and falls back to Light when the platform supplies no preference.
The saved choice remains System; new users still start in Light. The legacy
`view.darkMode` command and `ui.set {dark}` select a fixed Light or Dark theme.

`{command: "view.animations", params: {on: false}}` turns motion in the UI off (the selection
outline then jumps to a new selection instead of gliding there); without `on` it toggles. It is
the View ▸ Animations checkbox, saved with the other UI preferences and on by default.

`{command: "app.language.set", params: {language: "ja"}}` switches the interface language
(`code` is accepted as an alias for `language`); `app.language.english` and
`app.language.japanese` are shortcuts. Only interface chrome is translated, never command ids or
document text. The choice is saved in `ui.json`; with nothing saved the desktop app follows the
system language.

`{command: "file.open", params: {path: "Secret.xlsx", password: "…"}}` opens a password-protected
workbook (Agile or Standard encryption). Without `password` it returns an error containing
"is password-protected. Enter its password to open it." and never opens a dialog; a wrong
password returns "The password is incorrect." The password is left out of the journal and never
quoted in an error. The opened workbook keeps its file and, for this session only, its password:
Save writes it back encrypted with the same password.

`file.save` / `file.saveAs` / `file.saveBytes` take `password?`: a password to open, which
encrypts the `.xlsx` with Agile encryption (AES-256, SHA-512, salts and keys from the operating
system's secure random numbers, with the data-integrity HMAC Excel checks). Left out, the
workbook's own password applies; `""` saves without one. A password with `.csv`, `.json`, `.html`
or `.pdf` is an error. `file.encrypt {password, confirmPassword?}` sets the password to open (File >
Info > Encrypt with Password; `""` removes it) for the next save. Results report `encrypted`, never
the password. `file.encrypt` is not journaled.

For a headless equivalent (no window), use `gridcraft-cli mcp` or `gridcraft-cli run`;
see [`mcp.md`](mcp.md) and [`cli.md`](cli.md).

### Native printing

Desktop `engine.commands {"search":"print"}` (and connected MCP `list_commands`)
lists `ui.printSettings` and `ui.printSubmit` with their parameter and result fields.

`engine.execute` can call `ui.printSettings` with `{}` to inspect the printer list, selected
printer, copies, duplex, grayscale, discovery/submission status and backend receipt/error.
`{"refresh":true}` starts background discovery; poll the same command until `discovering` is
false. Set `printer` to a discovered name, integer `copies` from 1 to 999, `duplex` to
`printerDefault`, `off`, `longEdge` or `shortEdge`, and boolean `grayscale`. Invalid updates are
rejected without changing earlier settings. `ui.printSubmit` explicitly submits the current
print-ready PDF without opening a dialog; inspect `ui.printSettings` until `busy` becomes false
and check `status`/`detail`. The browser keeps its own print hook. PDF export remains separate.
GridCraft accepts the existing `file.print` sheet/pagination params on `ui.printSubmit`; the engine `file.print` command returns the PDF without spooling.
