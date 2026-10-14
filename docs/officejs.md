# Office add-ins (Office.js)

> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-14 · **Change:** major (first version: Office add-in manifests as plugins, the `office.sync` bridge, task panes, GridCraft's own Office.js) · **Target:** Microsoft Excel (Microsoft 365)

GridCraft runs **Office add-ins**, the web add-ins built with the Office JavaScript API
(`Office.js`), on top of its own add-ins ([plugins.md](plugins.md)). An Office add-in is
a web app on its vendor's server, described by an XML manifest. GridCraft reads the manifest into
a plugin: its ribbon buttons and right-click items are ordinary plugin items, listed in File ›
Options › Add-ins with the others, and its commands open its **task pane** (or call a function of
its function file) instead of starting a program.

The task pane shows the add-in's page in the system web view (WKWebView on macOS, WebView2 on
Windows). Before the page's own scripts run, GridCraft gives it **GridCraft's Office.js**
(`crates/office-js`), an implementation of the public API written from Microsoft's published
reference; Microsoft's `office.js` is never loaded, read or shipped. Each `context.sync()` of the
page becomes one `office.sync` command against the workbook.

```
 add-in page (vendor's https)              GridCraft
 ┌────────────────────────────────┐      ┌──────────────────────────────────────────┐
 │ the add-in's own scripts       │      │ office_pane (ui-egui): the pane, pump    │
 │   Excel.run(ctx => …)          │ JSON │ office.sync (engine): paths → objects,   │
 │   ctx.sync()                   │ ───► │   GridCraft commands, one undo step,     │
 │ GridCraft's Office.js (wasm)   │ ◄─── │   the manifest's permissions             │
 └────────────────────────────────┘      └──────────────────────────────────────────┘
   wry web view over the task pane (apps/gridcraft/src/office_web.rs)
```

## Installing an Office add-in

- Put the add-in's `manifest.xml` in a folder of the plugins folder (`<settings>/plugins/<name>/`),
  or load it with **Load from folder** / `plugins.load {"path": ".../manifest.xml"}`.
- The manifest must be for Excel (`<Host Name="Workbook"/>`), and its pages `https://`.
- It becomes the plugin `office.<its Id>`. Its default task pane (`<DefaultSettings><SourceLocation>`)
  is the command `taskpane`; each add-in command (`<VersionOverrides>`: buttons on Office tabs or
  custom tabs, menus, the cell's right-click menu) is a command of its own.

| Manifest | GridCraft |
|---|---|
| `<OfficeTab id="TabHome">` … `TabInsert`, `TabFormulas`, `TabData`, `TabReview`, `TabView`, `TabPageLayoutExcel`, `TabDraw`, `TabDeveloper` | That built-in tab (Developer → Automate); other ids → an **Add-ins** tab |
| `<CustomTab>` | A tab of its own, named by its `<Label>` |
| `<Control xsi:type="Button">`, the items of `xsi:type="Menu"` | Buttons (a menu's items are its group's buttons) |
| `<Action xsi:type="ShowTaskpane">` | Opens the page in the task pane (`TaskpaneId`s that match share a pane) |
| `<Action xsi:type="ExecuteFunction">` | Calls the function in the function file's page, with no pane |
| `<OfficeMenu id="ContextMenuCell">` | Items in the cell's right-click menu |
| `<Permissions>` | Restricted: settings only; ReadDocument / ReadAllDocument: read; WriteDocument: write; ReadWriteDocument: both |

Icons from the manifest aren't shown yet (the buttons get GridCraft's script icon). The unified
JSON manifest isn't read yet.

## What the API covers

Requirement sets reported as supported: **ExcelApi 1.1–1.4** for the objects below; anything else
answers `NotImplemented`, so add-ins that check `isSetSupported` turn the feature off.

| Object | Properties and methods |
|---|---|
| `Office` | `onReady`, `initialize`, `context.diagnostics`, `context.requirements.isSetSupported`, `context.document.getSelectedDataAsync` / `setSelectedDataAsync` (text, matrix), `context.document.settings` (`get`, `set`, `remove`, `saveAsync`) |
| `Excel.run`, `RequestContext` | `workbook`, `sync()` (one undo step; a failure takes the whole batch back) |
| `Workbook` | `worksheets`, `getSelectedRange()`, `getActiveCell()`, `name` |
| `WorksheetCollection` | `getActiveWorksheet()`, `getItem(name)`, `add(name)`, `items` |
| `Worksheet` | `name` (read, rename), `position`, `getRange(address?)`, `getCell(r, c)`, `getUsedRange()`, `activate()`, `delete()` |
| `Range` | `values`, `formulas`, `numberFormat`, `text`, `address`, `rowCount`, `columnCount`, `rowIndex`, `columnIndex`, `cellCount`; `getCell`, `getOffsetRange`, `getResizedRange`, `getRow`, `getColumn`, `getEntireRow`, `getEntireColumn`, `getUsedRange`, `clear(applyTo)`, `select()` |
| `Range.format.font` | `bold`, `italic`, `underline`, `strikethrough`, `color`, `size`, `name` |
| `Range.format.fill` | `color` |

One difference from Excel: a `sync()` is all or nothing. If an action fails, the earlier ones of
the same batch are taken back too, and the error says which action failed (`debugInfo`).

## The bridge (`office.sync`)

The page's library sends `{"id": n, "actions": [...]}` through the web view; GridCraft answers
`{"id": n, "results": [...]}` or `{"id": n, "error": {"code", "message", "action"}}`. Each action
names an object by its **path** from the workbook (`{"p": property}`, `{"m": method, "a": args}`):

```json
{"op": "set",  "path": [{"p": "workbook"}, {"p": "worksheets"}, {"m": "getItem", "a": ["Data"]}, {"m": "getRange", "a": ["A1:B2"]}], "prop": "values", "value": [[1, 2], [3, 4]]}
{"op": "load", "path": [{"p": "workbook"}, {"m": "getSelectedRange"}], "props": ["values", "address"]}
{"op": "call", "path": [{"p": "workbook"}, {"m": "getSelectedRange"}], "method": "clear", "args": ["Formats"]}
```

Paths start at `workbook`, `document` (the common API's selection) or `settings`. Paths are
resolved again for every action, so a range is its address (as Excel's ranges are). Methods
with an effect (`worksheets.add`) are `call`s, never steps of a path. Writes go through
GridCraft's commands (`range.setValues`, `home.bold`, `sheet.rename`…), reads through `sheet.read`
and `cell.get`. Agents can send the same batches with the `office.sync` command.

Settings are kept per workbook and add-in while GridCraft runs; saving them in the file (the
web-extension parts of the XLSX package, [MS-OWEXML]) comes next.

## Security

- The add-in's page runs in the web view's sandbox, on its own `https://` origin; its access to
  the workbook is the bridge's, bounded by the manifest's permissions.
- The web view only navigates to `https://` pages; links that open a window go to the browser.
- GridCraft never contacts Microsoft: the add-in's page is loaded from its vendor, and the API is
  GridCraft's.

## Limits and next steps

- **Platforms:** the desktop app on macOS and Windows. Linux and FreeBSD need WebKitGTK 4.1 in the
  packages; the web app needs an iframe host.
- **The bootstrap:** the API has to exist before the page's scripts run, and a wasm module of this
  size can't start synchronously in WebView2, so a few lines of JavaScript (`crates/office-js/bootstrap.js`)
  hold the first calls until the library is ready. It is the one hand-written script in GridCraft.
- Not yet: events (`onChanged`, `onSelectionChanged`), the dialog API, custom functions (they need
  asynchronous calculation; see [gaps.md](gaps.md) #21 and the live-data decision), tables,
  charts, named items, conditional formats, the unified JSON manifest, AppSource, single sign-on
  with Microsoft accounts (`getAccessToken`), manifest icons.
- Installing an Office add-in from a manifest URL in Options › Add-ins.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-14 | major | Created: Office add-in manifests as plugins, task panes in the system web view, GridCraft's Office.js, the `office.sync` bridge (ExcelApi 1.1–1.4 subset, common API selection and settings) |
